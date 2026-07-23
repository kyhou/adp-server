use thiserror::Error;

/// Errors surfaced by protocol-level verification. HTTP status-code mapping happens
/// in adp-server's route handlers, not here — this crate stays transport-agnostic.
#[derive(Debug, Error)]
pub enum AdpError {
    #[error("invalid nostr event signature")]
    InvalidSignature,

    #[error("wrong event kind: expected {expected}, found {found}")]
    WrongKind { expected: u16, found: u16 },

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

    #[error("receipt is not a valid ADP kind 30402 coordinate")]
    InvalidAdpReceiptCoordinate,

    #[error("invalid receipt chain: {0}")]
    InvalidReceiptChain(String),

    #[error("delegated receipt signing requires an authorization anchor")]
    MissingReceiptAuthorizationAnchor,

    #[error("receipt authorization requires its resolved authorization lifecycle")]
    MissingResolvedReceiptAuthorization,

    #[error("invalid resolved receipt authorization dependency: {0}")]
    InvalidReceiptAuthorizationDependency(String),

    #[error("receipt authorization does not identify the authorization root")]
    ReceiptAuthorizationRootMismatch,

    #[error("receipt authorization developer does not match the receipt coordinate developer")]
    ReceiptAuthorizationDeveloperMismatch,

    #[error("authorization does not permit the receipt signer at the event timestamp")]
    ReceiptAuthorizationDenied,

    #[error("entitlement grant is not a valid ADP kind 30402 coordinate")]
    InvalidAdpGrantCoordinate,

    #[error("invalid resolved entitlement grant: {0}")]
    InvalidEntitlementGrant(String),

    #[error("invalid resolved campaign dependency: {0}")]
    InvalidCampaignDependency(String),

    #[error("invalid resolved authorization dependency: {0}")]
    InvalidAuthorizationDependency(String),

    #[error("ADP entitlement grant is missing its campaign source_event")]
    MissingCampaignSource,

    #[error("entitlement campaign source_event does not identify the campaign root")]
    CampaignRootMismatch,

    #[error("entitlement campaign developer does not match the grant coordinate developer")]
    CampaignDeveloperMismatch,

    #[error("entitlement campaign coordinate does not match the grant coordinate")]
    CampaignCoordinateMismatch,

    #[error("campaign was not active and claimable at entitlement issuance")]
    CampaignNotClaimableAtIssuance,

    #[error("delegated entitlement issuance requires an authorization anchor")]
    MissingGrantAuthorizationAnchor,

    #[error("entitlement authorization requires its resolved authorization lifecycle")]
    MissingResolvedGrantAuthorization,

    #[error("entitlement authorization does not identify the authorization root")]
    AuthorizationRootMismatch,

    #[error("authorization developer does not match the grant coordinate developer")]
    AuthorizationDeveloperMismatch,

    #[error("authorization does not permit the entitlement root signer at issuance")]
    GrantAuthorizationDenied,

    #[error("only the grant coordinate developer may sign entitlement updates")]
    UnauthorizedGrantUpdateSigner,

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
