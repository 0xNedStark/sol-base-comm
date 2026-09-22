//! LayerZero dispatcher for the `base_caller` outbox.
//!
//! # Why this is a separate program
//!
//! LayerZero's Solana endpoint pins `solana-program = "=1.17.31"` and
//! anchor-lang 0.29. The outbox uses the Wormhole Anchor SDK, which needs 1.18
//! and 0.30.1. `solana-program` can appear only once in a binary, so cargo
//! refuses the combination at resolution time -- no single program can
//! dispatch over both transports.
//!
//! The outbox's prepare/dispatch split is what makes that survivable. The
//! envelope is built once into a `PreparedMessage` PDA; this program reads
//! those bytes, forwards them **unmodified** to the LayerZero endpoint, and
//! calls `mark_dispatched` back on the outbox to record that it did.
//!
//! Because the envelope is never re-derived, a message sent this way is
//! byte-identical to the same message sent over an in-process transport. The
//! destination hashes those bytes for the message id, so both transports
//! produce one id and the dual-transport quorum still works across the program
//! boundary.
//!
//! # How it talks to the outbox
//!
//! It cannot link the outbox -- same version conflict -- so it speaks raw
//! bytes through `base-caller-abi`, a crate with no dependencies at all. That
//! crate holds the instruction discriminator and the `PreparedMessage`
//! offsets, and the outbox's own test suite asserts the real Anchor-derived
//! values still match every constant in it. Rename an instruction or reorder a
//! field and that test fails, rather than this program silently reading the
//! wrong bytes against a live bridge.

use anchor_lang::prelude::*;
use anchor_lang::solana_program::{
    instruction::{AccountMeta, Instruction},
    program::invoke_signed,
};
use base_caller_abi as abi;
use oapp::endpoint::instructions::SendParams as EndpointSendParams;

pub mod options;

declare_id!("LZDisp1111111111111111111111111111111111111");

/// Seed of the PDA that is this dispatcher's LayerZero identity. Registered
/// with the endpoint as the OApp, and pinned by `LayerZeroAdapter.solanaPeer`
/// on the destination chain.
pub const OAPP_SEED: &[u8] = b"oapp";

/// Seed of the PDA that signs `mark_dispatched` on the outbox. The outbox
/// stores this program's id in `TransportConfig.dispatcher` and derives the
/// same address, so only this program can mark its transport.
pub const DISPATCHER_SEED: &[u8] = abi::DISPATCHER_SEED;

pub const STORE_SEED: &[u8] = b"store";

#[program]
pub mod lz_dispatcher {
    use super::*;

    pub fn initialize(ctx: Context<Initialize>, params: InitParams) -> Result<()> {
        let store = &mut ctx.accounts.store;
        store.admin = params.admin;
        store.base_caller_program = params.base_caller_program;
        store.endpoint_program = params.endpoint_program;
        store.transport_id = params.transport_id;
        store.dst_eid = params.dst_eid;
        store.peer = params.peer;
        store.bump = ctx.bumps.store;
        Ok(())
    }

    pub fn set_peer(ctx: Context<AdminOnly>, dst_eid: u32, peer: [u8; 32]) -> Result<()> {
        let store = &mut ctx.accounts.store;
        require!(store.admin == ctx.accounts.admin.key(), DispatcherError::NotAdmin);
        store.dst_eid = dst_eid;
        store.peer = peer;
        emit!(PeerSet { dst_eid, peer });
        Ok(())
    }

