use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;

use adp_core::{
    decode_authorization_header, latest_status_in_chain, parse_receipt_event, verify_nip98,
    verify_signer_authorization, AdpListing, ParsedReceipt,
};
use nostr_sdk::{Event, PublicKey};

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

#[derive(serde::Deserialize)]
pub struct DownloadQuery {
    token: Option<String>,
}

/// `GET /game/:game_coordinate` — requires both an ownership check (Path A: download
/// token, or Path B: relay receipt query) AND a distribution-authorization check
/// (this server must be named in the listing's `server` tags). See ADP-01.md.
pub async fn download_handler(
    State(state): State<AppState>,
    Path(game_coordinate): Path<String>,
    Query(query): Query<DownloadQuery>,
    headers: HeaderMap,
) -> ApiResult<Vec<u8>> {
    let (developer_pubkey, d_tag) = parse_coordinate(&game_coordinate)?;

    let buyer_pubkey_hex = if let Some(token) = &query.token {
        state
            .storage
            .resolve_download_token(token, &game_coordinate)
            .await?
            .ok_or_else(|| ApiError::Forbidden("invalid or expired download token".into()))?
    } else {
        let auth_header = headers
            .get("Authorization")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| {
                ApiError::Unauthorized("missing Authorization header or ?token=".into())
            })?;
        let token_event = decode_authorization_header(auth_header)?;
        let expected_url = format!("{}/game/{}", state.config.server_url, game_coordinate);
        verify_nip98(&token_event, "GET", &expected_url)?
            .pubkey
            .to_hex()
    };

    // Ownership Path B only runs when we didn't already resolve via a trusted local
    // download token — a token from /purchase/confirm already implies ownership.
    if query.token.is_none() {
        let buyer_pubkey = PublicKey::from_hex(&buyer_pubkey_hex)
            .map_err(|_| ApiError::Internal("malformed buyer pubkey".into()))?;
        let owns = verify_ownership_via_receipts(
            &state,
            buyer_pubkey,
            &game_coordinate,
            developer_pubkey,
            &d_tag,
        )
        .await?;
        if !owns {
            return Err(ApiError::Forbidden(
                "no valid purchase receipt found for this buyer and listing".into(),
            ));
        }
    }

    // Distribution authorization: independent of ownership. This server must be
    // named in the listing's `server` tags to serve the file at all.
    let listing_event = state
        .relay
        .fetch_listing(developer_pubkey, &d_tag)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or_else(|| ApiError::NotFound("listing not found on relays".into()))?;
    let listing = AdpListing::from_event(listing_event)?;

    if !listing.distributes_via(state.relay.server_url()) {
        return Err(ApiError::DistributionUnauthorized(
            "this server is not named in the listing's server tags for this game".into(),
        ));
    }

    let file_hash = listing
        .file_hash
        .ok_or_else(|| ApiError::NotFound("listing has no file_hash tag".into()))?;
    let file_path = state
        .storage
        .file_path_for_hash(&file_hash)
        .await?
        .ok_or_else(|| ApiError::NotFound("file not hosted on this server".into()))?;

    let bytes = tokio::fs::read(&file_path)
        .await
        .map_err(|_| ApiError::NotFound("stored file is missing on disk".into()))?;
    Ok(bytes)
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

fn receipt_matches_request(
    parsed: &ParsedReceipt,
    buyer_pubkey: PublicKey,
    game_coordinate: &str,
) -> bool {
    parsed.buyer_pubkey == buyer_pubkey && parsed.coordinate == game_coordinate
}

/// Ownership Path B: query relays for `kind:1020` receipts addressed to `buyer_pubkey`
/// referencing `game_coordinate`, group by order id, resolve each order's status-chain
/// tip, and check whether any tip is a `paid`/`fulfilled` receipt whose signer is
/// authorized per the (freshly fetched) listing's delegation tags.
async fn verify_ownership_via_receipts(
    state: &AppState,
    buyer_pubkey: PublicKey,
    game_coordinate: &str,
    developer_pubkey: PublicKey,
    d_tag: &str,
) -> ApiResult<bool> {
    let receipts = state
        .relay
        .fetch_receipts(buyer_pubkey, game_coordinate)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    if receipts.is_empty() {
        return Ok(false);
    }

    let mut by_order: HashMap<String, Vec<Event>> = HashMap::new();
    for event in receipts {
        if event.verify().is_err() {
            continue;
        }
        if let Ok(parsed) = parse_receipt_event(&event) {
            if receipt_matches_request(&parsed, buyer_pubkey, game_coordinate) {
                by_order
                    .entry(parsed.order_id.clone())
                    .or_default()
                    .push(event);
            }
        }
    }

    let listing_event = state
        .relay
        .fetch_listing(developer_pubkey, d_tag)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let listing_event = match listing_event {
        Some(e) => e,
        None => return Ok(false),
    };
    let listing = AdpListing::from_event(listing_event)?;

    for events in by_order.values() {
        let Some(tip) = latest_status_in_chain(events) else {
            continue;
        };
        let Ok(parsed) = parse_receipt_event(tip) else {
            continue;
        };
        if parsed.status != "paid" && parsed.status != "fulfilled" {
            continue;
        }
        if verify_signer_authorization(tip, &listing).is_ok() {
            return Ok(true);
        }
    }

    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use adp_core::ParsedReceipt;
    use nostr_sdk::Keys;

    fn parsed_receipt(buyer_pubkey: PublicKey, coordinate: &str) -> ParsedReceipt {
        ParsedReceipt {
            order_id: "order-1".to_string(),
            buyer_pubkey,
            coordinate: coordinate.to_string(),
            status: "paid".to_string(),
            amount: None,
            currency: None,
            e_tag: None,
        }
    }

    #[test]
    fn receipt_matches_request_requires_buyer_and_coordinate() {
        let buyer = Keys::generate().public_key();
        let other_buyer = Keys::generate().public_key();
        let coordinate = "30402:developer:game-1";

        assert!(receipt_matches_request(
            &parsed_receipt(buyer, coordinate),
            buyer,
            coordinate
        ));
        assert!(!receipt_matches_request(
            &parsed_receipt(other_buyer, coordinate),
            buyer,
            coordinate
        ));
        assert!(!receipt_matches_request(
            &parsed_receipt(buyer, "30402:developer:other-game"),
            buyer,
            coordinate
        ));
    }
}
