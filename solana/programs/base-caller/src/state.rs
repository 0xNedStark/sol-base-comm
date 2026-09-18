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
    /// Base's id *in this transport's namespace* (Wormhole: 30, LayerZero:
    /// 30184 -- confirm before deploy). Never the internal chain id.
    pub dest_chain: u32,
    pub bump: u8,
}

impl TransportConfig {
    pub const SEED: &'static [u8] = b"transport";
    pub const LEN: usize = 8 + 1 + 1 + 32 + 32 + 4 + 1;
}
