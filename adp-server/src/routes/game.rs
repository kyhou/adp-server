use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};

use adp_core::{decode_authorization_header, verify_nip98, AcquisitionPolicy, AdpListing};
use nostr_sdk::PublicKey;

use super::auth::request_url as download_auth_url;
use crate::error::{ApiError, ApiResult};
use crate::ownership::{
    verify_ownership_via_entitlements, verify_ownership_via_receipts, AccessBasis, CredentialCheck,
};
use crate::relay::RelayLookup;
use crate::state::AppState;

#[derive(serde::Deserialize)]
pub struct DownloadQuery {
    token: Option<String>,
}

/// `GET /game/:game_coordinate` keeps current distribution and file policy separate
/// from token, durable credential, and explicit acquisition authorization.
pub async fn download_handler(
    State(state): State<AppState>,
    Path(game_coordinate): Path<String>,
    Query(query): Query<DownloadQuery>,
    OriginalUri(uri): OriginalUri,
    method: Method,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let (developer_pubkey, d_tag) = parse_coordinate(&game_coordinate)?;

    let listing_event = match state.relay.lookup_listing(developer_pubkey, &d_tag).await {
        RelayLookup::Found(event) => event,
        RelayLookup::ConfirmedAbsent { .. } => {
            return Err(ApiError::NotFound("listing not found on relays".into()))
        }
        RelayLookup::Unavailable { .. } => {
            return Err(ApiError::ServiceUnavailable(
                "listing lookup unavailable".into(),
            ))
        }
    };
    listing_event
        .verify()
        .map_err(|_| ApiError::BadRequest("invalid listing signature".into()))?;
    let listing = AdpListing::from_event(listing_event)?;
    if listing.developer_pubkey != developer_pubkey || listing.coordinate() != game_coordinate {
        return Err(ApiError::BadRequest(
            "listing does not match requested coordinate".into(),
        ));
    }
    if !listing.distributes_via(state.relay.server_url()) {
        return Err(ApiError::DistributionUnauthorized(
            "this server is not named in the listing's server tags for this game".into(),
        ));
    }
    let file_hash = listing
        .file_hash
        .as_deref()
        .ok_or_else(|| ApiError::NotFound("listing has no file_hash tag".into()))?;
    let file_path = state
        .storage
        .file_path_for_hash(file_hash)
        .await?
        .ok_or_else(|| ApiError::NotFound("file not hosted on this server".into()))?;

    let access_basis = if let Some(token) = &query.token {
        state
            .storage
            .resolve_download_token(token, &game_coordinate)
            .await?
            .ok_or_else(|| ApiError::Forbidden("invalid or expired download token".into()))?;
        AccessBasis::DownloadToken
    } else {
        let auth_header = headers
            .get("Authorization")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| {
                ApiError::Unauthorized("missing Authorization header or ?token=".into())
            })?;
        let token_event = decode_authorization_header(auth_header)?;
        let expected_url = download_auth_url(&state.config.server_url, &uri)?;
        let buyer = verify_nip98(&token_event, method.as_str(), &expected_url)?.pubkey;
        // Defense in depth: retain the exact first-valued u tag comparison.
        let signed_url = token_event.tags.iter().find_map(|tag| {
            let values = tag.as_slice();
            (values.first().map(String::as_str) == Some("u"))
                .then(|| values.get(1).map(String::as_str))
                .flatten()
        });
        if signed_url != Some(expected_url.as_str()) {
            return Err(ApiError::Unauthorized(
                "u tag does not match the exact requested URL".into(),
            ));
        }
        let entitlement =
            verify_ownership_via_entitlements(&state, buyer, &game_coordinate, developer_pubkey)
                .await;
        let receipt = verify_ownership_via_receipts(
            &state,
            buyer,
            &game_coordinate,
            developer_pubkey,
            &listing,
        )
        .await;
        match (&entitlement, &receipt) {
            (CredentialCheck::Valid(basis), _) | (_, CredentialCheck::Valid(basis)) => *basis,
            _ => match &listing.acquisition_policy {
                AcquisitionPolicy::Public => AccessBasis::PublicPolicy,
                AcquisitionPolicy::TimedAccess { .. }
                    if listing.acquisition_policy.allows_at(unix_now()) =>
                {
                    AccessBasis::TimedAccessPolicy
                }
                _ if matches!(entitlement, CredentialCheck::Unavailable { .. })
                    || matches!(receipt, CredentialCheck::Unavailable { .. }) =>
                {
                    return Err(ApiError::ServiceUnavailable(
                        "durable ownership lookup unavailable".into(),
                    ));
                }
                _ => {
                    return Err(ApiError::Forbidden(
                        "no valid ownership credential or explicit acquisition policy".into(),
                    ))
                }
            },
        }
    };
    tracing::debug!(?access_basis, game_coordinate, "download authorized");

    verify_file_hash(&file_path, file_hash).await?;
    if if_none_match_matches(&headers, file_hash) {
        return Ok(not_modified_response(file_hash));
    }

    let bytes = tokio::fs::read(&file_path)
        .await
        .map_err(|_| ApiError::NotFound("stored file is missing on disk".into()))?;
    Ok(ok_file_response(file_hash, bytes))
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_secs()
}

