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
    #[msg("Destination chain id is zero or is Solana itself")]
    InvalidDestination,
}

#[error_code]
pub enum PrepareError {
    #[msg("At least one transport must be selected")]
    NoTransports,
    #[msg("Unknown transport bit in mask")]
    UnknownTransport,
    #[msg("This transport was not selected at prepare time")]
    TransportNotExpected,
    #[msg("This transport has already carried the message")]
    AlreadyDispatched,
    #[msg("Not every expected transport has dispatched yet")]
    NotFullyDispatched,
    #[msg("Prepared message has expired; finalize to reclaim rent")]
    PreparedExpired,
}
