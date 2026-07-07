use axum::extract::{Multipart, State};
use axum::http::HeaderMap;
use axum::Json;
use serde::Serialize;
use sha2::{Digest, Sha256};

use adp_core::{decode_authorization_header, verify_nip98, AdpListing};
use nostr_sdk::PublicKey;

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

use super::game::parse_coordinate;

#[derive(Serialize)]
pub struct UploadResponse {
    game_coordinate: String,
    file_hash: String,
    download_url: String,
}

fn required_file_hash(listing: &AdpListing) -> ApiResult<&str> {
    listing
        .file_hash
        .as_deref()
        .ok_or_else(|| ApiError::BadRequest("listing is missing required file_hash tag".into()))
}

fn active_delegated_uploader(
    listing: &AdpListing,
    uploader: PublicKey,
    now: u64,
) -> Option<PublicKey> {
    let listing_created_at = listing.event.created_at.as_secs();
    listing
        .fulfillment_delegations
        .iter()
        .find(|d| d.pubkey == uploader && d.authorizes_at(now, listing_created_at))
        .map(|d| d.pubkey)
}

/// `POST /upload` — developer- or fulfillment-key-authenticated. See ADP-01.md.
pub async fn upload_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> ApiResult<Json<UploadResponse>> {
    let auth_header = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| ApiError::Unauthorized("missing Authorization header".into()))?;

    let token_event = decode_authorization_header(auth_header)?;
    let expected_url = format!("{}/upload", state.config.server_url);
    let verified = verify_nip98(&token_event, "POST", &expected_url)?;

    let mut listing_event_json: Option<String> = None;
    let mut file_bytes: Option<Vec<u8>> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::BadRequest(format!("malformed multipart body: {e}")))?
    {
        match field.name() {
            Some("listing_event") => {
                listing_event_json = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| ApiError::BadRequest(e.to_string()))?,
                );
            }
            Some("file") => {
                file_bytes = Some(
                    field
                        .bytes()
                        .await
                        .map_err(|e| ApiError::BadRequest(e.to_string()))?
                        .to_vec(),
                );
            }
            _ => {}
        }
    }

    let listing_event_json = listing_event_json
        .ok_or_else(|| ApiError::BadRequest("missing listing_event field".into()))?;
    let file_bytes = file_bytes.ok_or_else(|| ApiError::BadRequest("missing file field".into()))?;

    let listing_event: nostr_sdk::Event = serde_json::from_str(&listing_event_json)
        .map_err(|_| ApiError::BadRequest("malformed listing_event JSON".into()))?;
    if listing_event.verify().is_err() {
        return Err(ApiError::BadRequest(
            "invalid listing_event signature".into(),
        ));
    }

    let submitted_listing = AdpListing::from_event(listing_event)?;
    let coordinate = submitted_listing.coordinate();
    let (developer_pubkey, d_tag) = parse_coordinate(&coordinate)?;
    let listing_event = state
        .relay
        .fetch_listing(developer_pubkey, &d_tag)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or_else(|| ApiError::NotFound("listing not found on relays".into()))?;
    if listing_event.verify().is_err() {
        return Err(ApiError::BadRequest(
            "invalid relay listing signature".into(),
        ));
    }
    let listing = AdpListing::from_event(listing_event)?;
    if listing.coordinate() != coordinate {
        return Err(ApiError::Internal(
            "relay listing does not match the submitted game coordinate".into(),
        ));
    }

    // Uploader must be the developer, or a currently-authorized fulfillment key this
    // operator actually holds for the listing.
    let now = nostr_sdk::Timestamp::now().as_secs();
    if verified.pubkey != listing.developer_pubkey {
        let delegated_pubkey = active_delegated_uploader(&listing, verified.pubkey, now)
            .ok_or_else(|| {
                ApiError::Unauthorized(
                    "uploader is not the developer and not an active fulfillment delegate".into(),
                )
            })?;
        let held = state
            .keystore
            .key_for_fulfillment_pubkey(&delegated_pubkey.to_hex())
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?
            .is_some();
        if !held {
            return Err(ApiError::Unauthorized(
                "uploader fulfillment key is not held by this operator".into(),
            ));
        }
    }

    let computed_hash = hex::encode(Sha256::digest(&file_bytes));
    let expected_hash = required_file_hash(&listing)?;
    if expected_hash != computed_hash {
        return Err(ApiError::BadRequest(
            "uploaded file's SHA-256 does not match the listing's file_hash tag".into(),
        ));
    }

    tokio::fs::create_dir_all(&state.config.files_dir)
        .await
        .map_err(|e| ApiError::Internal(format!("failed to create files dir: {e}")))?;
    let file_path = format!("{}/{}", state.config.files_dir, computed_hash);
    tokio::fs::write(&file_path, &file_bytes)
        .await
        .map_err(|e| ApiError::Internal(format!("failed to store file: {e}")))?;

    let coordinate = listing.coordinate();
    state
        .storage
        .record_file(&computed_hash, &coordinate, &file_path)
        .await?;

    Ok(Json(UploadResponse {
        download_url: format!("{}/game/{}", state.config.server_url, coordinate),
        game_coordinate: coordinate,
        file_hash: computed_hash,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr_sdk::{EventBuilder, Keys, Kind, Tag, Timestamp};

    fn tag(values: Vec<&str>) -> Tag {
        Tag::parse(values).expect("valid tag")
    }

    fn listing(tags: Vec<Tag>, created_at: u64) -> AdpListing {
        let developer = Keys::generate();
        let mut all_tags = vec![tag(vec!["d", "game-1"])];
        all_tags.extend(tags);
        let event = EventBuilder::new(Kind::Custom(30402), "")
            .tags(all_tags)
            .custom_created_at(Timestamp::from(created_at))
            .sign_with_keys(&developer)
            .expect("sign listing");
        AdpListing::from_event(event).expect("parse listing")
    }

    #[test]
    fn required_file_hash_rejects_missing_tag() {
        let listing = listing(vec![], 1_700_000_000);

        assert!(required_file_hash(&listing).is_err());
    }

    #[test]
    fn required_file_hash_returns_declared_hash() {
        let listing = listing(vec![tag(vec!["file_hash", "abc123"])], 1_700_000_000);

        assert_eq!(required_file_hash(&listing).expect("file hash"), "abc123");
    }

    #[test]
    fn active_delegated_uploader_candidate_uses_backdating_clamp() {
        let uploader = Keys::generate().public_key();
        let listing = listing(
            vec![tag(vec![
                "fulfillment_pubkey",
                &uploader.to_hex(),
                "1500000000",
                "1600000000",
            ])],
            1_700_000_000,
        );

        assert_eq!(
            active_delegated_uploader(&listing, uploader, 1_650_000_000),
            Some(uploader)
        );
    }

    #[test]
    fn active_delegated_uploader_rejects_unlisted_key() {
        let uploader = Keys::generate().public_key();
        let listing = listing(vec![], 1_700_000_000);

        assert_eq!(
            active_delegated_uploader(&listing, uploader, 1_700_000_001),
            None
        );
    }
}
