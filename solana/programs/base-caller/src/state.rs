use anchor_lang::prelude::*;

/// Global program configuration. One per deployment.
#[account]
pub struct Config {
    pub admin: Pubkey,
    /// Two-step admin handover, so a typo cannot orphan the program.
    pub pending_admin: Pubkey,
    /// Kill switch. Stops all outbound sends; does not affect messages already
    /// in flight, which is exactly why the Base side needs its own pause too.
    pub paused: bool,
    pub bump: u8,
}

impl Config {
    pub const SEED: &'static [u8] = b"config";
    pub const LEN: usize = 8 + 32 + 32 + 1 + 1;
}

/// Per-authority nonce. One PDA per sender, so two unrelated callers can never
/// contend for the same counter and stall each other.
#[account]
pub struct SenderState {
    pub authority: Pubkey,
    pub nonce: u64,
    pub bump: u8,
}

impl SenderState {
    pub const SEED: &'static [u8] = b"sender";
    pub const LEN: usize = 8 + 32 + 8 + 1;
}

/// Per-transport wiring. One PDA per transport id.
#[account]
pub struct TransportConfig {
    /// See `TRANSPORT_*` in transports/mod.rs.
    pub transport_id: u8,
    pub enabled: bool,
    /// Wormhole core bridge program, or LayerZero endpoint program.
    pub program_id: Pubkey,
    /// The Base-side adapter contract, left-padded to 32 bytes.
    ///
    /// The Solana side pins its counterpart just as the adapter pins this
    /// program. Trust is mutual and explicit at both ends; neither side accepts
    /// a message merely because it arrived over the right transport.
    pub peer: [u8; 32],
    /// The destination's id *in this transport's namespace* (Wormhole: Base
    /// is 30; LayerZero: Base is 30184). Never the internal chain id.
    pub dest_chain: u32,
    /// Program allowed to mark this transport dispatched from outside, or
    /// `Pubkey::default()` for transports dispatched by an instruction of this
    /// program.
    ///
    /// This exists because some transport SDKs cannot be linked into this
    /// program at all. LayerZero's Solana endpoint pins
    /// `solana-program = "=1.17.31"` and anchor-lang 0.29, while the Wormhole
    /// Anchor SDK needs 1.18 / 0.30.1; solana-program can only appear once in
    /// a binary, so no single program can carry both. A transport on an
    /// incompatible stack therefore ships as its own program that reads the
    /// prepared envelope and calls `mark_dispatched` here.
    pub dispatcher: Pubkey,
    pub bump: u8,
}

impl TransportConfig {
    pub const SEED: &'static [u8] = b"transport";
    pub const LEN: usize = 8 + 1 + 1 + 32 + 32 + 4 + 32 + 1;
}

/// An envelope built once and awaiting dispatch over one or more transports.
///
/// This is what makes dual-transport quorum reachable. If each transport's
/// send instruction built its own envelope, the same logical call would get
/// two nonces, two hashes and two message ids on the destination -- and a
/// quorum keyed on message id would never be met. So the envelope is built
/// exactly once here, and every dispatch forwards these bytes unmodified.
#[account]
pub struct PreparedMessage {
    pub authority: Pubkey,
    /// Paid the rent; receives it back on `finalize`.
    pub payer: Pubkey,
    pub nonce: u64,
    /// Mirrors the envelope so dispatch can refuse an expired message
    /// without re-parsing the bytes.
    pub expiry: u64,
    /// Bitmask of transports expected to carry this envelope.
    pub expected: u8,
    /// Bitmask of transports that have carried it so far.
    pub dispatched: u8,
    pub bump: u8,
    /// The canonical bytes. Never modified after `prepare`.
    pub envelope: Vec<u8>,
}

impl PreparedMessage {
    pub const SEED: &'static [u8] = b"msg";

    pub fn space(envelope_len: usize) -> usize {
        8 + 32 + 32 + 8 + 8 + 1 + 1 + 1 + 4 + envelope_len
    }
}
