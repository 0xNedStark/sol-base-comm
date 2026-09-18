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

    /// Everything else -- the real bridge's instruction encodings -- lands here.
    pub fn fallback(_program_id: &Pubkey, accounts: &[AccountInfo], data: &[u8]) -> Result<()> {
        msg!("mock_transport: accepted {} bytes, {} accounts", data.len(), accounts.len());
        Ok(())
    }
}

#[derive(Accounts)]
pub struct Noop {}