fn ok_file_response(file_hash: &str, bytes: Vec<u8>) -> Response {
    let mut response = bytes.into_response();
    response.headers_mut().insert(
        header::ETAG,
        HeaderValue::from_str(&etag_value(file_hash)).expect("valid file_hash etag"),
    );
    response
}

fn not_modified_response(file_hash: &str) -> Response {
    let mut response = StatusCode::NOT_MODIFIED.into_response();
    response.headers_mut().insert(
        header::ETAG,
        HeaderValue::from_str(&etag_value(file_hash)).expect("valid file_hash etag"),
    );
    response
}

fn etag_value(file_hash: &str) -> String {
    format!("\"{file_hash}\"")
}

fn if_none_match_matches(headers: &HeaderMap, file_hash: &str) -> bool {
    let expected = etag_value(file_hash);
    headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.split(',').any(|etag| etag.trim() == expected))
        .unwrap_or(false)
}

pub(crate) async fn verify_file_hash(file_path: &str, expected_hash: &str) -> ApiResult<()> {
    let bytes = tokio::fs::read(file_path)
        .await
        .map_err(|_| ApiError::NotFound("stored file is missing on disk".into()))?;
    let computed = hex::encode(Sha256::digest(&bytes));
    if computed != expected_hash {
        return Err(ApiError::Internal(
            "stored file's SHA-256 does not match listing file_hash".into(),
        ));
    }
    Ok(())
}

pub(super) fn parse_coordinate(coordinate: &str) -> ApiResult<(PublicKey, String)> {
    let parts: Vec<&str> = coordinate.splitn(3, ':').collect();
    if parts.len() != 3 || parts[0] != "30402" {
        return Err(ApiError::BadRequest(
            "malformed game_coordinate — expected 30402:<pubkey>:<d-tag>".into(),
        ));
    }
    let developer_pubkey = PublicKey::from_hex(parts[1])
        .map_err(|_| ApiError::BadRequest("malformed developer pubkey in coordinate".into()))?;
    Ok((developer_pubkey, parts[2].to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Uri;
    use sha2::Digest;

    #[test]
    fn download_url_uses_only_configured_origin_and_raw_target() {
        let uri: Uri = "/game/30402%3Aabc%3Agame?x=%2f&y=+&x=/"
            .parse()
            .expect("uri");
        for origin in [
            "http://localhost:8080",
            "https://example.com:443",
            "http://[::1]:80",
        ] {
            let configured = format!("{origin}/ignored/path?ignored=yes");
            assert_eq!(
                download_auth_url(&configured, &uri).expect("url"),
                format!("{origin}{uri}")
            );
        }
    }

    #[test]
    fn download_url_rejects_invalid_configured_origins() {
        let uri = "/game/test".parse().expect("uri");
        for configured in [
            "",
            "/relative",
            "localhost:8080",
            "ftp://example.com",
            "http://",
            "http://user@example.com",
            "http://example.com:99999",
            "http://example.com:bad",
        ] {
            assert!(download_auth_url(configured, &uri).is_err(), "{configured}");
        }
    }

    #[test]
    fn if_none_match_accepts_quoted_file_hash_etag() {
        let headers = HeaderMap::from_iter([(
            axum::http::header::IF_NONE_MATCH,
            axum::http::HeaderValue::from_static("\"abc123\""),
        )]);

        assert!(if_none_match_matches(&headers, "abc123"));
    }

    #[test]
    fn if_none_match_rejects_different_etag() {
        let headers = HeaderMap::from_iter([(
            axum::http::header::IF_NONE_MATCH,
            axum::http::HeaderValue::from_static("\"other\""),
        )]);

        assert!(!if_none_match_matches(&headers, "abc123"));
    }

    #[tokio::test]
    async fn verify_file_hash_accepts_matching_bytes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("game.bin");
        tokio::fs::write(&path, b"game bytes")
            .await
            .expect("write file");
        let hash = hex::encode(sha2::Sha256::digest(b"game bytes"));

        verify_file_hash(path.to_str().expect("utf8 path"), &hash)
            .await
            .expect("hash verifies");
    }

    #[tokio::test]
    async fn verify_file_hash_rejects_mismatch() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("game.bin");
        tokio::fs::write(&path, b"tampered bytes")
            .await
            .expect("write file");

        assert!(
            verify_file_hash(path.to_str().expect("utf8 path"), "abc123")
                .await
                .is_err()
        );
    }
}
