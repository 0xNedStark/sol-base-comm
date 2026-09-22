//! Guards the raw ABI that external dispatchers depend on.
//!
//! A dispatcher for a transport whose SDK cannot be linked into base_caller
//! talks to it in raw bytes: a hard-coded instruction discriminator and
//! hard-coded offsets into `PreparedMessage`. Those live in the
//! `base-caller-abi` crate, which has no dependencies so any stack can use it.
//!
//! Nothing stops someone renaming `mark_dispatched` or reordering
//! `PreparedMessage`'s fields -- base_caller would still compile, its own
//! tests would still pass, and every external dispatcher would start reading
//! the wrong bytes against a live bridge. These assertions are what turns that
//! into a failing build.

use anchor_lang::{Discriminator, InstructionData};
use base_caller::state::PreparedMessage;
use base_caller_abi as abi;

#[test]
fn mark_dispatched_discriminator_matches_the_published_abi() {
    let data = base_caller::instruction::MarkDispatched {
        nonce: 0,
        transport_id: 0,
    }
    .data();
    assert_eq!(
        &data[..8],
        &abi::MARK_DISPATCHED_IX,
        "base_caller's mark_dispatched discriminator changed; every external \
         dispatcher is now calling the wrong instruction. Update \
         base-caller-abi and every dispatcher built against it."
    );
}

#[test]
fn prepared_message_account_discriminator_matches_the_published_abi() {
    assert_eq!(
        PreparedMessage::DISCRIMINATOR,
        abi::PREPARED_MESSAGE_ACCOUNT,
        "PreparedMessage's account discriminator changed; dispatchers will \
         reject every message as not-a-PreparedMessage."
    );
}

#[test]
fn prepared_message_layout_matches_the_published_abi() {
    // Serialise a message with recognisable values and check the published
    // offsets actually land on those fields.
    use anchor_lang::AccountSerialize;

    let authority = anchor_lang::prelude::Pubkey::new_from_array([0x11; 32]);
    let payer = anchor_lang::prelude::Pubkey::new_from_array([0x22; 32]);
    let envelope = vec![0xDE, 0xAD, 0xBE, 0xEF, 0x05];
    let msg = PreparedMessage {
        authority,
        payer,
        nonce: 0x0102_0304_0506_0708,
        expiry: 0x1112_1314_1516_1718,
        expected: 0b011,
        dispatched: 0b001,
        bump: 254,
        envelope: envelope.clone(),
    };

    let mut buf = Vec::new();
    msg.try_serialize(&mut buf).unwrap();

    use abi::prepared_message as off;
    assert_eq!(&buf[..8], &abi::PREPARED_MESSAGE_ACCOUNT, "discriminator");
    assert_eq!(&buf[off::AUTHORITY..off::AUTHORITY + 32], authority.as_ref());
    assert_eq!(&buf[off::PAYER..off::PAYER + 32], payer.as_ref());
    assert_eq!(
        u64::from_le_bytes(buf[off::NONCE..off::NONCE + 8].try_into().unwrap()),
        msg.nonce
    );
    assert_eq!(
        u64::from_le_bytes(buf[off::EXPIRY..off::EXPIRY + 8].try_into().unwrap()),
        msg.expiry
    );
    assert_eq!(buf[off::EXPECTED], msg.expected);
    assert_eq!(buf[off::DISPATCHED], msg.dispatched);
    assert_eq!(buf[off::BUMP], msg.bump);
    assert_eq!(
        u32::from_le_bytes(buf[off::ENVELOPE_LEN..off::ENVELOPE].try_into().unwrap()) as usize,
        envelope.len()
    );

    // And the helpers a dispatcher actually calls.
    assert_eq!(abi::envelope_of(&buf), Some(&envelope[..]));
    assert_eq!(abi::nonce_of(&buf), Some(msg.nonce));
}

#[test]
fn envelope_helper_rejects_a_foreign_account() {
    let mut buf = vec![0u8; 200];
    buf[..8].copy_from_slice(&[9, 9, 9, 9, 9, 9, 9, 9]);
    assert_eq!(abi::envelope_of(&buf), None, "wrong discriminator");
    assert_eq!(abi::envelope_of(&[0u8; 4]), None, "too short");
}

#[test]
fn seeds_match_the_published_abi() {
    use base_caller::state::{Config, TransportConfig};
    assert_eq!(Config::SEED, abi::CONFIG_SEED);
    assert_eq!(TransportConfig::SEED, abi::TRANSPORT_SEED);
    assert_eq!(PreparedMessage::SEED, abi::PREPARED_MESSAGE_SEED);
    assert_eq!(base_caller::transports::DISPATCHER_SEED, abi::DISPATCHER_SEED);
}
