//! LayerZero v2 transport.
//!
//! STATUS: cannot be dispatched from inside this program, and not for want of
//! effort. LayerZero's Solana endpoint pins `solana-program = "=1.17.31"` and
//! anchor-lang 0.29; `oapp-latest` pins 2.3 and 0.32.1. The Wormhole Anchor
//! SDK this program uses needs 1.18 / 0.30.1, and `solana-program` can appear
//! only once in a binary. Attempting the dependency fails at resolution, not
//! at compile time:
//!
//!   error: failed to select a version for `solana-program`
//!       ... required by package `endpoint` (LayerZero-v2)
//!       versions that meet the requirements `=1.17.31` are: 1.17.31
//!       all possible versions conflict with previously selected packages
//!
//! So LayerZero ships as an EXTERNAL DISPATCHER: its own program, built
//! against whatever stack `oapp` requires, which reads the prepared envelope,
//! forwards those exact bytes to the endpoint, and calls
//! `base_caller::mark_dispatched` back here. See `TransportConfig::dispatcher`
//! and the `external_dispatcher_*` tests. Nothing in this file is needed for
//! that path; it stays as the marker for where an in-process implementation
//! would go if the version conflict ever resolves.
//!
//! To build that dispatcher:
//!   1. New Anchor workspace, anchor-lang and solana-program matching `oapp`.
//!   2. Depend on `oapp` from the LayerZero-v2 git repo.
//!   3. On dispatch: read `PreparedMessage.envelope`, pass it UNMODIFIED as
//!      the message payload to `oapp::endpoint_cpi::send`. Re-encoding it
//!      changes the destination's message id and breaks quorum.
//!   4. Build options from the envelope's gas_limit with the SDK's builder.
//!   5. CPI `base_caller::mark_dispatched`, signing as the PDA seeded
//!      `["dispatcher"]` of the dispatcher program.
//!   6. Register it: `set_transport(TRANSPORT_LAYERZERO, .., dispatcher = <its id>)`.
//!   7. Configure the DVN set -- two independent DVNs is what makes LayerZero
//!      the recommended default; one gives up that advantage.

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
    // Unreachable by construction: no instruction of this program calls it.
    // LayerZero is dispatched out of process; see the module docs.
    unimplemented!("LayerZero dispatches out of process -- see the module docs")
}
