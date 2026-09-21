//! Wormhole core bridge transport.
//!
//! Publishes the envelope as a Wormhole message. The guardian network observes
//! it, signs a VAA, and anyone can submit that VAA to `WormholeAdapter` on the
//! destination chain.
//!
//! The CPI is performed through `wormhole-anchor-sdk`, the bridge's own Anchor
//! bindings, rather than by hand-encoding the instruction. That matters: the
//! instruction discriminant, the order and mutability of the nine accounts,
//! and the `Finality` encoding are all things this repo would otherwise be
//! transcribing from documentation and silently getting wrong on the day the
//! bridge changes them.

use anchor_lang::prelude::*;
use anchor_lang::solana_program::{program::invoke, system_instruction};
use wormhole_anchor_sdk::wormhole::{self, BridgeData, Finality};

use crate::state::TransportConfig;

/// Publish at finalized, never confirmed.
///
/// This is the reorg guard and it is the single most dangerous constant in the
/// Solana half of the system, because the unsafe value is also the fast one. A
/// message attested on a slot that is later rolled back is a forged call that
/// every downstream check accepts. `Finality::Finalized` is the SDK's own
/// encoding (it serialises to 1; `Confirmed` is 0), so this cannot drift from
/// what the bridge expects.
///
/// The destination-side `WormholeAdapter.minConsistencyLevel` must demand the
/// same. Two independent places, so a mistake in one is caught by the other.
const FINALITY: Finality = Finality::Finalized;

pub struct WormholeAccounts<'info> {
    pub bridge: AccountInfo<'info>,
    pub message: AccountInfo<'info>,
    pub emitter: AccountInfo<'info>,
    pub sequence: AccountInfo<'info>,
    pub fee_collector: AccountInfo<'info>,
    pub program: AccountInfo<'info>,
    pub payer: AccountInfo<'info>,
    pub clock: AccountInfo<'info>,
    pub rent: AccountInfo<'info>,
    pub system_program: AccountInfo<'info>,
    pub emitter_bump: u8,
}

pub fn post_message(
    _transport: &Account<TransportConfig>,
    envelope: &[u8],
    accounts: &WormholeAccounts,
    batch_nonce: u32,
) -> Result<()> {
    // The bridge charges a per-message fee, collected by transfer to the fee
    // collector before the call.
    let fee = message_fee(&accounts.bridge)?;
    if fee > 0 {
        invoke(
            &system_instruction::transfer(accounts.payer.key, accounts.fee_collector.key, fee),
            &[
                accounts.payer.clone(),
                accounts.fee_collector.clone(),
                accounts.system_program.clone(),
            ],
        )?;
    }

    // The emitter is a PDA of this program, so this program signs for it. That
    // PDA is the identity `WormholeAdapter.solanaPeer` pins on the destination
    // chain -- the root of the whole trust chain, and one no external key can
    // produce.
    // Bound to a local: the seeds array must outlive the CpiContext.
    let emitter_bump = [accounts.emitter_bump];
    let emitter_seeds: &[&[u8]] = &[wormhole::SEED_PREFIX_EMITTER, &emitter_bump];
    let signer_seeds: &[&[&[u8]]] = &[emitter_seeds];

    let cpi = CpiContext::new_with_signer(
        accounts.program.clone(),
        wormhole::PostMessage {
            config: accounts.bridge.clone(),
            message: accounts.message.clone(),
            emitter: accounts.emitter.clone(),
            sequence: accounts.sequence.clone(),
            payer: accounts.payer.clone(),
            fee_collector: accounts.fee_collector.clone(),
            clock: accounts.clock.clone(),
            rent: accounts.rent.clone(),
            system_program: accounts.system_program.clone(),
        },
        signer_seeds,
    );

    // The envelope goes over the wire byte for byte. Never re-wrap or re-encode
    // it here: the destination hashes these bytes to derive the message id, so
    // any change produces a different id for the same logical message and
    // breaks both deduplication and the dual-transport quorum.
    wormhole::post_message(cpi, batch_nonce, envelope.to_vec(), FINALITY)
}

/// Current per-message fee, read through the SDK's `BridgeData` layout rather
/// than by indexing into the account at a hard-coded offset.
///
/// An empty account means no bridge is deployed there, which happens only
/// against the localnet mock; the real bridge always carries config, and if a
/// fee were somehow underpaid the bridge itself rejects the message.
fn message_fee(bridge: &AccountInfo) -> Result<u64> {
    let data = bridge.try_borrow_data()?;
    if data.is_empty() {
        return Ok(0);
    }
    let mut slice: &[u8] = &data;
    Ok(BridgeData::try_deserialize_unchecked(&mut slice)?.fee())
}
