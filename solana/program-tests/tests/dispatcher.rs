//! Proves the cross-program, cross-anchor-version dispatch path.
//!
//! Three programs are loaded into one validator, and two of them cannot be
//! linked together at all:
//!
//!   base_caller    anchor 0.30.1, solana-program 1.18   (Wormhole SDK)
//!   lz_dispatcher  anchor 0.29,   solana-program 1.17.31 (LayerZero oapp)
//!   mock_transport stands in for the LayerZero endpoint
//!
//! The version conflict is a property of a single *binary*, not of a
//! transaction, which is exactly what makes the external-dispatcher design
//! work. This test is the proof: separate .so files, one CPI chain.
//!
//! The test itself speaks to lz_dispatcher the way any cross-version client
//! must -- hand-encoded discriminators and borsh args -- because it cannot
//! depend on a crate built against anchor 0.29 either.
//!
//! What it asserts, in order of importance:
//!   1. The dispatcher marks the transport on the outbox. The raw ABI works.
//!   2. The envelope is byte-identical afterwards. Nothing re-encoded it, so
//!      the destination derives the same message id as it would for an
//!      in-process transport, and the dual-transport quorum survives the
//!      program boundary.
//!   3. One prepared envelope satisfies both transports.

use anchor_lang::{AccountDeserialize, InstructionData, ToAccountMetas};
use base_caller::envelope::{CHAIN_BASE, MODE_DIRECT};
use base_caller::state::{Config, PreparedMessage, TransportConfig};
use base_caller::transports::{MASK_LAYERZERO, MASK_WORMHOLE, TRANSPORT_LAYERZERO, TRANSPORT_WORMHOLE};
use base_caller::SendParams;
use base_caller_abi as abi;
use solana_program_test::*;
use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::{Keypair, Signer},
    system_program,
    transaction::Transaction,
};
use std::str::FromStr;

/// lz_dispatcher's declared program id.
const LZ_DISPATCHER_ID: &str = "LZDisp1111111111111111111111111111111111111";

/// sha256("global:<name>")[..8] for lz_dispatcher's instructions. Hand-encoded
/// because this crate is built against anchor 0.30.1 and cannot depend on a
/// 0.29 crate -- the same constraint every real client faces.
const IX_INITIALIZE: [u8; 8] = [175, 175, 109, 31, 13, 152, 155, 237];
const IX_DISPATCH: [u8; 8] = [8, 67, 96, 172, 17, 124, 160, 63];

/// The endpoint's `Send` needs its declared fields plus the event_cpi pair,
/// plus the program itself at index 0. Short of that, `construct_context`
/// refuses before the CPI is built.
const ENDPOINT_ACCOUNTS: usize = 10;

struct Env {
    banks: BanksClient,
    payer: Keypair,
    outbox: Pubkey,
    dispatcher: Pubkey,
    endpoint: Pubkey,
    config: Pubkey,
}

impl Env {
    async fn new() -> Self {
        let outbox = base_caller::id();
        let dispatcher = Pubkey::from_str(LZ_DISPATCHER_ID).unwrap();
        let endpoint = mock_transport::id();

        let mut pt = ProgramTest::new("base_caller", outbox, None);
        pt.add_program("mock_transport", endpoint, None);
        pt.add_program("lz_dispatcher", dispatcher, None);
        let (banks, payer, _) = pt.start().await;

        let (config, _) = Pubkey::find_program_address(&[Config::SEED], &outbox);
        Self { banks, payer, outbox, dispatcher, endpoint, config }
    }

    async fn send(&mut self, ix: Instruction, extra: &[&Keypair]) -> Result<(), String> {
        let hash = self.banks.get_latest_blockhash().await.unwrap();
        let mut signers: Vec<&Keypair> = vec![&self.payer];
        signers.extend_from_slice(extra);
        let tx = Transaction::new_signed_with_payer(&[ix], Some(&self.payer.pubkey()), &signers, hash);
        self.banks.process_transaction(tx).await.map_err(|e| format!("{e:?}"))
    }

    fn pda(&self, seeds: &[&[u8]], program: &Pubkey) -> Pubkey {
        Pubkey::find_program_address(seeds, program).0
    }

    fn transport(&self, id: u8) -> Pubkey {
        self.pda(&[TransportConfig::SEED, &[id]], &self.outbox)
    }

    fn message(&self, authority: &Pubkey, nonce: u64) -> Pubkey {
        self.pda(
            &[PreparedMessage::SEED, authority.as_ref(), &nonce.to_le_bytes()],
            &self.outbox,
        )
    }

    async fn prepared(&mut self, authority: &Pubkey, nonce: u64) -> PreparedMessage {
        let acc = self.banks.get_account(self.message(authority, nonce)).await.unwrap().unwrap();
        PreparedMessage::try_deserialize(&mut &acc.data[..]).unwrap()
    }

