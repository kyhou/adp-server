use axum::extract::State;
use axum::http::HeaderMap;
use axum::Json;
use nostr_sdk::Event;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use adp_core::{
    build_receipt_event_builder, decode_authorization_header, encrypt_receipt_content,
    new_order_id, verify_bolt11_preimage, verify_nip98, verify_zap_receipt, AdpListing,
    BuildReceiptParams, ReceiptContent, ReceiptItem,
};

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

#[derive(Deserialize)]
pub struct ConfirmRequest {
    game_coordinate: String,
    listing_event: Event,
    zap_receipt_event: Option<Event>,
    bolt11: Option<String>,
    preimage: Option<String>,
}

use super::game::parse_coordinate;

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_secs() as i64
}

/// `POST /purchase/confirm` — buyer-authenticated. Verifies payment proof, then
/// signs and publishes a `kind:1020` NIP-102 receipt with the server's fulfillment
/// key. See ADP-01.md `POST /purchase/confirm`.
pub async fn confirm_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ConfirmRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let auth_header = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| ApiError::Unauthorized("missing Authorization header".into()))?;
    let token_event = decode_authorization_header(auth_header)?;
    let expected_url = format!("{}/purchase/confirm", state.config.server_url);
    let verified = verify_nip98(&token_event, "POST", &expected_url)?;

    if body.listing_event.verify().is_err() {
        return Err(ApiError::BadRequest(
            "invalid listing_event signature".into(),
        ));
    }
    let submitted_listing = AdpListing::from_event(body.listing_event.clone())?;
    if submitted_listing.coordinate() != body.game_coordinate {
        return Err(ApiError::BadRequest(
            "listing_event does not match the claimed game_coordinate".into(),
        ));
    }

    let (developer_pubkey, d_tag) = parse_coordinate(&body.game_coordinate)?;
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
    if listing.coordinate() != body.game_coordinate {
        return Err(ApiError::Internal(
            "relay listing does not match the requested game coordinate".into(),
        ));
    }

    // This server's fulfillment key must currently be authorized to fulfill this
    // listing — either as the developer's own key, or an active delegation.
    let now = now_secs() as u64;
    let server_pubkey = state.fulfillment_keys.public_key();
    let server_authorized = listing.developer_pubkey == server_pubkey
        || listing
            .fulfillment_delegations
            .iter()
            .any(|d| d.pubkey == server_pubkey && d.authorizes_at(now));
    if !server_authorized {
        return Err(ApiError::Internal(
            "this server's fulfillment key is not currently authorized for this listing \
             (misconfiguration — the developer may have revoked or never delegated to it)"
                .into(),
        ));
    }

    let buyer_pubkey_hex = verified.pubkey.to_hex();

    let (payment_proof_event_id, bolt11, preimage) = if let Some(zap) = &body.zap_receipt_event {
        verify_zap_receipt(zap, &buyer_pubkey_hex, &body.game_coordinate)
            .map_err(|e| ApiError::PaymentRequired(e.to_string()))?;
        (Some(zap.id.to_hex()), None, None)
    } else if let (Some(bolt11), Some(preimage)) = (&body.bolt11, &body.preimage) {
        verify_bolt11_preimage(bolt11, preimage, None)
            .map_err(|e| ApiError::PaymentRequired(e.to_string()))?;
        (None, Some(bolt11.clone()), Some(preimage.clone()))
    } else {
        return Err(ApiError::BadRequest(
            "no valid payment proof provided (zap_receipt_event, or bolt11+preimage)".into(),
        ));
    };

    // Replay protection key: the zap receipt's event id, or the preimage itself when
    // using the self-contained path (a preimage can only be revealed once payment
    // settles, so it is equally suitable as a one-time replay key).
    let replay_key = payment_proof_event_id
        .clone()
        .or_else(|| preimage.clone())
        .ok_or_else(|| ApiError::Internal("no replay key derived from payment proof".into()))?;

    if state
        .storage
        .payment_proof_already_used(&replay_key)
        .await?
    {
        return Err(ApiError::Conflict("payment proof already used".into()));
    }

    let order_id = new_order_id();

    let content = ReceiptContent {
        order_id: order_id.clone(),
        items: vec![ReceiptItem {
            product_id: listing.d_tag.clone(),
            name: listing.d_tag.clone(),
            quantity: 1,
            unit_price: "0".to_string(),
            currency: "SATS".to_string(),
        }],
        subtotal: "0".to_string(),
        tax: "0".to_string(),
        shipping: "0".to_string(),
        total: "0".to_string(),
        currency: "SATS".to_string(),
        notes: Some("Digital delivery via ADP.".to_string()),
        fulfilled_at: now,
    };
    let plaintext = serde_json::to_string(&content).map_err(adp_core::AdpError::from)?;
    let encrypted_content =
        encrypt_receipt_content(&state.fulfillment_keys, &verified.pubkey, &plaintext)?;

    let params = BuildReceiptParams {
        listing: &listing,
        buyer_pubkey: verified.pubkey,
        payment_proof_event_id: payment_proof_event_id.clone(),
        bolt11,
        preimage,
        // NOTE: reference implementation does not resolve the listing's declared
        // price into an amount/currency pair. A production deployment should read
        // this from the listing's NIP-99 price tag(s).
        amount: "0".to_string(),
        currency: "SATS".to_string(),
    };

    let builder = build_receipt_event_builder(&order_id, &params, encrypted_content)?;
    let receipt: Event = builder
        .sign_with_keys(&state.fulfillment_keys)
        .map_err(|e| ApiError::Internal(format!("failed to sign receipt: {e}")))?;

    state
        .relay
        .publish(&receipt)
        .await
        .map_err(|e| ApiError::Internal(format!("failed to publish receipt: {e}")))?;

    state
        .storage
        .record_purchase(
            &order_id,
            &replay_key,
            &buyer_pubkey_hex,
            &body.game_coordinate,
            &receipt.id.to_hex(),
        )
        .await?;

    let token = Uuid::new_v4().to_string();
    let expires_at = now_secs() + state.config.download_token_ttl_secs;
    state
        .storage
        .issue_download_token(&token, &body.game_coordinate, &buyer_pubkey_hex, expires_at)
        .await?;

    Ok(Json(json!({
        "receipt": receipt,
        "download_token": token,
        "token_expires_at": expires_at,
    })))
}
