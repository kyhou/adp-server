use super::auth::Authenticated;
use axum::extract::State;
use axum::Json;
use nostr_sdk::Event;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use adp_core::{
    build_receipt_event_builder, encrypt_receipt_content, new_order_id, verify_bolt11_preimage,
    verify_zap_receipt, AdpListing, BuildReceiptParams, ReceiptContent, ReceiptItem, ReceiptProof,
    VerifiedPaymentProof,
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
    #[serde(default)]
    quote_id: Option<String>,
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
    Authenticated(buyer): Authenticated,
    Json(body): Json<ConfirmRequest>,
) -> ApiResult<Json<serde_json::Value>> {
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
    let listing_event = match state.relay.lookup_listing(developer_pubkey, &d_tag).await {
        crate::relay::RelayLookup::Found(event) => event,
        crate::relay::RelayLookup::ConfirmedAbsent { .. } => {
            return Err(ApiError::NotFound("listing not found on relays".into()))
        }
        crate::relay::RelayLookup::Unavailable { .. } => {
            return Err(ApiError::ServiceUnavailable(
                "listing lookup unavailable".into(),
            ))
        }
    };
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
    if !listing.distributes_via(state.relay.server_url()) {
        return Err(ApiError::DistributionUnauthorized(
            "this server is not named in the current listing".into(),
        ));
    }

    if let Some(quote_id) = body.quote_id.as_deref() {
        let stored = state
            .storage
            .settlement_quote(quote_id)
            .await?
            .ok_or_else(|| ApiError::NotFound("quote not found".into()))?;
        if stored.buyer_pubkey != buyer.to_hex() || stored.listing_event.id != listing.event.id {
            return Err(ApiError::Forbidden(
                "quote does not bind this buyer and listing".into(),
            ));
        }
        let receipt = stored
            .receipt
            .ok_or_else(|| ApiError::Conflict("quote is not completely settled".into()))?;
        state
            .storage
            .record_receipt_publish_attempt(quote_id)
            .await?;
        state.relay.publish(&receipt).await.map_err(|error| {
            ApiError::ServiceUnavailable(format!("failed to republish receipt: {error}"))
        })?;
        let token = Uuid::new_v4().to_string();
        let expires_at = now_secs() + state.config.download_token_ttl_secs;
        state
            .storage
            .issue_attributed_download_token(
                &token,
                &body.game_coordinate,
                &stored.buyer_pubkey,
                expires_at,
                "purchase",
                &receipt.id.to_hex(),
            )
            .await?;
        return Ok(Json(json!({
            "receipt": receipt, "download_token": token,
            "token_expires_at": expires_at
        })));
    }

    let now = now_secs() as u64;
    let (signing_keys, authorization) = super::entitlement::select_signing_authority(
        &state,
        &listing,
        developer_pubkey,
        &body.game_coordinate,
        adp_core::CAPABILITY_ISSUE_RECEIPT,
        now,
    )
    .await?;

    let buyer_pubkey_hex = buyer.to_hex();
    let expected_amount_msat = expected_amount_msat(&listing);

    let mut bindings: Vec<VerifiedPaymentProof> = Vec::new();
    let mut proofs = Vec::new();
    let mut replay_keys = Vec::new();
    if let Some(zap) = &body.zap_receipt_event {
        let lud16 = listing.lud16.as_deref().ok_or_else(|| {
            ApiError::PaymentRequired(
                "listing has no lud16 tag for zap receipt LSP verification".into(),
            )
        })?;
        let lsp_pubkey = resolve_lud16_lsp_pubkey(lud16, state.config.allow_insecure_lnurl)
            .await
            .map_err(|e| ApiError::PaymentRequired(e.to_string()))?;
        let binding =
            verify_zap_receipt(zap, &buyer_pubkey_hex, &body.game_coordinate, &lsp_pubkey)
                .map_err(|e| ApiError::PaymentRequired(e.to_string()))?;
        if expected_amount_msat.is_some_and(|expected| binding.amount_msat < expected) {
            return Err(ApiError::PaymentRequired(
                "zap amount is below the listing price".into(),
            ));
        }
        bindings.push(binding);
        proofs.push(ReceiptProof::Zap(zap.id));
        replay_keys.push(zap.id.to_hex());
    }
    match (&body.bolt11, &body.preimage) {
        (Some(bolt11), Some(preimage)) => {
            let binding = verify_bolt11_preimage(bolt11, preimage, expected_amount_msat)
                .map_err(|e| ApiError::PaymentRequired(e.to_string()))?;
            replay_keys.push(preimage.clone());
            bindings.push(binding);
            proofs.push(ReceiptProof::Bolt11Preimage);
        }
        (None, None) => {}
        _ => {
            return Err(ApiError::BadRequest(
                "bolt11 and preimage must be supplied together".into(),
            ))
        }
    }
    if bindings.is_empty() {
        return Err(ApiError::BadRequest(
            "no valid payment proof provided (zap_receipt_event, or bolt11+preimage)".into(),
        ));
    }
    let binding = consistent_payment_binding(&bindings)?;

    if state
        .storage
        .payment_proofs_already_used(&replay_keys)
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
        bolt11: body.bolt11.clone(),
        preimage: body.preimage.clone(),
        notes: Some("Digital delivery via ADP.".to_string()),
        settled_at: binding.settled_at.unwrap_or(now),
    };
    let plaintext = serde_json::to_string(&content).map_err(adp_core::AdpError::from)?;
    let encrypted_content = encrypt_receipt_content(&signing_keys, &buyer, &plaintext)?;

    let params = BuildReceiptParams {
        listing: &listing,
        buyer_pubkey: buyer,
        authorization,
        payment_hash: binding.payment_hash,
        amount_msat: binding.amount_msat,
        settled_at: content.settled_at,
        proofs,
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
        .map_err(|e| ApiError::ServiceUnavailable(format!("failed to publish receipt: {e}")))?;

    state
        .storage
        .record_purchase_v3(
            &order_id,
            &replay_keys,
            &buyer_pubkey_hex,
            &body.game_coordinate,
            &receipt,
        )
        .await?;

    let token = Uuid::new_v4().to_string();
    let expires_at = now_secs() + state.config.download_token_ttl_secs;
    state
        .storage
        .issue_attributed_download_token(
            &token,
            &body.game_coordinate,
            &buyer_pubkey_hex,
            expires_at,
            "purchase",
            &receipt.id.to_hex(),
        )
        .await?;

    Ok(Json(json!({
        "receipt": receipt,
        "download_token": token,
        "token_expires_at": expires_at,
    })))
}

