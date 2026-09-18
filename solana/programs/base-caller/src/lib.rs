//! base_caller -- the Solana half of Solana -> Base contract invocation.
//!
//! It is the only thing that mints outbound envelopes. Callers are either user
//! wallets or, more usefully, other Solana programs that CPI in with
//! `invoke_signed` passing one of their own PDAs as `authority`. The pubkey that
//! lands in `xDomainMessageSender()` on Base is then a key no human holds and
//! that only the calling program's logic can produce -- which is the whole point
//! of doing this on-chain rather than with an off-chain signer.
//!
//! The instruction set is split three ways, and the split is load-bearing:
//!
//!   prepare          build the envelope ONCE, assign the nonce, store the bytes
//!   dispatch_via_*   forward those exact bytes over one transport
//!   finalize         reclaim rent once every expected transport has carried it
//!
//! A design with one send instruction per transport would give the same logical
//! call two nonces on two transports -- two hashes, two message ids -- and the
//! destination's dual-transport quorum would never be met. Building once and
//! dispatching many is what makes byte-identical envelopes possible.
//!
//! Architecture: docs/01-architecture.md
//! Wire format:  docs/02-message-format.md

use anchor_lang::prelude::*;

pub mod envelope;
pub mod errors;
pub mod state;
pub mod transports;

use envelope::{CallParams, HEADER_SIZE, MAX_CALLDATA, MODE_ACCOUNT};
use errors::BaseCallerError;
use state::{Config, PreparedMessage, SenderState, TransportConfig};
use transports::{
    MASK_ALL, MASK_LAYERZERO, MASK_WORMHOLE, TRANSPORT_LAYERZERO, TRANSPORT_WORMHOLE,
};

// Placeholder program id (Anchor's canonical example key). Replace with the
// real deployed id via `anchor keys sync` before any deployment.
declare_id!("Fg6PaFpoGXkYsidMpWTK6W2BeZ7FEfcYkg476zPFsLnS");

/// Floor on the Base-side gas limit. A message that cannot pay for its own
/// dispatch overhead is dead on arrival; rejecting it here is cheaper than
/// discovering it after the transport fee is spent.
pub const MIN_GAS_LIMIT: u64 = 50_000;

#[program]
pub mod base_caller {
    use super::*;

    pub fn initialize(ctx: Context<Initialize>, admin: Pubkey) -> Result<()> {
        let config = &mut ctx.accounts.config;
        config.admin = admin;
        config.pending_admin = Pubkey::default();
        config.paused = false;
        config.bump = ctx.bumps.config;
        Ok(())
    }

    /// Register or update a transport. Admin-gated; hold this with a multisig,
    /// because repointing `peer` is equivalent to replacing the Base side.
    pub fn set_transport(
        ctx: Context<SetTransport>,
        transport_id: u8,
        enabled: bool,
        program_id: Pubkey,
        peer: [u8; 32],
        dest_chain: u32,
    ) -> Result<()> {
        require!(
            ctx.accounts.config.admin == ctx.accounts.admin.key(),
            BaseCallerError::NotAdmin
        );
        let t = &mut ctx.accounts.transport;
        t.transport_id = transport_id;
        t.enabled = enabled;
        t.program_id = program_id;
        t.peer = peer;
        t.dest_chain = dest_chain;
        t.bump = ctx.bumps.transport;
        emit!(TransportUpdated {
            transport_id,
            enabled,
            peer,
            dest_chain
        });
        Ok(())
    }

    pub fn set_paused(ctx: Context<AdminOnly>, paused: bool) -> Result<()> {
        require!(
            ctx.accounts.config.admin == ctx.accounts.admin.key(),
            BaseCallerError::NotAdmin
        );
        ctx.accounts.config.paused = paused;
        Ok(())
    }

    pub fn transfer_admin(ctx: Context<AdminOnly>, new_admin: Pubkey) -> Result<()> {
        require!(
            ctx.accounts.config.admin == ctx.accounts.admin.key(),
            BaseCallerError::NotAdmin
        );
        ctx.accounts.config.pending_admin = new_admin;
        Ok(())
    }