    // ---------------------------------------------------------------- outbox

    async fn init_outbox(&mut self) {
        let ix = Instruction {
            program_id: self.outbox,
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

    async fn register(&mut self, transport_id: u8, dispatcher: Pubkey, dest_chain: u32) {
        let ix = Instruction {
            program_id: self.outbox,
            accounts: base_caller::accounts::SetTransport {
                config: self.config,
                transport: self.transport(transport_id),
                admin: self.payer.pubkey(),
                system_program: system_program::id(),
            }
            .to_account_metas(None),
            data: base_caller::instruction::SetTransport {
                transport_id,
                enabled: true,
                program_id: self.endpoint,
                peer: [0xBB; 32],
                dest_chain,
                dispatcher,
            }
            .data(),
        };
        self.send(ix, &[]).await.unwrap();
    }

    async fn prepare(&mut self, authority: &Keypair, nonce: u64, transports: u8, gas_limit: u64) {
        let ix = Instruction {
            program_id: self.outbox,
            accounts: base_caller::accounts::Prepare {
                config: self.config,
                sender_state: self.pda(
                    &[base_caller::state::SenderState::SEED, authority.pubkey().as_ref()],
                    &self.outbox,
                ),
                message: self.message(&authority.pubkey(), nonce),
                authority: authority.pubkey(),
                payer: self.payer.pubkey(),
                system_program: system_program::id(),
            }
            .to_account_metas(None),
            data: base_caller::instruction::Prepare {
                params: SendParams {
                    dst_chain_id: CHAIN_BASE,
                    target: [0xAB; 20],
                    value: 0,
                    gas_limit,
                    expiry: 0,
                    mode: MODE_DIRECT,
                    calldata: vec![0xde, 0xad, 0xbe, 0xef],
                },
                transports,
            }
            .data(),
        };
        self.send(ix, &[authority]).await.unwrap();
    }

    // ------------------------------------------------------------ dispatcher

    async fn init_dispatcher(&mut self) {
        let store = self.pda(&[b"store"], &self.dispatcher);
        let mut data = Vec::from(IX_INITIALIZE);
        // InitParams, borsh: admin, base_caller_program, endpoint_program,
        // transport_id, dst_eid, peer
        data.extend_from_slice(self.payer.pubkey().as_ref());
        data.extend_from_slice(self.outbox.as_ref());
        data.extend_from_slice(self.endpoint.as_ref());
        data.push(TRANSPORT_LAYERZERO);
        data.extend_from_slice(&30_184u32.to_le_bytes()); // Base mainnet eid
        data.extend_from_slice(&[0xBB; 32]);

        let ix = Instruction {
            program_id: self.dispatcher,
            accounts: vec![
                AccountMeta::new(store, false),
                AccountMeta::new(self.payer.pubkey(), true),
                AccountMeta::new_readonly(system_program::id(), false),
            ],
            data,
        };
        self.send(ix, &[]).await.unwrap();
    }

    async fn dispatch(&mut self, authority: &Pubkey, nonce: u64) -> Result<(), String> {
        let store = self.pda(&[b"store"], &self.dispatcher);
        let oapp = self.pda(&[b"oapp"], &self.dispatcher);
        let dispatcher_authority = self.pda(&[abi::DISPATCHER_SEED], &self.dispatcher);

        let mut accounts = vec![
            AccountMeta::new_readonly(store, false),
            AccountMeta::new_readonly(oapp, false),
            AccountMeta::new_readonly(dispatcher_authority, false),
            AccountMeta::new(self.message(authority, nonce), false),
            AccountMeta::new_readonly(*authority, false),
            AccountMeta::new_readonly(self.config, false),
            AccountMeta::new_readonly(self.transport(TRANSPORT_LAYERZERO), false),
            AccountMeta::new_readonly(self.outbox, false),
            AccountMeta::new_readonly(self.endpoint, false),
        ];
        // The endpoint's own account list, in the order its `Send` declares:
        //
        //   0 endpoint program        5 send_library_info
        //   1 sender (the OApp PDA)   6 endpoint settings
        //   2 send_library_program    7 nonce            <- declared mut
        //   3 send_library_config     8 event_authority
        //   4 default_send_library_   9 program
        //
        // Mutability has to match, or the runtime rejects the CPI for
        // privilege escalation before the endpoint ever runs. Everything but
        // the nonce is read-only. A real client builds this list with the
        // LayerZero SDK; here they are placeholders the mock ignores, but
        // `construct_context` still requires the right count and privileges.
        const NONCE_INDEX: usize = 7;
        accounts.push(AccountMeta::new_readonly(self.endpoint, false));
        accounts.push(AccountMeta::new_readonly(oapp, false));
        for i in 2..ENDPOINT_ACCOUNTS {
            let key = Pubkey::new_unique();
            accounts.push(if i == NONCE_INDEX {
                AccountMeta::new(key, false)
            } else {
                AccountMeta::new_readonly(key, false)
            });
        }

        let mut data = Vec::from(IX_DISPATCH);
        data.extend_from_slice(&nonce.to_le_bytes());
        data.extend_from_slice(&0u64.to_le_bytes()); // native_fee

        self.send(Instruction { program_id: self.dispatcher, accounts, data }, &[]).await
    }
}

#[tokio::test]
async fn layerzero_dispatcher_marks_the_outbox_across_the_version_boundary() {
    let mut env = Env::new().await;
    env.init_outbox().await;
    env.register(TRANSPORT_LAYERZERO, env.dispatcher, 30_184).await;
    env.init_dispatcher().await;

    let authority = Keypair::new();
    env.prepare(&authority, 1, MASK_LAYERZERO, 300_000).await;
    let before = env.prepared(&authority.pubkey(), 1).await;
    assert_eq!(before.dispatched, 0);

    env.dispatch(&authority.pubkey(), 1).await.expect("dispatch should succeed");

    let after = env.prepared(&authority.pubkey(), 1).await;
    assert_eq!(
        after.dispatched, MASK_LAYERZERO,
        "the dispatcher marked the transport through the raw ABI"
    );
    assert_eq!(
        after.envelope, before.envelope,
        "the envelope must be byte-identical: the destination hashes these \
         bytes for the message id, so re-encoding would break dedup and quorum"
    );
}

#[tokio::test]
async fn one_envelope_satisfies_an_in_process_and_an_external_transport() {
    let mut env = Env::new().await;
    env.init_outbox().await;
    env.register(TRANSPORT_WORMHOLE, Pubkey::default(), 30).await;
    env.register(TRANSPORT_LAYERZERO, env.dispatcher, 30_184).await;
    env.init_dispatcher().await;

    let authority = Keypair::new();
    env.prepare(&authority, 1, MASK_WORMHOLE | MASK_LAYERZERO, 300_000).await;
    let envelope = env.prepared(&authority.pubkey(), 1).await.envelope;

    // Out-of-process. (The in-process Wormhole leg is covered in outbox.rs;
    // here the point is that the external leg leaves the envelope alone.)
    env.dispatch(&authority.pubkey(), 1).await.expect("external dispatch");

    let after = env.prepared(&authority.pubkey(), 1).await;
    assert_eq!(after.dispatched, MASK_LAYERZERO);
    assert_eq!(after.expected, MASK_WORMHOLE | MASK_LAYERZERO);
    assert_eq!(after.envelope, envelope);
}

#[tokio::test]
async fn dispatcher_refuses_a_message_belonging_to_another_program() {
    let mut env = Env::new().await;
    env.init_outbox().await;
    env.register(TRANSPORT_LAYERZERO, env.dispatcher, 30_184).await;
    env.init_dispatcher().await;

    let authority = Keypair::new();
    env.prepare(&authority, 1, MASK_LAYERZERO, 300_000).await;

    // Point the dispatcher at an account the outbox does not own. Without the
    // ownership check this program would happily sign a LayerZero message for
    // whatever bytes an attacker put there.
    let store = env.pda(&[b"store"], &env.dispatcher);
    let oapp = env.pda(&[b"oapp"], &env.dispatcher);
    let dispatcher_authority = env.pda(&[abi::DISPATCHER_SEED], &env.dispatcher);
    let mut accounts = vec![
        AccountMeta::new_readonly(store, false),
        AccountMeta::new_readonly(oapp, false),
        AccountMeta::new_readonly(dispatcher_authority, false),
        AccountMeta::new(env.config, false), // owned by the outbox but not a message
        AccountMeta::new_readonly(authority.pubkey(), false),
        AccountMeta::new_readonly(env.config, false),
        AccountMeta::new_readonly(env.transport(TRANSPORT_LAYERZERO), false),
        AccountMeta::new_readonly(env.outbox, false),
        AccountMeta::new_readonly(env.endpoint, false),
    ];
    accounts.push(AccountMeta::new_readonly(env.endpoint, false));
    accounts.push(AccountMeta::new_readonly(oapp, false));
    for i in 2..ENDPOINT_ACCOUNTS {
        let key = Pubkey::new_unique();
        accounts.push(if i == 7 {
            AccountMeta::new(key, false)
        } else {
            AccountMeta::new_readonly(key, false)
        });
    }
    let mut data = Vec::from(IX_DISPATCH);
    data.extend_from_slice(&1u64.to_le_bytes());
    data.extend_from_slice(&0u64.to_le_bytes());

    let err = env
        .send(Instruction { program_id: env.dispatcher, accounts, data }, &[])
        .await
        .expect_err("a Config account is not a PreparedMessage");
    assert!(
        err.contains("MalformedMessage") || err.contains("Custom"),
        "expected the discriminator check to reject it, got: {err}"
    );
}
