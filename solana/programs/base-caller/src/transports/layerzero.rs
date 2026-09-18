//! LayerZero v2 transport.
//!
//! STATUS: intentionally a stub. Unlike Wormhole's core bridge, whose
//! `post_message` is a stable single instruction that is reasonable to encode by
//! hand, the LayerZero send path involves the endpoint's message library
//! resolution, DVN configuration and a dynamic account list that the official
//! Solana OApp SDK derives for you. Hand-rolling it would be guesswork that
//! looks like working code, which is worse than an explicit stub.
//!
//! To complete:
//!   1. Add the LayerZero Solana OApp crate as a dependency.
//!   2. Replace `send` below with the SDK's send CPI, passing `envelope`
//!      unmodified as the message payload.
//!   3. Build options with the SDK's options builder from `gas_limit` (and a
//!      native-drop amount if the envelope carries a non-zero `value`).
//!   4. Add a `quote` instruction wrapping the SDK's quote so clients can size
//!      `native_fee` before sending.
//!   5. Configure the DVN set. Two independent DVNs is the configuration that
//!      makes LayerZero the recommended default; one DVN gives up that
//!      advantage. See docs/03-transport-comparison.md.

use anchor_lang::prelude::*;

use crate::state::TransportConfig;

pub struct LayerZeroAccounts<'info> {
    pub endpoint_program: AccountInfo<'info>,
    pub oapp: AccountInfo<'info>,
    pub payer: AccountInfo<'info>,
    pub system_program: AccountInfo<'info>,
    pub oapp_bump: u8,
}

pub fn send(
    _transport: &Account<TransportConfig>,
    _envelope: &[u8],
    _accounts: &LayerZeroAccounts,
    _native_fee: u64,
    _gas_limit: u64,
) -> Result<()> {
    // See the module docs. Wire this to the official SDK rather than encoding
    // the endpoint CPI by hand.
    unimplemented!("wire to the LayerZero Solana OApp SDK before use")
}
