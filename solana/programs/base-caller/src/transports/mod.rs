//! Transport modules.
//!
//! Each transport gets its own module and its own instruction in lib.rs, but all
//! of them receive an already-built envelope and must forward it unmodified. No
//! transport may reinterpret, re-wrap or re-sign the envelope bytes: the Base
//! side hashes them to derive the message id, so a transport that touched the
//! payload would produce a different id for the same logical message and break
//! both deduplication and the dual-transport quorum.

pub mod layerzero;
pub mod wormhole;

pub const TRANSPORT_WORMHOLE: u8 = 1;
pub const TRANSPORT_LAYERZERO: u8 = 2;