    pub fn accept_admin(ctx: Context<AcceptAdmin>) -> Result<()> {
        let config = &mut ctx.accounts.config;
        require!(
            config.pending_admin == ctx.accounts.new_admin.key(),
            BaseCallerError::NotPendingAdmin
        );
        config.admin = config.pending_admin;
        config.pending_admin = Pubkey::default();
        Ok(())
    }

    // ------------------------------------------------------------- prepare

    /// Build the envelope once and store it for dispatch.
    ///
    /// `transports` is a bitmask of `MASK_*` naming every transport that will
    /// carry this envelope. Choosing them here, rather than at dispatch, makes
    /// a partially dispatched message visible on-chain instead of mysterious.
    ///
    /// The nonce is incremented here, and only here. If this instruction fails
    /// the whole transaction reverts and the increment is rolled back, so
    /// nonces stay gapless -- which matters for any Base-side contract doing
    /// the `lastNonce` ordering check from the Treasury example.
    pub fn prepare(ctx: Context<Prepare>, params: SendParams, transports: u8) -> Result<()> {
        let config = &ctx.accounts.config;
        require!(!config.paused, BaseCallerError::Paused);

        require!(transports != 0, BaseCallerError::NoTransports);
        require!(transports & !MASK_ALL == 0, BaseCallerError::UnknownTransport);

        require!(
            params.calldata.len() <= MAX_CALLDATA,
            BaseCallerError::CalldataTooLarge
        );
        require!(params.mode <= MODE_ACCOUNT, BaseCallerError::InvalidMode);
        require!(params.target != [0u8; 20], BaseCallerError::InvalidTarget);
        require!(
            params.gas_limit >= MIN_GAS_LIMIT,
            BaseCallerError::GasLimitTooLow
        );
        require!(
            params.dst_chain_id != 0 && params.dst_chain_id != envelope::SRC_CHAIN_SOLANA,
            BaseCallerError::InvalidDestination
        );
        // TODO(multi-destination): TransportConfig is keyed by transport only.
        // A second destination needs it keyed by (transport, dst_chain_id) so
        // each destination pins its own adapter peer. Single-destination is
        // correct as is; do this before adding a second gateway.

        if params.expiry != 0 {
            let now = Clock::get()?.unix_timestamp as u64;
            require!(params.expiry > now, BaseCallerError::ExpiryInPast);
        }

        let authority = ctx.accounts.authority.key();
        let sender_state = &mut ctx.accounts.sender_state;
        sender_state.authority = authority;
        sender_state.nonce = sender_state
            .nonce
            .checked_add(1)
            .ok_or(BaseCallerError::NonceOverflow)?;
        let nonce = sender_state.nonce;

        let bytes = envelope::encode(
            &authority.to_bytes(),
            nonce,
            &CallParams {
                dst_chain_id: params.dst_chain_id,
                target: params.target,
                value: params.value,
                gas_limit: params.gas_limit,
                expiry: params.expiry,
                mode: params.mode,
                calldata: params.calldata.clone(),
            },
        )
        .map_err(|_| error!(BaseCallerError::EncodeFailed))?;

        let msg = &mut ctx.accounts.message;
        msg.authority = authority;
        msg.payer = ctx.accounts.payer.key();
        msg.nonce = nonce;
        msg.expiry = params.expiry;
        msg.expected = transports;
        msg.dispatched = 0;
        msg.bump = ctx.bumps.message;
        msg.envelope = bytes;

        emit!(MessagePrepared {
            sender: authority,
            nonce,
            target: params.target,
            transports,
            envelope_len: msg.envelope.len() as u32,
        });
        Ok(())
    }

    // ------------------------------------------------------------ dispatch

