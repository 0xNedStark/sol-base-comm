//! Executes the REAL SBF build of the outbox program under solana-program-test
//! (BanksClient). From `solana/`: build both programs with `cargo build-sbf`,
//! then `cd program-tests && SBF_OUT_DIR=../target/deploy cargo test`.
//!
//! The transport is a mock program that accepts any instruction, so this
//! exercises the whole prepare -> dispatch -> finalize state machine including
//! the CPI shape and the PDA-signed emitter, but not the real bridge's account
//! validation.

use anchor_lang::{AccountDeserialize, InstructionData, ToAccountMetas};
use base_caller::envelope::{CHAIN_BASE, HEADER_SIZE, MODE_DIRECT, SRC_CHAIN_SOLANA};
use base_caller::errors::BaseCallerError;
use base_caller::state::{Config, PreparedMessage, SenderState, TransportConfig};
use base_caller::transports::{
    MASK_LAYERZERO, MASK_WORMHOLE, TRANSPORT_LAYERZERO, TRANSPORT_WORMHOLE,
};
use base_caller::SendParams;
use solana_program_test::*;
use solana_sdk::{
    instruction::{Instruction, InstructionError},
    pubkey::Pubkey,
    signature::{Keypair, Signer},
    system_program, sysvar,
    transaction::{Transaction, TransactionError},
};

struct Env {
    banks: BanksClient,
    payer: Keypair,
    program_id: Pubkey,
    mock_wh: Pubkey,
    config: Pubkey,
    transport: Pubkey,
    emitter: Pubkey,
}

impl Env {
    async fn new() -> Self {
        let program_id = base_caller::id();
        let mock_wh = mock_transport::id();
        // `None` = load <name>.so from SBF_OUT_DIR: the real build, not a
        // native re-entry, so what runs here is what would run on a validator.
        let mut pt = ProgramTest::new("base_caller", program_id, None);
        pt.add_program("mock_transport", mock_wh, None);
        let (banks, payer, _) = pt.start().await;
        let (config, _) = Pubkey::find_program_address(&[Config::SEED], &program_id);
        let (transport, _) =
            Pubkey::find_program_address(&[TransportConfig::SEED, &[TRANSPORT_WORMHOLE]], &program_id);
        let (emitter, _) = Pubkey::find_program_address(&[b"emitter"], &program_id);
        Self { banks, payer, program_id, mock_wh, config, transport, emitter }
    }

    async fn send(&mut self, ix: Instruction, extra: &[&Keypair]) -> Result<(), TransactionError> {
        let hash = self.banks.get_latest_blockhash().await.unwrap();
        let mut signers: Vec<&Keypair> = vec![&self.payer];
        signers.extend_from_slice(extra);
        let tx = Transaction::new_signed_with_payer(&[ix], Some(&self.payer.pubkey()), &signers, hash);
        self.banks.process_transaction(tx).await.map_err(|e| e.unwrap())
    }

    async fn account<T: AccountDeserialize>(&mut self, key: Pubkey) -> Option<T> {
        let acc = self.banks.get_account(key).await.unwrap()?;
        Some(T::try_deserialize(&mut &acc.data[..]).unwrap())
    }

    // The bridge-owned PDAs, derived from the configured bridge program id --
    // here the mock. The program derives the same addresses and rejects
    // anything else, so these must be right for the test to pass.
    fn bridge_pda(&self) -> Pubkey {
        Pubkey::find_program_address(&[b"Bridge"], &self.mock_wh).0
    }

    fn fee_collector_pda(&self) -> Pubkey {
        Pubkey::find_program_address(&[b"fee_collector"], &self.mock_wh).0
    }

    fn sequence_pda(&self) -> Pubkey {
        Pubkey::find_program_address(&[b"Sequence", self.emitter.as_ref()], &self.mock_wh).0
    }

    fn sender_state(&self, authority: &Pubkey) -> Pubkey {
        Pubkey::find_program_address(&[SenderState::SEED, authority.as_ref()], &self.program_id).0
    }

    fn message(&self, authority: &Pubkey, nonce: u64) -> Pubkey {
        Pubkey::find_program_address(
            &[PreparedMessage::SEED, authority.as_ref(), &nonce.to_le_bytes()],
            &self.program_id,
        )
        .0
    }

    async fn initialize(&mut self) {
        let ix = Instruction {
            program_id: self.program_id,
            accounts: base_caller::accounts::Initialize {
                config: self.config,
                payer: self.payer.pubkey(),
                system_program: system_program::id(),
            }
            .to_account_metas(None),
            data: base_caller::instruction::Initialize { admin: self.payer.pubkey() }.data(),
        };
        self.send(ix, &[]).await.unwrap();
    }

