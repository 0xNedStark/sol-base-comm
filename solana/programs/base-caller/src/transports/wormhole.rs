//! Wormhole core bridge transport.
//!
//! Publishes the envelope as a Wormhole message. The guardian network observes
//! it, signs a VAA, and anyone can submit that VAA to `WormholeAdapter` on Base.
//!
//! STATUS: reference skeleton. The instruction discriminant, account ordering
//! and fee mechanics below follow the core bridge's documented shape but were
//! not verifiable from the environment this was written in. Check them against
//! the deployed core bridge and an integration test on devnet before mainnet.

use anchor_lang::prelude::*;
use anchor_lang::solana_program::{
    instruction::{AccountMeta, Instruction},
    program::{invoke, invoke_signed},
    system_instruction,
};

use crate::state::TransportConfig;

/// Core bridge `post_message` instruction discriminant.
const IX_POST_MESSAGE: u8 = 1;

/// Consistency level to publish at.
///
/// This is the reorg guard, and it is the single most dangerous constant in the
/// Solana half of the system: the unsafe value is also the fast one. A message
/// attested on a slot that later gets rolled back is a forged call that every
/// downstream check will happily accept. Publish at finalized, and make sure the
/// Base-side `minConsistencyLevel` demands the same. Confirm the numeric
/// encoding against current Wormhole docs.
const CONSISTENCY_FINALIZED: u8 = 1;

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
    let fee = read_message_fee(&accounts.bridge)?;
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

    let mut data = Vec::with_capacity(1 + 4 + 4 + envelope.len() + 1);
    data.push(IX_POST_MESSAGE);
    data.extend_from_slice(&batch_nonce.to_le_bytes());
    data.extend_from_slice(&(envelope.len() as u32).to_le_bytes());
    data.extend_from_slice(envelope);
    data.push(CONSISTENCY_FINALIZED);

    let ix = Instruction {
        program_id: *accounts.program.key,
        accounts: vec![
            AccountMeta::new(*accounts.bridge.key, false),
            AccountMeta::new(*accounts.message.key, true),
            AccountMeta::new_readonly(*accounts.emitter.key, true),
            AccountMeta::new(*accounts.sequence.key, false),
            AccountMeta::new(*accounts.payer.key, true),
            AccountMeta::new(*accounts.fee_collector.key, false),
            AccountMeta::new_readonly(*accounts.clock.key, false),
            AccountMeta::new_readonly(*accounts.system_program.key, false),
            AccountMeta::new_readonly(*accounts.rent.key, false),
        ],
        data,
    };

    // The emitter is a PDA of this program, so this program signs for it. That
    // PDA is the identity `WormholeAdapter.solanaPeer` pins on Base -- it is the
    // root of the whole trust chain and no external key can produce it.
    invoke_signed(
        &ix,
        &[
            accounts.bridge.clone(),
            accounts.message.clone(),
            accounts.emitter.clone(),
            accounts.sequence.clone(),
            accounts.payer.clone(),
            accounts.fee_collector.clone(),
            accounts.clock.clone(),
            accounts.system_program.clone(),
            accounts.rent.clone(),
        ],
        &[&[b"emitter", &[accounts.emitter_bump]]],
    )?;

    Ok(())
}

/// Read the current message fee from the bridge config account.
/// Layout: guardian_set_index (u32) | last_deployed (u32) | guardian_set_expiry
/// (u32) | fee (u64 LE). Verify against the deployed bridge.
fn read_message_fee(bridge: &AccountInfo) -> Result<u64> {
    let data = bridge.try_borrow_data()?;
    const FEE_OFFSET: usize = 4 + 4 + 4;
    if data.len() < FEE_OFFSET + 8 {
        return Ok(0);
    }
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&data[FEE_OFFSET..FEE_OFFSET + 8]);
    Ok(u64::from_le_bytes(buf))
}