    /// Forward a prepared envelope over Wormhole. Permissionless: the bytes are
    /// fixed, so it does not matter who pays to relay them.
    ///
    /// There is one dispatch instruction per transport rather than one that
    /// switches on a transport id, because Anchor account contexts are static:
    /// the Wormhole core bridge and the LayerZero endpoint need different
    /// accounts, and a single context would have to accept the union of both
    /// as optional and validate them by hand.
    pub fn dispatch_via_wormhole(
        ctx: Context<DispatchViaWormhole>,
        nonce: u64,
        batch_nonce: u32,
    ) -> Result<()> {
        require!(!ctx.accounts.config.paused, BaseCallerError::Paused);
        require!(
            ctx.accounts.transport.enabled,
            BaseCallerError::TransportDisabled
        );

        // Scope the mutable borrow: the CPI below needs `ctx.accounts` again.
        let (authority, dispatched, expected) = {
            let msg = &mut ctx.accounts.message;
            mark_dispatch(msg, MASK_WORMHOLE)?;
            (msg.authority, msg.dispatched, msg.expected)
        };

        let wh = ctx
            .accounts
            .to_wormhole_accounts(ctx.bumps.wormhole_emitter);
        transports::wormhole::post_message(
            &ctx.accounts.transport,
            &ctx.accounts.message.envelope,
            &wh,
            batch_nonce,
        )?;

        emit!(CallDispatched {
            transport_id: TRANSPORT_WORMHOLE,
            sender: authority,
            nonce,
            dispatched,
            expected,
        });
        Ok(())
    }

    /// Forward a prepared envelope over LayerZero v2. Permissionless.
    pub fn dispatch_via_layerzero(
        ctx: Context<DispatchViaLayerZero>,
        nonce: u64,
        native_fee: u64,
    ) -> Result<()> {
        require!(!ctx.accounts.config.paused, BaseCallerError::Paused);
        require!(
            ctx.accounts.transport.enabled,
            BaseCallerError::TransportDisabled
        );

        let (authority, dispatched, expected) = {
            let msg = &mut ctx.accounts.message;
            mark_dispatch(msg, MASK_LAYERZERO)?;
            (msg.authority, msg.dispatched, msg.expected)
        };

        // gas_limit lives in the envelope; read it back rather than trusting a
        // caller-supplied duplicate that could disagree with the bytes.
        let envelope = &ctx.accounts.message.envelope;
        let gas_limit = u64::from_be_bytes(envelope[82..90].try_into().unwrap());

        let lz = ctx.accounts.to_layerzero_accounts(ctx.bumps.oapp);
        transports::layerzero::send(
            &ctx.accounts.transport,
            envelope,
            &lz,
            native_fee,
            gas_limit,
        )?;

        emit!(CallDispatched {
            transport_id: TRANSPORT_LAYERZERO,
            sender: authority,
            nonce,
            dispatched,
            expected,
        });
        Ok(())
    }

    // ------------------------------------------------------------ finalize

    /// Close the prepared message and return rent to the original payer.
    ///
    /// Allowed once every expected transport has dispatched, or once the
    /// message has expired (a stuck partial dispatch should not lock rent
    /// forever -- the destination will reject the expired envelope anyway).
    pub fn finalize(ctx: Context<Finalize>, nonce: u64) -> Result<()> {
        let msg = &ctx.accounts.message;
        let fully_dispatched = msg.dispatched == msg.expected;
        let expired = msg.expiry != 0 && (Clock::get()?.unix_timestamp as u64) > msg.expiry;
        require!(
            fully_dispatched || expired,
            BaseCallerError::NotFullyDispatched
        );

        emit!(MessageFinalized {
            sender: msg.authority,
            nonce,
            dispatched: msg.dispatched,
            expected: msg.expected
        });
        Ok(())
    }
}