    /// Send a prepared envelope over LayerZero, then record the dispatch.
    ///
    /// Permissionless: the envelope's bytes are fixed by the time it gets
    /// here, so it does not matter who pays to relay them.
    ///
    /// `ctx.remaining_accounts` is the endpoint's own account list, in the
    /// order its `Send` instruction expects -- element 0 the endpoint program,
    /// element 1 this program's OApp PDA, then the rest. The client builds it
    /// with the LayerZero SDK; `oapp::endpoint_cpi::send` slices and validates
    /// it. Element 1 is checked against our OApp PDA before the call so a
    /// mismatch fails here rather than deep inside the endpoint.
    pub fn dispatch(ctx: Context<Dispatch>, nonce: u64, native_fee: u64) -> Result<()> {
        let store = &ctx.accounts.store;

        // The account must genuinely belong to the outbox. Without this an
        // attacker could pass a look-alike account they control and have this
        // program sign a LayerZero message for arbitrary bytes.
        require_keys_eq!(
            *ctx.accounts.prepared_message.owner,
            store.base_caller_program,
            DispatcherError::ForeignMessage
        );

        let (envelope, msg_nonce) = {
            let data = ctx.accounts.prepared_message.try_borrow_data()?;
            let envelope = abi::envelope_of(&data)
                .ok_or(DispatcherError::MalformedMessage)?
                .to_vec();
            let msg_nonce = abi::nonce_of(&data).ok_or(DispatcherError::MalformedMessage)?;
            (envelope, msg_nonce)
        };
        require_eq!(msg_nonce, nonce, DispatcherError::NonceMismatch);

        // Gas the executor is paid for must match what the envelope promises.
        let gas_limit = envelope_gas_limit(&envelope).ok_or(DispatcherError::MalformedMessage)?;

        let (oapp, oapp_bump) = Pubkey::find_program_address(&[OAPP_SEED], ctx.program_id);
        require_keys_eq!(ctx.accounts.oapp.key(), oapp, DispatcherError::BadOapp);
        require!(
            ctx.remaining_accounts.len() > 1,
            DispatcherError::MissingEndpointAccounts
        );
        require_keys_eq!(
            ctx.remaining_accounts[1].key(),
            oapp,
            DispatcherError::BadOapp
        );

        // The envelope goes over the wire exactly as the outbox built it.
        // Re-encoding it here would change the destination's message id and
        // silently break deduplication and the dual-transport quorum.
        oapp::endpoint_cpi::send(
            store.endpoint_program,
            oapp,
            ctx.remaining_accounts,
            &[OAPP_SEED, &[oapp_bump]],
            EndpointSendParams {
                dst_eid: store.dst_eid,
                receiver: store.peer,
                message: envelope,
                options: options::executor_lz_receive(gas_limit),
                native_fee,
                lz_token_fee: 0,
            },
        )?;

        mark_dispatched(&ctx, nonce, store.transport_id)?;

        emit!(Dispatched {
            nonce,
            dst_eid: store.dst_eid,
            gas_limit,
        });
        Ok(())
    }
}

/// `gasLimit` lives at offset 82 of the envelope, big-endian, as specified in
/// docs/02-message-format.md.
fn envelope_gas_limit(envelope: &[u8]) -> Option<u64> {
    let b = envelope.get(82..90)?;
    Some(u64::from_be_bytes([
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
    ]))
}