    async fn set_wormhole_transport(&mut self) {
        let ix = Instruction {
            program_id: self.program_id,
            accounts: base_caller::accounts::SetTransport {
                config: self.config,
                transport: self.transport,
                admin: self.payer.pubkey(),
                system_program: system_program::id(),
            }
            .to_account_metas(None),
            data: base_caller::instruction::SetTransport {
                transport_id: TRANSPORT_WORMHOLE,
                enabled: true,
                program_id: self.mock_wh,
                peer: [0xAA; 32],
                dest_chain: 30,
                dispatcher: Pubkey::default(), // dispatched in-process
            }
            .data(),
        };
        self.send(ix, &[]).await.unwrap();
    }

    fn lz_transport(&self) -> Pubkey {
        Pubkey::find_program_address(
            &[TransportConfig::SEED, &[TRANSPORT_LAYERZERO]],
            &self.program_id,
        )
        .0
    }

    /// Register LayerZero with the mock program as its EXTERNAL dispatcher --
    /// the arrangement a transport whose SDK cannot be linked in must use.
    async fn set_layerzero_external(&mut self) {
        let ix = Instruction {
            program_id: self.program_id,
            accounts: base_caller::accounts::SetTransport {
                config: self.config,
                transport: self.lz_transport(),
                admin: self.payer.pubkey(),
                system_program: system_program::id(),
            }
            .to_account_metas(None),
            data: base_caller::instruction::SetTransport {
                transport_id: TRANSPORT_LAYERZERO,
                enabled: true,
                program_id: self.mock_wh,
                peer: [0xBB; 32],
                dest_chain: 30184,
                dispatcher: self.mock_wh,
            }
            .data(),
        };
        self.send(ix, &[]).await.unwrap();
    }

    /// Drive the mock as an external dispatcher: it CPIs back into
    /// `mark_dispatched`, signing as its own dispatcher PDA.
    async fn external_dispatch(&mut self, authority: &Pubkey, nonce: u64) -> Result<(), TransactionError> {
        let dispatcher_authority =
            Pubkey::find_program_address(&[b"dispatcher"], &self.mock_wh).0;
        let ix = Instruction {
            program_id: self.mock_wh,
            accounts: mock_transport::accounts::DispatchAndMark {
                base_caller_program: self.program_id,
                config: self.config,
                transport: self.lz_transport(),
                message: self.message(authority, nonce),
                authority: *authority,
                dispatcher_authority,
            }
            .to_account_metas(None),
            data: mock_transport::instruction::DispatchAndMark { nonce, transport_id: TRANSPORT_LAYERZERO }
                .data(),
        };
        self.send(ix, &[]).await
    }

    fn params() -> SendParams {
        SendParams {
            dst_chain_id: CHAIN_BASE,
            target: [0xAB; 20],
            value: 0,
            gas_limit: 200_000,
            expiry: 0,
            mode: MODE_DIRECT,
            calldata: vec![0xde, 0xad, 0xbe, 0xef],
        }
    }

    async fn prepare(&mut self, authority: &Keypair, next_nonce: u64, transports: u8) -> Result<(), TransactionError> {
        let ix = Instruction {
            program_id: self.program_id,
            accounts: base_caller::accounts::Prepare {
                config: self.config,
                sender_state: self.sender_state(&authority.pubkey()),
                message: self.message(&authority.pubkey(), next_nonce),
                authority: authority.pubkey(),
                payer: self.payer.pubkey(),
                system_program: system_program::id(),
            }
            .to_account_metas(None),
            data: base_caller::instruction::Prepare { params: Self::params(), transports }.data(),
        };
        self.send(ix, &[authority]).await
    }

    async fn dispatch_wormhole(&mut self, authority: &Pubkey, nonce: u64) -> Result<(), TransactionError> {
        let wh_message = Keypair::new();
        let ix = Instruction {
            program_id: self.program_id,
            accounts: base_caller::accounts::DispatchViaWormhole {
                config: self.config,
                transport: self.transport,
                message: self.message(authority, nonce),
                authority: *authority,
                payer: self.payer.pubkey(),
                wormhole_bridge: self.bridge_pda(),
                wormhole_message: wh_message.pubkey(),
                wormhole_emitter: self.emitter,
                wormhole_sequence: self.sequence_pda(),
                wormhole_fee_collector: self.fee_collector_pda(),
                wormhole_program: self.mock_wh,
                clock: sysvar::clock::id(),
                rent: sysvar::rent::id(),
                system_program: system_program::id(),
            }
            .to_account_metas(None),
            data: base_caller::instruction::DispatchViaWormhole { nonce, batch_nonce: 0 }.data(),
        };
        self.send(ix, &[&wh_message]).await
    }