fn consistent_payment_binding(
    bindings: &[VerifiedPaymentProof],
) -> ApiResult<VerifiedPaymentProof> {
    let binding = bindings
        .first()
        .cloned()
        .ok_or_else(|| ApiError::BadRequest("no payment proof supplied".into()))?;
    if bindings.iter().any(|candidate| {
        candidate.payment_hash != binding.payment_hash
            || candidate.amount_msat != binding.amount_msat
    }) {
        return Err(ApiError::PaymentRequired(
            "supplied payment proofs are contradictory".into(),
        ));
    }
    Ok(binding)
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

    #[test]
    fn mixed_proofs_must_have_identical_public_bindings() {
        let first = VerifiedPaymentProof {
            payment_hash: "11".repeat(32),
            amount_msat: 1_000,
            settled_at: Some(100),
        };
        let mut contradictory = first.clone();
        contradictory.amount_msat = 2_000;
        assert!(consistent_payment_binding(&[first.clone(), first]).is_ok());
        assert!(matches!(
            consistent_payment_binding(&[
                contradictory,
                VerifiedPaymentProof {
                    payment_hash: "11".repeat(32),
                    amount_msat: 1_000,
                    settled_at: None,
                }
            ]),
            Err(ApiError::PaymentRequired(_))
        ));
    }
}
