use thiserror::Error;

/// Errors surfaced by protocol-level verification. HTTP status-code mapping happens
/// in adp-server's route handlers, not here — this crate stays transport-agnostic.
#[derive(Debug, Error)]
pub enum AdpError {
    #[error("invalid nostr event signature")]
    InvalidSignature,

    #[error("event is missing required tag or field: {0}")]
    MissingTag(&'static str),

    #[error("event contains duplicate tag: {0}")]
    DuplicateTag(String),

    #[error("event contains malformed tag: {0}")]
    MalformedTag(String),

    #[error("event contains forbidden tag: {0}")]
    ForbiddenTag(String),

    #[error("listing coordinate mismatch: expected {expected}, found {found}")]
    CoordinateMismatch { expected: String, found: String },

    #[error("receipt signer {0} is not authorized to sign for this listing (not the developer key, and not an active fulfillment delegation)")]
    UnauthorizedSigner(String),

    #[error("fulfillment key was revoked at {revoked_at}, but receipt created_at ({created_at}) is at or after that revocation")]
    RevokedAtSigningTime { revoked_at: u64, created_at: u64 },

    #[error("payment proof invalid: {0}")]
    InvalidPaymentProof(String),

    #[error("nip-98 auth failed: {0}")]
    Nip98(String),

    #[error("nip-44 encryption failed: {0}")]
    Encryption(String),

    #[error("nostr protocol error: {0}")]
    Nostr(String),

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
}