/// Shared dispatch bookkeeping. Refuses an unexpected or repeated transport
/// and an expired message, then records the bit.
fn mark_dispatch(msg: &mut Account<PreparedMessage>, mask: u8) -> Result<()> {
    require!(msg.expected & mask != 0, BaseCallerError::TransportNotExpected);
    require!(msg.dispatched & mask == 0, BaseCallerError::AlreadyDispatched);
    if msg.expiry != 0 {
        let now = Clock::get()?.unix_timestamp as u64;
        require!(now <= msg.expiry, BaseCallerError::PreparedExpired);
    }
    msg.dispatched |= mask;
    Ok(())
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct SendParams {
    /// Internal id of the destination chain (see envelope::CHAIN_*). Baked into
    /// the envelope so a gateway on any other chain rejects it.
    pub dst_chain_id: u16,
    /// Contract to call on the destination.
    pub target: [u8; 20],
    /// Wei of ETH to attach on Base, drawn from this sender's gateway balance.
    pub value: u128,
    pub gas_limit: u64,
    /// Unix seconds; 0 = never expires. Set it for anything price-sensitive.
    pub expiry: u64,
    /// 0 = DIRECT (gateway calls the target), 1 = ACCOUNT (call routes through
    /// this sender's deterministic Base smart account).
    pub mode: u8,
    /// ABI-encoded call, selector first.
    pub calldata: Vec<u8>,
}

#[event]
pub struct MessagePrepared {
    pub sender: Pubkey,
    pub nonce: u64,
    pub target: [u8; 20],
    pub transports: u8,
    pub envelope_len: u32,
}

#[event]
pub struct CallDispatched {
    pub transport_id: u8,
    pub sender: Pubkey,
    pub nonce: u64,
    pub dispatched: u8,
    pub expected: u8,
}

#[event]
pub struct MessageFinalized {
    pub sender: Pubkey,
    pub nonce: u64,
    pub dispatched: u8,
    pub expected: u8,
}

#[event]
pub struct TransportUpdated {
    pub transport_id: u8,
    pub enabled: bool,
    pub peer: [u8; 32],
    pub dest_chain: u32,
}

// ------------------------------------------------------------------ contexts

#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(init, payer = payer, space = Config::LEN, seeds = [Config::SEED], bump)]
    pub config: Account<'info, Config>,
    #[account(mut)]
    pub payer: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(transport_id: u8)]
pub struct SetTransport<'info> {
    #[account(seeds = [Config::SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    #[account(
        init_if_needed,
        payer = admin,
        space = TransportConfig::LEN,
        seeds = [TransportConfig::SEED, &[transport_id]],
        bump
    )]
    pub transport: Account<'info, TransportConfig>,
    #[account(mut)]
    pub admin: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct AdminOnly<'info> {
    #[account(mut, seeds = [Config::SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    pub admin: Signer<'info>,
}

#[derive(Accounts)]
pub struct AcceptAdmin<'info> {
    #[account(mut, seeds = [Config::SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    pub new_admin: Signer<'info>,
}

/// `authority` is whatever identity should appear on Base. A user wallet signs
/// directly; a calling program passes its own PDA and signs with
/// `invoke_signed`, which is the intended usage.
///
/// The message PDA is keyed by (authority, next nonce). The nonce is read from
/// `sender_state` before the increment, so the seed is `nonce + 1`.
#[derive(Accounts)]
#[instruction(params: SendParams)]
pub struct Prepare<'info> {
    #[account(seeds = [Config::SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    #[account(
        init_if_needed,
        payer = payer,
        space = SenderState::LEN,
        seeds = [SenderState::SEED, authority.key().as_ref()],
        bump
    )]
    pub sender_state: Account<'info, SenderState>,
    #[account(
        init,
        payer = payer,
        space = PreparedMessage::space(HEADER_SIZE + params.calldata.len()),
        seeds = [
            PreparedMessage::SEED,
            authority.key().as_ref(),
            &(sender_state.nonce + 1).to_le_bytes()
        ],
        bump
    )]
    pub message: Account<'info, PreparedMessage>,

    pub authority: Signer<'info>,
    #[account(mut)]
    pub payer: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(nonce: u64)]
pub struct DispatchViaWormhole<'info> {
    #[account(seeds = [Config::SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    #[account(
        seeds = [TransportConfig::SEED, &[TRANSPORT_WORMHOLE]],
        bump = transport.bump
    )]
    pub transport: Account<'info, TransportConfig>,
    #[account(
        mut,
        seeds = [PreparedMessage::SEED, authority.key().as_ref(), &nonce.to_le_bytes()],
        bump = message.bump,
        has_one = authority
    )]
    pub message: Account<'info, PreparedMessage>,
    /// CHECK: only used to derive the message PDA; `has_one` pins it.
    pub authority: UncheckedAccount<'info>,

    /// Pays the transport fee. Need not be the authority -- dispatch is
    /// permissionless because the bytes are already fixed.
    #[account(mut)]
    pub payer: Signer<'info>,

    // --- Wormhole core bridge accounts. Verify the exact set and ordering
    // --- against the deployed core bridge before mainnet.
    /// CHECK: validated by the core bridge
    #[account(mut)]
    pub wormhole_bridge: UncheckedAccount<'info>,
    /// CHECK: message account, created by this instruction
    #[account(mut)]
    pub wormhole_message: Signer<'info>,
    /// CHECK: PDA of this program, seeds ["emitter"] -- this is the identity the
    /// Base-side WormholeAdapter pins as `solanaPeer`.
    #[account(seeds = [b"emitter"], bump)]
    pub wormhole_emitter: UncheckedAccount<'info>,
    /// CHECK: validated by the core bridge
    #[account(mut)]
    pub wormhole_sequence: UncheckedAccount<'info>,
    /// CHECK: validated by the core bridge
    #[account(mut)]
    pub wormhole_fee_collector: UncheckedAccount<'info>,
    /// CHECK: address checked against transport.program_id
    #[account(address = transport.program_id)]
    pub wormhole_program: UncheckedAccount<'info>,

    pub clock: Sysvar<'info, Clock>,
    pub rent: Sysvar<'info, Rent>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(nonce: u64)]
