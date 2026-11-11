//! One authentication boundary for actual raw request targets, never decoded routes.
use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{
    extract::{FromRequestParts, OriginalUri},
    http::{request::Parts, HeaderMap, Uri},
};
use nostr_sdk::PublicKey;

pub(crate) struct Authenticated(pub PublicKey);

#[async_trait::async_trait]
impl FromRequestParts<AppState> for Authenticated {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let uri = parts
            .extensions
            .get::<OriginalUri>()
            .ok_or_else(|| ApiError::BadRequest("missing original request target".into()))?;
        authenticate(state, &parts.headers, parts.method.as_str(), &uri.0).map(Self)
    }
}

pub(super) fn authenticate(
    state: &AppState,
    headers: &HeaderMap,
    method: &str,
    uri: &Uri,
) -> ApiResult<PublicKey> {
    let header = headers
        .get("Authorization")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| ApiError::Unauthorized("missing Authorization header".into()))?;
    let event = adp_core::decode_authorization_header(header)?;
    let expected_url = request_url(&state.config.server_url, uri)?;
    Ok(adp_core::verify_nip98(&event, method, &expected_url)?.pubkey)
}

pub(super) fn request_url(server_url: &str, uri: &Uri) -> ApiResult<String> {
    let invalid_origin = || ApiError::Internal("invalid ADP_SERVER_URL origin".into());
    let configured: Uri = server_url.parse().map_err(|_| invalid_origin())?;
    let scheme = configured.scheme_str().ok_or_else(invalid_origin)?;
    let authority = configured.authority().ok_or_else(invalid_origin)?;
    let port_suffix = &authority.as_str()[authority.host().len()..];
    if !matches!(scheme, "http" | "https")
        || authority.host().is_empty()
        || authority.as_str().contains('@')
        || (!port_suffix.is_empty()
            && port_suffix
                .strip_prefix(':')
                .and_then(|port| port.parse::<u16>().ok())
                .is_none())
    {
        return Err(invalid_origin());
    }
    let target = uri
        .path_and_query()
        .ok_or_else(|| ApiError::BadRequest("missing request path and query".into()))?;
    // Host/forwarding headers are untrusted; path and query must remain verbatim.
    Ok(format!("{scheme}://{authority}{}", target.as_str()))
}