    async fn finalize(&mut self, authority: &Pubkey, nonce: u64) -> Result<(), TransactionError> {
        let ix = Instruction {
            program_id: self.program_id,
            accounts: base_caller::accounts::Finalize {
                message: self.message(authority, nonce),
                authority: *authority,
                payer: self.payer.pubkey(),
            }
            .to_account_metas(None),
            data: base_caller::instruction::Finalize { nonce }.data(),
        };
        self.send(ix, &[]).await
    }
}

fn custom(err: BaseCallerError) -> TransactionError {
    TransactionError::InstructionError(0, InstructionError::Custom(u32::from(err)))
}

#[tokio::test]
async fn prepare_dispatch_finalize_round_trip() {
    let mut env = Env::new().await;
    env.initialize().await;
    env.set_wormhole_transport().await;
    let authority = Keypair::new();

    // prepare: nonce goes 0 -> 1, PDA keyed by nonce 1, envelope bytes correct
    env.prepare(&authority, 1, MASK_WORMHOLE).await.unwrap();
    let ss: SenderState = env.account(env.sender_state(&authority.pubkey())).await.unwrap();
    assert_eq!(ss.nonce, 1);
    let msg: PreparedMessage = env.account(env.message(&authority.pubkey(), 1)).await.unwrap();
    assert_eq!(msg.nonce, 1);
    assert_eq!(msg.expected, MASK_WORMHOLE);
    assert_eq!(msg.dispatched, 0);
    assert_eq!(msg.payer, env.payer.pubkey());
    assert_eq!(msg.envelope.len(), HEADER_SIZE + 4);
    assert_eq!(msg.envelope[0], 1, "version");
    assert_eq!(u16::from_be_bytes([msg.envelope[2], msg.envelope[3]]), SRC_CHAIN_SOLANA);
    assert_eq!(u16::from_be_bytes([msg.envelope[4], msg.envelope[5]]), CHAIN_BASE);
    assert_eq!(&msg.envelope[6..38], authority.pubkey().as_ref(), "sender is the authority");
    assert_eq!(u64::from_be_bytes(msg.envelope[38..46].try_into().unwrap()), 1, "nonce");
    assert_eq!(&msg.envelope[HEADER_SIZE..], &[0xde, 0xad, 0xbe, 0xef]);

    // dispatch: CPI to the (mock) bridge with the PDA-signed emitter; bit set
    env.dispatch_wormhole(&authority.pubkey(), 1).await.unwrap();
    let msg: PreparedMessage = env.account(env.message(&authority.pubkey(), 1)).await.unwrap();
    assert_eq!(msg.dispatched, MASK_WORMHOLE);

    // dispatching the same transport twice is refused
    let err = env.dispatch_wormhole(&authority.pubkey(), 1).await.unwrap_err();
    assert_eq!(err, custom(BaseCallerError::AlreadyDispatched));

    // finalize: PDA closed, rent back to payer
    let before = env.banks.get_balance(env.payer.pubkey()).await.unwrap();
    env.finalize(&authority.pubkey(), 1).await.unwrap();
    assert!(env.banks.get_account(env.message(&authority.pubkey(), 1)).await.unwrap().is_none());
    let after = env.banks.get_balance(env.payer.pubkey()).await.unwrap();
    assert!(after > before, "rent refunded to payer (minus tx fee)");

    // nonces are gapless: the next prepare is nonce 2
    env.prepare(&authority, 2, MASK_WORMHOLE).await.unwrap();
    let ss: SenderState = env.account(env.sender_state(&authority.pubkey())).await.unwrap();
    assert_eq!(ss.nonce, 2);
}

#[tokio::test]
async fn dispatch_over_unexpected_transport_is_refused() {
    let mut env = Env::new().await;
    env.initialize().await;
    env.set_wormhole_transport().await;
    let authority = Keypair::new();
    // prepared for LayerZero only
    env.prepare(&authority, 1, MASK_LAYERZERO).await.unwrap();
    let err = env.dispatch_wormhole(&authority.pubkey(), 1).await.unwrap_err();
    assert_eq!(err, custom(BaseCallerError::TransportNotExpected));
}