pub struct DispatchViaLayerZero<'info> {
    #[account(seeds = [Config::SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    #[account(
        seeds = [TransportConfig::SEED, &[TRANSPORT_LAYERZERO]],
        bump = transport.bump
    )]
    pub transport: Account<'info, TransportConfig>,
    #[account(
        mut,
        seeds = [PreparedMessage::SEED, authority.key().as_ref(), &nonce.to_le_bytes()],
        bump = message.bump,
        has_one = authority
    )]
    pub message: Account<'info, PreparedMessage>,
    /// CHECK: only used to derive the message PDA; `has_one` pins it.
    pub authority: UncheckedAccount<'info>,

    #[account(mut)]
    pub payer: Signer<'info>,

    // --- LayerZero endpoint accounts. The real set comes from the official
    // --- Solana OApp SDK; wire it from there rather than by hand.
    /// CHECK: address checked against transport.program_id
    #[account(address = transport.program_id)]
    pub endpoint_program: UncheckedAccount<'info>,
    /// CHECK: this program's OApp PDA -- the identity LayerZeroAdapter pins.
    #[account(seeds = [b"oapp"], bump)]
    pub oapp: UncheckedAccount<'info>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(nonce: u64)]
pub struct Finalize<'info> {
    #[account(
        mut,
        close = payer,
        seeds = [PreparedMessage::SEED, authority.key().as_ref(), &nonce.to_le_bytes()],
        bump = message.bump,
        has_one = authority,
        has_one = payer
    )]
    pub message: Account<'info, PreparedMessage>,
    /// CHECK: only used to derive the message PDA; `has_one` pins it.
    pub authority: UncheckedAccount<'info>,
    /// CHECK: receives the rent back; `has_one` pins it to the original payer.
    #[account(mut)]
    pub payer: UncheckedAccount<'info>,
}

// ------------------------------------------------- context -> transport views

impl<'info> DispatchViaWormhole<'info> {
    fn to_wormhole_accounts(
        &self,
        emitter_bump: u8,
    ) -> transports::wormhole::WormholeAccounts<'info> {
        transports::wormhole::WormholeAccounts {
            bridge: self.wormhole_bridge.to_account_info(),
            message: self.wormhole_message.to_account_info(),
            emitter: self.wormhole_emitter.to_account_info(),
            sequence: self.wormhole_sequence.to_account_info(),
            fee_collector: self.wormhole_fee_collector.to_account_info(),
            program: self.wormhole_program.to_account_info(),
            payer: self.payer.to_account_info(),
            clock: self.clock.to_account_info(),
            rent: self.rent.to_account_info(),
            system_program: self.system_program.to_account_info(),
            emitter_bump,
        }
    }
}

impl<'info> DispatchViaLayerZero<'info> {
    fn to_layerzero_accounts(
        &self,
        oapp_bump: u8,
    ) -> transports::layerzero::LayerZeroAccounts<'info> {
        transports::layerzero::LayerZeroAccounts {
            endpoint_program: self.endpoint_program.to_account_info(),
            oapp: self.oapp.to_account_info(),
            payer: self.payer.to_account_info(),
            system_program: self.system_program.to_account_info(),
            oapp_bump,
        }
    }
}
