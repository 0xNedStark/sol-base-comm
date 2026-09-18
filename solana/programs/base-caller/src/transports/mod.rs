//! Transport modules.
//!
//! Each transport gets its own module and its own dispatch instruction in
//! lib.rs. Every one of them receives an already-built envelope from a
//! `PreparedMessage` and must forward it unmodified. No transport may
//! reinterpret, re-wrap or re-sign the envelope bytes: the destination hashes
//! them to derive the message id, so a transport that touched the payload would
//! produce a different id for the same logical message and break both
//! deduplication and the dual-transport quorum.

pub mod layerzero;
pub mod wormhole;

/// Transport ids, used as PDA seeds for `TransportConfig`.
pub const TRANSPORT_WORMHOLE: u8 = 1;
pub const TRANSPORT_LAYERZERO: u8 = 2;

/// Bitmask positions, used in `PreparedMessage.expected` / `.dispatched`.
pub const MASK_WORMHOLE: u8 = 1 << 0;
pub const MASK_LAYERZERO: u8 = 1 << 1;
pub const MASK_ALL: u8 = MASK_WORMHOLE | MASK_LAYERZERO;