/// Call the outbox's `mark_dispatched`, built by hand because its generated
/// CPI helpers are compiled against an anchor-lang this program cannot link.
/// The account order is `MarkDispatched`'s field order; the discriminator and
/// the argument encoding come from `base-caller-abi`.
fn mark_dispatched(ctx: &Context<Dispatch>, nonce: u64, transport_id: u8) -> Result<()> {
    let (dispatcher_authority, bump) =
        Pubkey::find_program_address(&[DISPATCHER_SEED], ctx.program_id);
    require_keys_eq!(
        ctx.accounts.dispatcher_authority.key(),
        dispatcher_authority,
        DispatcherError::BadDispatcherAuthority
    );

    let mut data = Vec::with_capacity(8 + 8 + 1);
    data.extend_from_slice(&abi::MARK_DISPATCHED_IX);
    data.extend_from_slice(&nonce.to_le_bytes()); // borsh u64
    data.push(transport_id); // borsh u8

    let ix = Instruction {
        program_id: ctx.accounts.store.base_caller_program,
        accounts: vec![
            AccountMeta::new_readonly(ctx.accounts.outbox_config.key(), false),
            AccountMeta::new_readonly(ctx.accounts.outbox_transport.key(), false),
            AccountMeta::new(ctx.accounts.prepared_message.key(), false),
            AccountMeta::new_readonly(ctx.accounts.authority.key(), false),
            AccountMeta::new_readonly(dispatcher_authority, true),
        ],
        data,
    };

    invoke_signed(
        &ix,
        &[
            ctx.accounts.outbox_config.to_account_info(),
            ctx.accounts.outbox_transport.to_account_info(),
            ctx.accounts.prepared_message.to_account_info(),
            ctx.accounts.authority.to_account_info(),
            ctx.accounts.dispatcher_authority.to_account_info(),
            ctx.accounts.base_caller_program.to_account_info(),
        ],
        &[&[DISPATCHER_SEED, &[bump]]],
    )
    .map_err(Into::into)
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct InitParams {
    pub admin: Pubkey,
    pub base_caller_program: Pubkey,
    pub endpoint_program: Pubkey,
    /// Must match the id this dispatcher is registered under in the outbox's
    /// `TransportConfig`.
    pub transport_id: u8,
    pub dst_eid: u32,
    /// The destination-chain adapter, left-padded to 32 bytes.
    pub peer: [u8; 32],
}

#[account]
pub struct Store {
    pub admin: Pubkey,
    pub base_caller_program: Pubkey,
    pub endpoint_program: Pubkey,
    pub transport_id: u8,
    pub dst_eid: u32,
    pub peer: [u8; 32],
    pub bump: u8,
}

impl Store {
    pub const LEN: usize = 8 + 32 + 32 + 32 + 1 + 4 + 32 + 1;
}

#[event]
pub struct Dispatched {
    pub nonce: u64,
    pub dst_eid: u32,
    pub gas_limit: u64,
}

#[event]
pub struct PeerSet {
    pub dst_eid: u32,
    pub peer: [u8; 32],
}

#[error_code]
pub enum DispatcherError {
    #[msg("Not the admin")]
    NotAdmin,
    #[msg("Prepared message is not owned by the configured outbox program")]
    ForeignMessage,
    #[msg("Prepared message account is malformed")]
    MalformedMessage,
    #[msg("Nonce argument does not match the prepared message")]
    NonceMismatch,
    #[msg("OApp account is not this program's OApp PDA")]
    BadOapp,
    #[msg("Dispatcher authority is not this program's dispatcher PDA")]
    BadDispatcherAuthority,
    #[msg("Endpoint account list is too short")]
    MissingEndpointAccounts,
}

#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(init, payer = payer, space = Store::LEN, seeds = [STORE_SEED], bump)]
    pub store: Account<'info, Store>,
    #[account(mut)]
    pub payer: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct AdminOnly<'info> {
    #[account(mut, seeds = [STORE_SEED], bump = store.bump)]
    pub store: Account<'info, Store>,
    pub admin: Signer<'info>,
}

#[derive(Accounts)]
#[instruction(nonce: u64)]
pub struct Dispatch<'info> {
    #[account(seeds = [STORE_SEED], bump = store.bump)]
    pub store: Account<'info, Store>,

    /// CHECK: this program's LayerZero identity; checked against the derived
    /// PDA in the handler and passed to the endpoint as the sender.
    #[account(seeds = [OAPP_SEED], bump)]
    pub oapp: UncheckedAccount<'info>,

    /// CHECK: this program's outbox-facing identity; checked against the
    /// derived PDA in the handler and signed for with `invoke_signed`.
    #[account(seeds = [DISPATCHER_SEED], bump)]
    pub dispatcher_authority: UncheckedAccount<'info>,

    /// CHECK: owned by the outbox, parsed through base-caller-abi. Ownership
    /// and discriminator are both verified in the handler.
    #[account(mut)]
    pub prepared_message: UncheckedAccount<'info>,

    /// CHECK: the envelope's authority; the outbox pins it with `has_one`.
    pub authority: UncheckedAccount<'info>,

    /// CHECK: validated by the outbox.
    pub outbox_config: UncheckedAccount<'info>,
    /// CHECK: validated by the outbox.
    pub outbox_transport: UncheckedAccount<'info>,
    /// CHECK: must be the outbox this dispatcher was initialized against.
    #[account(address = store.base_caller_program)]
    pub base_caller_program: UncheckedAccount<'info>,

    /// CHECK: must be the endpoint this dispatcher was initialized against.
    /// The endpoint's own account list follows in `remaining_accounts`.
    #[account(address = store.endpoint_program)]
    pub endpoint_program: UncheckedAccount<'info>,
}
