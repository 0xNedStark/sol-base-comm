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
///
/// Ids 1..=MAX_TRANSPORT_ID are valid and map to bit (id - 1) of the dispatch
/// bitmask. Ids beyond the two named here need no code change: register a
/// `TransportConfig` for the id with an external dispatcher program and it
/// works. Only transports dispatched by an instruction of THIS program need
/// one added.
pub const TRANSPORT_WORMHOLE: u8 = 1;
pub const TRANSPORT_LAYERZERO: u8 = 2;

/// The bitmask is a u8, so eight transports can be registered.
pub const MAX_TRANSPORT_ID: u8 = 8;

/// Seed of the PDA an external dispatcher program signs with when calling
/// `mark_dispatched`. The dispatcher proves its identity by signing as
/// `find_program_address(&[DISPATCHER_SEED], &transport.dispatcher)`.
pub const DISPATCHER_SEED: &[u8] = b"dispatcher";

/// Bitmask positions, used in `PreparedMessage.expected` / `.dispatched`.
pub const MASK_WORMHOLE: u8 = 1 << 0;
pub const MASK_LAYERZERO: u8 = 1 << 1;

/// Bit for a transport id. Ids are 1-based so that a zero mask is
/// unambiguously "no transports".
pub const fn mask_of(transport_id: u8) -> Option<u8> {
    if transport_id == 0 || transport_id > MAX_TRANSPORT_ID {
        None
    } else {
        Some(1 << (transport_id - 1))
    }
}