#[tokio::test]
async fn finalize_before_full_dispatch_is_refused() {
    let mut env = Env::new().await;
    env.initialize().await;
    env.set_wormhole_transport().await;
    let authority = Keypair::new();
    env.prepare(&authority, 1, MASK_WORMHOLE | MASK_LAYERZERO).await.unwrap();
    env.dispatch_wormhole(&authority.pubkey(), 1).await.unwrap();
    // one of two transports done; not expired -> cannot finalize
    let err = env.finalize(&authority.pubkey(), 1).await.unwrap_err();
    assert_eq!(err, custom(BaseCallerError::NotFullyDispatched));
}

#[tokio::test]
async fn prepare_validation() {
    let mut env = Env::new().await;
    env.initialize().await;
    env.set_wormhole_transport().await;
    let authority = Keypair::new();
    // no transports selected
    let err = env.prepare(&authority, 1, 0).await.unwrap_err();
    assert_eq!(err, custom(BaseCallerError::NoTransports));
    // A bit with no transport registered yet is structurally valid -- ids are
    // registered by an admin, not baked into the program.
    env.prepare(&authority, 1, 0b1000_0000).await.unwrap();
}

#[tokio::test]
async fn paused_program_refuses_prepare() {
    let mut env = Env::new().await;
    env.initialize().await;
    env.set_wormhole_transport().await;
    let ix = Instruction {
        program_id: env.program_id,
        accounts: base_caller::accounts::AdminOnly { config: env.config, admin: env.payer.pubkey() }
            .to_account_metas(None),
        data: base_caller::instruction::SetPaused { paused: true }.data(),
    };
    env.send(ix, &[]).await.unwrap();
    let authority = Keypair::new();
    let err = env.prepare(&authority, 1, MASK_WORMHOLE).await.unwrap_err();
    assert_eq!(err, custom(BaseCallerError::Paused));
}

/// The finding that forced this interface: LayerZero's Solana endpoint pins
/// `solana-program = "=1.17.31"` and anchor-lang 0.29, while the Wormhole
/// Anchor SDK needs 1.18 / 0.30.1. `solana-program` can appear only once in a
/// binary, so no single program can dispatch over both. A transport on an
/// incompatible stack therefore ships as its own program and calls back here.
///
/// What matters for the design is that a message dispatched this way is
/// byte-identical to one dispatched in-process, so the destination derives one
/// message id for both and the dual-transport quorum still works.
#[tokio::test]
async fn external_dispatcher_marks_a_transport_without_being_linked_in() {
    let mut env = Env::new().await;
    env.initialize().await;
    env.set_wormhole_transport().await;
    env.set_layerzero_external().await;
    let authority = Keypair::new();

    // One prepared envelope, both transports expected.
    env.prepare(&authority, 1, MASK_WORMHOLE | MASK_LAYERZERO).await.unwrap();
    let before: PreparedMessage = env.account(env.message(&authority.pubkey(), 1)).await.unwrap();
    let envelope = before.envelope.clone();

    // In-process transport.
    env.dispatch_wormhole(&authority.pubkey(), 1).await.unwrap();
    // Out-of-process transport, via a separate program.
    env.external_dispatch(&authority.pubkey(), 1).await.unwrap();

    let after: PreparedMessage = env.account(env.message(&authority.pubkey(), 1)).await.unwrap();
    assert_eq!(after.dispatched, MASK_WORMHOLE | MASK_LAYERZERO);
    assert_eq!(after.envelope, envelope, "the envelope is never re-encoded");

    // Both transports done, so the message can now be closed.
    env.finalize(&authority.pubkey(), 1).await.unwrap();
}

#[tokio::test]
async fn external_dispatch_is_refused_without_the_dispatcher_signature() {
    let mut env = Env::new().await;
    env.initialize().await;
    env.set_wormhole_transport().await;
    let authority = Keypair::new();
    env.prepare(&authority, 1, MASK_LAYERZERO).await.unwrap();

    // LayerZero registered with NO external dispatcher: the callback path is
    // closed even though the transport itself is enabled.
    let ix = Instruction {
        program_id: env.program_id,
        accounts: base_caller::accounts::SetTransport {
            config: env.config,
            transport: env.lz_transport(),
            admin: env.payer.pubkey(),
            system_program: system_program::id(),
        }
        .to_account_metas(None),
        data: base_caller::instruction::SetTransport {
            transport_id: TRANSPORT_LAYERZERO,
            enabled: true,
            program_id: env.mock_wh,
            peer: [0xBB; 32],
            dest_chain: 30184,
            dispatcher: Pubkey::default(),
        }
        .data(),
    };
    env.send(ix, &[]).await.unwrap();

    let err = env.external_dispatch(&authority.pubkey(), 1).await.unwrap_err();
    assert_eq!(err, custom(BaseCallerError::NoDispatcher));
}
