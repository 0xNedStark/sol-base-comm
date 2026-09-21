//! Localnet stand-in for a bridge program (Wormhole core bridge, LayerZero
//! endpoint). Accepts every instruction, including non-Anchor discriminators
//! like Wormhole's single-byte `post_message`, via the fallback handler. Lets
//! `anchor test` exercise prepare -> dispatch -> finalize on a real validator
//! without the real bridge, which has no localnet deployment.

use anchor_lang::prelude::*;

declare_id!("7Qe2izm2v1QMzzpYV9JnZccy7gpARhkg6brRfwJmo2Ya");

#[program]
pub mod mock_transport {
    use super::*;

    /// Anchor requires at least one instruction; never called.
    pub fn noop(_ctx: Context<Noop>) -> Result<()> {
        Ok(())
    }

    /// Act as an EXTERNAL DISPATCHER for a transport whose SDK cannot be
    /// linked into base_caller.
    ///
    /// This is the shape a real out-of-process transport takes. A production
    /// dispatcher would, between reading the envelope and calling back:
    /// forward `message.envelope` verbatim to its own endpoint. This mock
    /// skips that step and only proves the authorisation path and the
    /// bookkeeping callback.
    pub fn dispatch_and_mark(
        ctx: Context<DispatchAndMark>,
        nonce: u64,
        transport_id: u8,
    ) -> Result<()> {
        // A real dispatcher sends these exact bytes to its endpoint here,
        // without re-encoding them -- that is what keeps the message id the
        // same as it would be over any other transport.
        let envelope_len = ctx.accounts.message.envelope.len();
        msg!(
            "mock dispatcher: forwarding {} envelope bytes",
            envelope_len
        );

        let bump = [ctx.bumps.dispatcher_authority];
        let seeds: &[&[u8]] = &[b"dispatcher", &bump];

        base_caller::cpi::mark_dispatched(
            CpiContext::new_with_signer(
                ctx.accounts.base_caller_program.to_account_info(),
                base_caller::cpi::accounts::MarkDispatched {
                    config: ctx.accounts.config.to_account_info(),
                    transport: ctx.accounts.transport.to_account_info(),
                    message: ctx.accounts.message.to_account_info(),
                    authority: ctx.accounts.authority.to_account_info(),
                    dispatcher_authority: ctx.accounts.dispatcher_authority.to_account_info(),
                },
                &[seeds],
            ),
            nonce,
            transport_id,
        )
    }

    /// Everything else -- the real bridge's instruction encodings -- lands here.
    pub fn fallback(_program_id: &Pubkey, accounts: &[AccountInfo], data: &[u8]) -> Result<()> {
        msg!(
            "mock_transport: accepted {} bytes, {} accounts",
            data.len(),
            accounts.len()
        );
        Ok(())
    }
}

#[derive(Accounts)]
pub struct Noop {}

#[derive(Accounts)]
#[instruction(nonce: u64, transport_id: u8)]
pub struct DispatchAndMark<'info> {
    /// CHECK: the base_caller program being called back into.
    pub base_caller_program: UncheckedAccount<'info>,
    /// CHECK: validated by base_caller.
    pub config: UncheckedAccount<'info>,
    /// CHECK: validated by base_caller.
    pub transport: UncheckedAccount<'info>,
    #[account(mut)]
    pub message: Account<'info, base_caller::state::PreparedMessage>,
    /// CHECK: validated by base_caller.
    pub authority: UncheckedAccount<'info>,
    /// CHECK: this program's dispatcher PDA; base_caller checks it matches the
    /// dispatcher registered for the transport.
    #[account(seeds = [b"dispatcher"], bump)]
    pub dispatcher_authority: UncheckedAccount<'info>,
}
