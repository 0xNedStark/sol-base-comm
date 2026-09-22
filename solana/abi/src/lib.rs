//! The wire-level ABI of `base_caller`, for programs that cannot link it.
//!
//! An external transport dispatcher is built against whatever anchor-lang and
//! solana-program versions its own SDK demands, and those conflict with
//! base_caller's (see `TransportConfig::dispatcher`). It therefore cannot use
//! base_caller's generated CPI helpers or account types, and must speak to it
//! in raw bytes instead.
//!
//! This crate has no dependencies at all, so any program can depend on it
//! whatever its own stack. It is the single place those raw constants live,
//! and `base_caller`'s own test suite asserts that the real Anchor-derived
//! values still match every constant here. Change base_caller's instruction
//! name or `PreparedMessage`'s layout and that test fails, rather than a
//! dispatcher silently reading the wrong bytes in production.
#![no_std]

/// Anchor instruction discriminator: sha256("global:mark_dispatched")[..8].
pub const MARK_DISPATCHED_IX: [u8; 8] = [13, 216, 84, 228, 161, 189, 95, 211];

/// Anchor account discriminator: sha256("account:PreparedMessage")[..8].
pub const PREPARED_MESSAGE_ACCOUNT: [u8; 8] = [169, 24, 80, 234, 91, 37, 123, 123];

/// Byte offsets into a `PreparedMessage` account, including the leading
/// 8-byte Anchor account discriminator.
///
/// ```text
///   0  8  discriminator
///   8 32  authority   Pubkey
///  40 32  payer       Pubkey
///  72  8  nonce       u64  LE
///  80  8  expiry      u64  LE
///  88  1  expected    u8   transport bitmask
///  89  1  dispatched  u8   transport bitmask
///  90  1  bump        u8
///  91  4  envelope    Vec<u8> length, u32 LE
///  95  n  envelope    bytes
/// ```
pub mod prepared_message {
    pub const AUTHORITY: usize = 8;
    pub const PAYER: usize = 40;
    pub const NONCE: usize = 72;
    pub const EXPIRY: usize = 80;
    pub const EXPECTED: usize = 88;
    pub const DISPATCHED: usize = 89;
    pub const BUMP: usize = 90;
    pub const ENVELOPE_LEN: usize = 91;
    pub const ENVELOPE: usize = 95;
}

/// Seed of the PDA an external dispatcher signs with when calling
/// `mark_dispatched`.
pub const DISPATCHER_SEED: &[u8] = b"dispatcher";

/// Seed prefix of a `PreparedMessage` PDA: `["msg", authority, nonce_le]`.
pub const PREPARED_MESSAGE_SEED: &[u8] = b"msg";

/// Seed prefix of a `TransportConfig` PDA: `["transport", transport_id]`.
pub const TRANSPORT_SEED: &[u8] = b"transport";

/// Seed of the `Config` PDA.
pub const CONFIG_SEED: &[u8] = b"config";

/// Read the envelope out of a `PreparedMessage` account's data.
///
/// Returns `None` if the discriminator is wrong or the buffer is short, so a
/// dispatcher handed the wrong account fails cleanly instead of forwarding
/// whatever bytes happened to be there.
pub fn envelope_of(data: &[u8]) -> Option<&[u8]> {
    if data.len() < prepared_message::ENVELOPE {
        return None;
    }
    if data[..8] != PREPARED_MESSAGE_ACCOUNT {
        return None;
    }
    let len_bytes = &data[prepared_message::ENVELOPE_LEN..prepared_message::ENVELOPE];
    let len = u32::from_le_bytes([len_bytes[0], len_bytes[1], len_bytes[2], len_bytes[3]]) as usize;
    let end = prepared_message::ENVELOPE.checked_add(len)?;
    data.get(prepared_message::ENVELOPE..end)
}

/// Read the per-sender nonce out of a `PreparedMessage` account's data.
pub fn nonce_of(data: &[u8]) -> Option<u64> {
    let b = data.get(prepared_message::NONCE..prepared_message::NONCE + 8)?;
    Some(u64::from_le_bytes([
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
    ]))
}
