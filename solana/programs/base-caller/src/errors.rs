use anchor_lang::prelude::*;

#[error_code]
pub enum BaseCallerError {
    #[msg("Program is paused")]
    Paused,
    #[msg("Transport is not enabled")]
    TransportDisabled,
    #[msg("Calldata exceeds the maximum size")]
    CalldataTooLarge,
    #[msg("Unknown execution mode")]
    InvalidMode,
    #[msg("Target address must not be the zero address")]
    InvalidTarget,
    #[msg("Gas limit below the minimum")]
    GasLimitTooLow,
    #[msg("Expiry is already in the past")]
    ExpiryInPast,
    #[msg("Nonce overflow")]
    NonceOverflow,
    #[msg("Not the admin")]
    NotAdmin,
    #[msg("Not the pending admin")]
    NotPendingAdmin,
    #[msg("Envelope encoding failed")]
    EncodeFailed,
}
