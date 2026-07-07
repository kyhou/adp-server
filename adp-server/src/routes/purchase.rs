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
use crate::lnurl::resolve_lud16_lsp_pubkey;
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

fn expected_amount_msat(listing: &AdpListing) -> Option<u64> {
    let (amount, currency) = listing.price.as_ref()?;
    match currency.to_ascii_uppercase().as_str() {
        "SAT" | "SATS" => parse_decimal_units(amount, 1_000, 0),
        "BTC" => parse_decimal_units(amount, 100_000_000_000, 8),
        _ => None,
    }
}

fn parse_decimal_units(amount: &str, units_per_whole: u64, max_decimals: usize) -> Option<u64> {
    let (whole, fractional) = amount.split_once('.').unwrap_or((amount, ""));
    if whole.is_empty() || fractional.len() > max_decimals {
        return None;
    }
    let whole = whole.parse::<u64>().ok()?;
    let mut total = whole.checked_mul(units_per_whole)?;
    if !fractional.is_empty() {
        let fractional_value = fractional.parse::<u64>().ok()?;
        let scale = 10_u64.checked_pow(fractional.len() as u32)?;
        total = total.checked_add(fractional_value.checked_mul(units_per_whole)? / scale)?;
    }
    Some(total)
}

/// `POST /purchase/confirm` — buyer-authenticated. Verifies payment proof, then
/// signs and publishes a `kind:1020` NIP-102 receipt with a provisioned fulfillment
/// key this operator currently holds. See ADP-01.md `POST /purchase/confirm`.
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

    // This operator must hold one of the listing's currently-authorized delegated
    // fulfillment keys. Direct developer signing is outside this hosted server.
    let now = now_secs() as u64;
    let listing_created_at = listing.event.created_at.as_secs();
    let mut signing_keys = None;
    for delegation in &listing.fulfillment_delegations {
        if !delegation.authorizes_at(now, listing_created_at) {
            continue;
        }
        if let Some(keys) = state
            .keystore
            .key_for_fulfillment_pubkey(&delegation.pubkey.to_hex())
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?
        {
            signing_keys = Some(keys);
            break;
        }
    }
    let signing_keys = signing_keys.ok_or_else(|| {
        ApiError::Internal("no held key currently authorized for this listing".into())
    })?;

    let buyer_pubkey_hex = verified.pubkey.to_hex();
    let expected_amount_msat = expected_amount_msat(&listing);

    let (payment_proof_event_id, bolt11, preimage) = if let Some(zap) = &body.zap_receipt_event {
        let lud16 = listing.lud16.as_deref().ok_or_else(|| {
            ApiError::PaymentRequired(
                "listing has no lud16 tag for zap receipt LSP verification".into(),
            )
        })?;
        let lsp_pubkey = resolve_lud16_lsp_pubkey(lud16, state.config.allow_insecure_lnurl)
            .await
            .map_err(|e| ApiError::PaymentRequired(e.to_string()))?;
        verify_zap_receipt(zap, &buyer_pubkey_hex, &body.game_coordinate, &lsp_pubkey)
            .map_err(|e| ApiError::PaymentRequired(e.to_string()))?;
        (Some(zap.id.to_hex()), None, None)
    } else if let (Some(bolt11), Some(preimage)) = (&body.bolt11, &body.preimage) {
        // Only sats/BTC listings have self-contained msat expectations here. Fiat
        // prices need exchange-rate policy outside this protocol server.
        verify_bolt11_preimage(bolt11, preimage, expected_amount_msat)
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
    let (amount, currency) = listing
        .price
        .clone()
        .unwrap_or_else(|| ("0".to_string(), "SATS".to_string()));

    let content = ReceiptContent {
        order_id: order_id.clone(),
        items: vec![ReceiptItem {
            product_id: listing.d_tag.clone(),
            name: listing.d_tag.clone(),
            quantity: 1,
            unit_price: amount.clone(),
            currency: currency.clone(),
        }],
        subtotal: amount.clone(),
        tax: "0".to_string(),
        shipping: "0".to_string(),
        total: amount.clone(),
        currency: currency.clone(),
        notes: Some("Digital delivery via ADP.".to_string()),
        fulfilled_at: now,
    };
    let plaintext = serde_json::to_string(&content).map_err(adp_core::AdpError::from)?;
    let encrypted_content = encrypt_receipt_content(&signing_keys, &verified.pubkey, &plaintext)?;

    let params = BuildReceiptParams {
        listing: &listing,
        buyer_pubkey: verified.pubkey,
        payment_proof_event_id: payment_proof_event_id.clone(),
        bolt11,
        preimage,
        amount,
        currency,
    };

    let builder = build_receipt_event_builder(&order_id, &params, encrypted_content)?;
    let receipt: Event = builder
        .sign_with_keys(&signing_keys)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn listing_with_price(amount: &str, currency: &str) -> AdpListing {
        let developer = nostr_sdk::Keys::generate();
        let event = nostr_sdk::EventBuilder::new(nostr_sdk::Kind::Custom(30402), "")
            .tags(vec![
                nostr_sdk::Tag::parse(vec!["d", "game-1"]).expect("valid d tag"),
                nostr_sdk::Tag::parse(vec!["price", amount, currency]).expect("valid price tag"),
            ])
            .sign_with_keys(&developer)
            .expect("sign listing");
        AdpListing::from_event(event).expect("parse listing")
    }

    #[test]
    fn sats_price_resolves_to_expected_msats() {
        let listing = listing_with_price("21", "SATS");

        assert_eq!(expected_amount_msat(&listing), Some(21_000));
    }

    #[test]
    fn btc_price_resolves_to_expected_msats() {
        let listing = listing_with_price("0.00000021", "BTC");

        assert_eq!(expected_amount_msat(&listing), Some(21_000));
    }

    #[test]
    fn fiat_price_does_not_resolve_to_msats() {
        let listing = listing_with_price("9.99", "USD");

        assert_eq!(expected_amount_msat(&listing), None);
    }
}
