use axum::{
    http::StatusCode,
    response::{IntoResponse, Json, Response},
};
use serde_json::json;
use thiserror::Error;

/// adp-server's own error type. Wraps `adp_core::AdpError` and `sqlx::Error` (which
/// can't get a blanket `IntoResponse` impl directly due to the orphan rule) and maps
/// each variant to the HTTP status codes defined in ADP-01.md's endpoint tables.
#[derive(Debug, Error)]
pub enum ApiError {
    #[error(transparent)]
    Protocol(#[from] adp_core::AdpError),

    #[error("storage error: {0}")]
    Storage(#[from] sqlx::Error),

    #[error("{0}")]
    BadRequest(String),

    #[error("{0}")]
    Unauthorized(String),

    #[error("{0}")]
    PaymentRequired(String),

    #[error("{0}")]
    Conflict(String),

    #[error("{0}")]
    NotFound(String),

    #[error("{0}")]
    Forbidden(String),

    /// HTTP 451 — server is not an authorized distributor for this listing.
    /// Distinct from `Forbidden` (403, ownership failure) per ADP-01.md.
    #[error("{0}")]
    DistributionUnauthorized(String),

    #[error("internal error: {0}")]
    Internal(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            ApiError::Protocol(e) => match e {
                adp_core::AdpError::Nip98(_) => (StatusCode::UNAUTHORIZED, e.to_string()),
                adp_core::AdpError::Encryption(_)
                | adp_core::AdpError::Nostr(_)
                | adp_core::AdpError::Serde(_) => {
                    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
                }
                _ => (StatusCode::BAD_REQUEST, e.to_string()),
            },
            ApiError::Storage(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
            ApiError::BadRequest(m) => (StatusCode::BAD_REQUEST, m.clone()),
            ApiError::Unauthorized(m) => (StatusCode::UNAUTHORIZED, m.clone()),
            ApiError::PaymentRequired(m) => (StatusCode::PAYMENT_REQUIRED, m.clone()),
            ApiError::Conflict(m) => (StatusCode::CONFLICT, m.clone()),
            ApiError::NotFound(m) => (StatusCode::NOT_FOUND, m.clone()),
            ApiError::Forbidden(m) => (StatusCode::FORBIDDEN, m.clone()),
            ApiError::DistributionUnauthorized(m) => {
                (StatusCode::from_u16(451).unwrap(), m.clone())
            }
            ApiError::Internal(m) => (StatusCode::INTERNAL_SERVER_ERROR, m.clone()),
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}

pub type ApiResult<T> = Result<T, ApiError>;
