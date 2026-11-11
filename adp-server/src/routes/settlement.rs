use std::collections::{HashMap, HashSet};

use super::auth::Authenticated;
use crate::settlement_clock::SettlementClock;
use adp_core::{
    build_aggregate_receipt_event_builder, build_payment_legs, encrypt_receipt_content,
    parse_gross_msat, parse_terms_event, verify_bolt11_preimage, AdpListing, AggregateProofLeg,
    BuildAggregateReceiptParams, InvoicePreflight, QuoteState, RefundDestination, SignedQuote,
    TermsPayload, QUOTE_KIND,
};
use axum::extract::{Path, State};
use axum::Extension;
use axum::Json;
use nostr_sdk::{Event, EventBuilder, Kind, PublicKey};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::crypto::{EncryptedSecret, MasterCipher};
use crate::error::{ApiError, ApiResult};
use crate::lnurl::{request_exact_invoice, ZapRequestContext};
use crate::state::AppState;
use crate::storage::{NewSettlementAttempt, NewSettlementTerm, StoredSettlementQuote};

#[derive(Debug, Deserialize)]
pub struct CreateQuoteRequest {
    pub purchase_id: String,
    pub listing_event: Event,
    #[serde(default)]
    pub terms_events: Vec<Event>,
    pub refund_destination: RefundDestination,
    #[serde(default)]
    pub zap_requests: HashMap<String, Event>,
}

#[derive(Debug, Deserialize)]
pub struct SubmitProofRequest {
    pub attempt_id: String,
    pub preimage: String,
}

#[derive(Debug, Deserialize)]
pub struct ReplaceRefundDestinationRequest {
    pub destination: RefundDestination,
}

#[derive(Debug, Deserialize)]
pub struct SubmitRecoveryRequest {
    pub event: Event,
    pub plaintext: String,
}

#[derive(Debug, Deserialize)]
pub struct ConfirmRefundRequest {
    pub attempt_id: String,
    pub preimage: String,
}

#[derive(Debug, Serialize)]
struct InvoiceResponse {
    quote_id: String,
    attempt_id: String,
    leg_id: String,
    amount_msat: u64,
    payment_hash: String,
    bolt11: String,
    expires_at: u64,
}

fn cipher(state: &AppState) -> Result<MasterCipher, ApiError> {
    MasterCipher::new(&state.config.master_key)
        .map_err(|error| ApiError::Internal(format!("invalid settlement cipher: {error}")))
}

fn encrypt_private(state: &AppState, plaintext: &[u8]) -> Result<Vec<u8>, ApiError> {
    serde_json::to_vec(&cipher(state)?.encrypt(plaintext))
        .map_err(|error| ApiError::Internal(format!("failed to encode ciphertext: {error}")))
}

fn decrypt_private(state: &AppState, encrypted: &[u8]) -> Result<Vec<u8>, ApiError> {
    let envelope: EncryptedSecret = serde_json::from_slice(encrypted)
        .map_err(|_| ApiError::Internal("corrupt settlement ciphertext".into()))?;
    cipher(state)?
        .decrypt(&envelope)
        .map_err(|_| ApiError::Internal("cannot decrypt settlement data".into()))
}

fn decrypt_quote(
    state: &AppState,
    stored: &StoredSettlementQuote,
) -> Result<SignedQuote, ApiError> {
    serde_json::from_slice(&decrypt_private(state, &stored.encrypted_quote)?)
        .map_err(|_| ApiError::Internal("corrupt settlement quote".into()))
}

async fn current_listing(state: &AppState, submitted: Event) -> ApiResult<AdpListing> {
    if submitted.verify().is_err() {
        return Err(ApiError::BadRequest("invalid listing signature".into()));
    }
    let parsed = AdpListing::from_event(submitted)?;
    let relay = match state
        .relay
        .lookup_listing(parsed.developer_pubkey, &parsed.d_tag)
        .await
    {
        crate::relay::RelayLookup::Found(event) => event,
        crate::relay::RelayLookup::ConfirmedAbsent { .. } => {
            return Err(ApiError::NotFound("listing not found".into()))
        }
        crate::relay::RelayLookup::Unavailable { .. } => {
            return Err(ApiError::ServiceUnavailable(
                "listing lookup unavailable".into(),
            ))
        }
    };
    if relay.id != parsed.event.id || relay.verify().is_err() {
        return Err(ApiError::Conflict(
            "quote requires the exact current listing snapshot".into(),
        ));
    }
    if !parsed.distributes_via(state.relay.server_url()) {
        return Err(ApiError::DistributionUnauthorized(
            "processor is not authorized by the listing".into(),
        ));
    }
    Ok(parsed)
}

pub async fn create_quote_handler(
    State(state): State<AppState>,
    Extension(clock): Extension<SettlementClock>,
    Authenticated(buyer): Authenticated,
    Json(body): Json<CreateQuoteRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    if body.purchase_id.is_empty() || body.purchase_id.len() > 128 {
        return Err(ApiError::BadRequest("invalid purchase_id".into()));
    }
    if let Some(stored) = state.storage.settlement_quote(&body.purchase_id).await? {
        if stored.buyer_pubkey != buyer.to_hex() || stored.listing_event.id != body.listing_event.id
        {
            return Err(ApiError::Conflict("purchase identity already used".into()));
        }
        return Ok(Json(
            json!({ "quote": stored.quote_event, "state": stored.state }),
        ));
    }
    let listing = current_listing(&state, body.listing_event).await?;
    if listing.malformed_accepted_terms > 0 || listing.accepted_terms.is_empty() {
        return Err(ApiError::BadRequest(
            "split listing has incomplete terms coverage".into(),
        ));
    }
    let (amount, currency) = listing
        .price
        .as_ref()
        .ok_or_else(|| ApiError::BadRequest("split purchase requires a price".into()))?;
    let gross_msat = parse_gross_msat(amount, currency)
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    let developer_destination = listing.lud16.as_deref().ok_or_else(|| {
        ApiError::BadRequest("listing has no developer payment destination".into())
    })?;

    let accepted: HashMap<_, _> = listing
        .accepted_terms
        .iter()
        .map(|reference| {
            (
                reference.operator_pubkey.as_str(),
                reference.terms_event_id.as_str(),
            )
        })
        .collect();
    if accepted.len() != listing.accepted_terms.len() || body.terms_events.len() != accepted.len() {
        return Err(ApiError::BadRequest(
            "terms snapshot does not cover every operator".into(),
        ));
    }
    let mut seen = HashSet::new();
    let mut terms = Vec::new();
    let mut operator_terms = Vec::new();
    for event in body.terms_events {
        let TermsPayload::Offer { terms: offer } =
            parse_terms_event(&event).map_err(|error| ApiError::BadRequest(error.to_string()))?
        else {
            return Err(ApiError::BadRequest(
                "withdrawal is not an accepted terms root".into(),
            ));
        };
        if accepted.get(offer.operator_pubkey.as_str()).copied() != Some(event.id.to_hex().as_str())
            || !seen.insert(offer.operator_pubkey.clone())
        {
            return Err(ApiError::BadRequest(
                "terms event does not match listing consent".into(),
            ));
        }
        operator_terms.push((offer.clone(), event.id.to_hex()));
        terms.push(NewSettlementTerm {
            encrypted_destination: encrypt_private(&state, offer.payment_destination.as_bytes())?,
            event,
            terms: offer,
        });
    }
    if !seen.contains(&state.operator_keys.public_key().to_hex()) {
        return Err(ApiError::Forbidden(
            "this processor has no accepted terms".into(),
        ));
    }
    let legs = build_payment_legs(
        gross_msat,
        &listing.developer_pubkey.to_hex(),
        developer_destination,
        &operator_terms,
    )
    .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    adp_core::validate_refund_destination(&body.refund_destination, &legs)
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;

    let issued_at = clock.now_secs()?;
    let mut quote = SignedQuote::new(
        body.purchase_id.clone(),
        buyer.to_hex(),
        listing.event.id.to_hex(),
        listing.coordinate(),
        state.operator_keys.public_key().to_hex(),
        gross_msat,
        issued_at,
        &body.refund_destination,
        legs,
    )
    .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    quote.state = QuoteState::Preflighting;
    let mut preflights = Vec::new();
    let mut attempts = Vec::new();
    let mut encrypted_destinations = Vec::new();
    for leg in &quote.legs {
        let recipient = PublicKey::from_hex(&leg.recipient_pubkey)
            .map_err(|_| ApiError::BadRequest("invalid leg recipient".into()))?;
        let zap = body.zap_requests.get(&leg.leg_id);
        let exact = request_exact_invoice(
            &leg.destination,
            leg.amount_msat,
            buyer,
            recipient,
            zap,
            zap.map(|_| ZapRequestContext {
                listing_coordinate: &quote.listing_coordinate,
                quote_id: &quote.quote_id,
                leg_id: &leg.leg_id,
            }),
            quote.confirmation_expires_at,
            state.config.allow_insecure_lnurl,
        )
        .await
        .map_err(|error| ApiError::PaymentRequired(error.to_string()))?;
        preflights.push(InvoicePreflight {
            leg_id: leg.leg_id.clone(),
            recipient_pubkey: leg.recipient_pubkey.clone(),
            amount_msat: leg.amount_msat,
            payment_hash: exact.payment_hash.clone(),
            lnurl_origin: exact.provenance.endpoint.clone(),
            callback: exact.provenance.callback.clone(),
            lsp_pubkey: exact.provenance.lsp_pubkey.clone(),
            invoice_expires_at: exact.expires_at,
        });
        attempts.push(NewSettlementAttempt {
            attempt_id: Uuid::new_v4().to_string(),
            leg_id: leg.leg_id.clone(),
            encrypted_bolt11: encrypt_private(&state, exact.bolt11.as_bytes())?,
            lnurl_origin: exact.provenance.endpoint,
            callback: exact.provenance.callback,
            lsp_pubkey: exact.provenance.lsp_pubkey,
            payment_hash: exact.payment_hash,
            amount_msat: leg.amount_msat,
            expires_at: exact.expires_at,
        });
        encrypted_destinations.push((
            leg.leg_id.clone(),
            encrypt_private(&state, leg.destination.as_bytes())?,
        ));
    }
    quote
        .activate(issued_at, &preflights)
        .map_err(|error| ApiError::PaymentRequired(error.to_string()))?;
    let quote_plaintext = serde_json::to_string(&quote)
        .map_err(|error| ApiError::Internal(format!("quote serialization failed: {error}")))?;
    let quote_ciphertext = encrypt_receipt_content(&state.operator_keys, &buyer, &quote_plaintext)?;
    let quote_event = EventBuilder::new(Kind::Custom(QUOTE_KIND), quote_ciphertext)
        .custom_created_at(nostr_sdk::Timestamp::from_secs(issued_at))
        .sign_with_keys(&state.operator_keys)
        .map_err(|error| ApiError::Internal(format!("quote signing failed: {error}")))?;
    let refund_commitment = adp_core::destination_commitment(&body.refund_destination)
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    let refund_cipher = encrypt_private(
        &state,
        serde_json::to_string(&body.refund_destination)
            .map_err(|error| ApiError::Internal(error.to_string()))?
            .as_bytes(),
    )?;
    state
        .storage
        .save_active_settlement(
            &quote,
            &quote_event,
            &listing.event,
            &encrypt_private(&state, quote_plaintext.as_bytes())?,
            &terms,
            &encrypted_destinations,
            &attempts,
            &Uuid::new_v4().to_string(),
            &refund_commitment,
            &refund_cipher,
            body.refund_destination.min_sendable_msat,
            body.refund_destination.max_sendable_msat,
        )
        .await?;
    Ok(Json(json!({ "quote": quote_event, "state": "active" })))
}

async fn authorized_quote(
    state: &AppState,
    buyer: PublicKey,
    quote_id: &str,
) -> ApiResult<StoredSettlementQuote> {
    let quote = state
        .storage
        .settlement_quote(quote_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("quote not found".into()))?;
    if quote.buyer_pubkey != buyer.to_hex() {
        return Err(ApiError::Forbidden("quote belongs to another buyer".into()));
    }
    Ok(quote)
}

pub async fn status_handler(
    State(state): State<AppState>,
    Extension(clock): Extension<SettlementClock>,
    Path(quote_id): Path<String>,
    Authenticated(buyer): Authenticated,
) -> ApiResult<Json<serde_json::Value>> {
    authorized_quote(&state, buyer, &quote_id).await?;
    let now = clock.now_secs()?;
    state
        .storage
        .process_settlement_deadlines(&quote_id, now)
        .await?;
    let status = state
        .storage
        .settlement_status(&quote_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("quote not found".into()))?;
    Ok(Json(json!({
        "quote_id": quote_id, "state": status.state, "total_legs": status.total_legs,
        "disclosed_legs": status.disclosed_legs, "settled_legs": status.settled_legs,
        "failure_at": status.failure_at, "refund_due_at": status.refund_due_at,
        "receipt_event_id": status.receipt_event_id
    })))
}

pub async fn status_post_handler(
    State(state): State<AppState>,
    Extension(clock): Extension<SettlementClock>,
    Path(quote_id): Path<String>,
    Authenticated(buyer): Authenticated,
) -> ApiResult<Json<serde_json::Value>> {
    authorized_quote(&state, buyer, &quote_id).await?;
    let now = clock.now_secs()?;
    state
        .storage
        .process_settlement_deadlines(&quote_id, now)
        .await?;
    let status = state
        .storage
        .settlement_status(&quote_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("quote not found".into()))?;
    Ok(Json(json!({
        "quote_id": quote_id, "state": status.state, "total_legs": status.total_legs,
        "disclosed_legs": status.disclosed_legs, "settled_legs": status.settled_legs,
        "failure_at": status.failure_at, "refund_due_at": status.refund_due_at,
        "receipt_event_id": status.receipt_event_id
    })))
}

pub async fn next_invoice_handler(
    State(state): State<AppState>,
    Extension(clock): Extension<SettlementClock>,
    Path(quote_id): Path<String>,
    Authenticated(buyer): Authenticated,
) -> ApiResult<Json<serde_json::Value>> {
    let stored = authorized_quote(&state, buyer, &quote_id).await?;
    let now = clock.now_secs()?;
    state
        .storage
        .process_settlement_deadlines(&quote_id, now)
        .await?;
    if stored.state != QuoteState::Active || now > stored.payment_expires_at {
        return Err(ApiError::Conflict("quote is not payable".into()));
    }
    let attempts = state.storage.settlement_attempts(&quote_id).await?;
    let mut next = attempts
        .into_iter()
        .find(|attempt| attempt.settled_at.is_none())
        .ok_or_else(|| ApiError::Conflict("all legs are already settled".into()))?;
    if next.expires_at <= now {
        let quote = decrypt_quote(&state, &stored)?;
        let leg = quote
            .legs
            .iter()
            .find(|leg| leg.leg_id == next.leg_id)
            .ok_or_else(|| ApiError::Internal("invoice leg missing from quote".into()))?;
        let recipient = PublicKey::from_hex(&leg.recipient_pubkey)
            .map_err(|_| ApiError::Internal("corrupt quote recipient".into()))?;
        let exact = request_exact_invoice(
            &leg.destination,
            leg.amount_msat,
            buyer,
            recipient,
            None,
            None,
            quote.confirmation_expires_at,
            state.config.allow_insecure_lnurl,
        )
        .await
        .map_err(|error| ApiError::PaymentRequired(error.to_string()))?;
        let replacement = NewSettlementAttempt {
            attempt_id: Uuid::new_v4().to_string(),
            leg_id: leg.leg_id.clone(),
            encrypted_bolt11: encrypt_private(&state, exact.bolt11.as_bytes())?,
            lnurl_origin: exact.provenance.endpoint,
            callback: exact.provenance.callback,
            lsp_pubkey: exact.provenance.lsp_pubkey,
            payment_hash: exact.payment_hash,
            amount_msat: leg.amount_msat,
            expires_at: exact.expires_at,
        };
        state
            .storage
            .add_replacement_attempt(&quote_id, &next.attempt_id, &replacement, now)
            .await?;
        next = crate::storage::StoredSettlementAttempt {
            attempt_id: replacement.attempt_id,
            leg_id: replacement.leg_id,
            encrypted_bolt11: replacement.encrypted_bolt11,
            payment_hash: replacement.payment_hash,
            amount_msat: replacement.amount_msat,
            expires_at: replacement.expires_at,
            disclosed_at: None,
            settled_at: None,
            proof_type: None,
            proof_reference: None,
        };
    }
    if !state
        .storage
        .disclose_attempt(&quote_id, &next.attempt_id, now)
        .await?
    {
        return Err(ApiError::Conflict("prior leg is not settled".into()));
    }
    let bolt11 = String::from_utf8(decrypt_private(&state, &next.encrypted_bolt11)?)
        .map_err(|_| ApiError::Internal("corrupt stored invoice".into()))?;
    Ok(Json(json!(InvoiceResponse {
        quote_id,
        attempt_id: next.attempt_id,
        leg_id: next.leg_id,
        amount_msat: next.amount_msat,
        payment_hash: next.payment_hash,
        bolt11,
        expires_at: next.expires_at
    })))
}

pub async fn proof_handler(
    State(state): State<AppState>,
    Extension(clock): Extension<SettlementClock>,
    Path(quote_id): Path<String>,
    Authenticated(buyer): Authenticated,
    Json(body): Json<SubmitProofRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let stored = authorized_quote(&state, buyer, &quote_id).await?;
    if let Some(receipt) = &stored.receipt {
        state
            .storage
            .record_receipt_publish_attempt(&quote_id)
            .await?;
        state.relay.publish(&receipt).await.map_err(|error| {
            ApiError::ServiceUnavailable(format!("failed to republish receipt: {error}"))
        })?;
        let quote = decrypt_quote(&state, &stored)?;
        return settled_response(&state, &quote, receipt, clock.now_secs()?).await;
    }
    let attempts = state.storage.settlement_attempts(&quote_id).await?;
    let attempt = attempts
        .iter()
        .find(|attempt| attempt.attempt_id == body.attempt_id)
        .ok_or_else(|| ApiError::NotFound("invoice attempt not found".into()))?;
    if attempt.disclosed_at.is_none() {
        return Err(ApiError::Conflict("invoice was not disclosed".into()));
    }
    let bolt11 = String::from_utf8(decrypt_private(&state, &attempt.encrypted_bolt11)?)
        .map_err(|_| ApiError::Internal("corrupt stored invoice".into()))?;
    let binding = verify_bolt11_preimage(&bolt11, &body.preimage, Some(attempt.amount_msat))
        .map_err(|error| ApiError::PaymentRequired(error.to_string()))?;
    if binding.payment_hash != attempt.payment_hash {
        return Err(ApiError::PaymentRequired("payment hash mismatch".into()));
    }
    let settled_at = clock.now_secs()?;
    if attempt.settled_at.is_none()
        && !state
            .storage
            .settle_attempt(
                &quote_id,
                &attempt.attempt_id,
                "bolt11-preimage",
                &attempt.payment_hash,
                &encrypt_private(&state, body.preimage.as_bytes())?,
                settled_at,
            )
            .await?
    {
        return Err(ApiError::Conflict(
            "payment proof was already consumed".into(),
        ));
    }
    state
        .storage
        .process_settlement_deadlines(&quote_id, settled_at)
        .await?;
    let attempts = state.storage.settlement_attempts(&quote_id).await?;
    let status = state
        .storage
        .settlement_status(&quote_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("quote not found".into()))?;
    if status.state == QuoteState::Failed {
        return Ok(Json(json!({
            "state": "failed",
            "settled_leg": attempt.leg_id,
            "failure_at": status.failure_at,
            "refund_due_at": status.refund_due_at
        })));
    }
    if attempts.iter().any(|attempt| attempt.settled_at.is_none()) {
        return Ok(Json(
            json!({ "state": "active", "settled_leg": attempt.leg_id }),
        ));
    }
    finalize_settlement(&state, &quote_id, &stored, attempts, settled_at).await
}

async fn finalize_settlement(
    state: &AppState,
    quote_id: &str,
    stored: &StoredSettlementQuote,
    attempts: Vec<crate::storage::StoredSettlementAttempt>,
    now: u64,
) -> ApiResult<Json<serde_json::Value>> {
    if now > stored.confirmation_expires_at {
        return Err(ApiError::Conflict("confirmation deadline passed".into()));
    }
    let quote = decrypt_quote(state, stored)?;
    let listing = AdpListing::from_event(stored.listing_event.clone())?;
    let developer = listing.developer_pubkey;
    let (signing_keys, authorization) = super::entitlement::select_signing_authority(
        state,
        &listing,
        developer,
        &quote.listing_coordinate,
        adp_core::CAPABILITY_ISSUE_RECEIPT,
        now,
    )
    .await?;
    let mut proof_legs = Vec::new();
    for leg in &quote.legs {
        let attempt = attempts
            .iter()
            .find(|attempt| attempt.leg_id == leg.leg_id && attempt.settled_at.is_some())
            .ok_or_else(|| ApiError::Conflict("settlement is incomplete".into()))?;
        proof_legs.push(AggregateProofLeg {
            leg_id: leg.leg_id.clone(),
            role: leg.role.clone(),
            recipient_pubkey: leg.recipient_pubkey.clone(),
            amount_msat: leg.amount_msat,
            payment_hash: attempt.payment_hash.clone(),
            proof_type: attempt
                .proof_type
                .clone()
                .ok_or_else(|| ApiError::Internal("missing proof type".into()))?,
            proof_reference: attempt
                .proof_reference
                .clone()
                .ok_or_else(|| ApiError::Internal("missing proof reference".into()))?,
            accepted_terms_id: leg.accepted_terms_id.clone(),
        });
    }
    let content = serde_json::to_string(&json!({
        "quote_id": quote_id,
        "listing_event_id": quote.listing_event_id,
        "settled_at": now,
        "payment_hashes": proof_legs.iter().map(|leg| &leg.payment_hash).collect::<Vec<_>>()
    }))
    .map_err(|error| ApiError::Internal(error.to_string()))?;
    let buyer = PublicKey::from_hex(&quote.buyer_pubkey)
        .map_err(|_| ApiError::Internal("corrupt quote buyer".into()))?;
    let encrypted = encrypt_receipt_content(&signing_keys, &buyer, &content)?;
    let params = BuildAggregateReceiptParams {
        listing: &listing,
        buyer_pubkey: buyer,
        authorization,
        quote_event_id: stored.quote_event.id,
        processor_pubkey: state.operator_keys.public_key(),
        processor_url: Some(&state.config.server_url),
        gross_msat: quote.gross_msat,
        settled_at: now,
        legs: &proof_legs,
    };
    let receipt = build_aggregate_receipt_event_builder(quote_id, &params, encrypted)?
        .sign_with_keys(&signing_keys)
        .map_err(|error| ApiError::Internal(format!("receipt signing failed: {error}")))?;
    let proof_keys = proof_legs
        .iter()
        .map(|leg| leg.payment_hash.clone())
        .collect::<Vec<_>>();
    state
        .storage
        .store_settlement_receipt(
            quote_id,
            quote_id,
            &proof_keys,
            &quote.buyer_pubkey,
            &quote.listing_coordinate,
            &receipt,
        )
        .await?;
    state
        .storage
        .record_receipt_publish_attempt(quote_id)
        .await?;
    state.relay.publish(&receipt).await.map_err(|error| {
        ApiError::ServiceUnavailable(format!("receipt persisted but publication failed: {error}"))
    })?;
    settled_response(state, &quote, &receipt, now).await
}

// Both initial settlement and publication recovery return a usable capability.
// Ownership comes from the durable authenticated quote, never retry parameters.
async fn settled_response(
    state: &AppState,
    quote: &SignedQuote,
    receipt: &Event,
    now: u64,
) -> ApiResult<Json<serde_json::Value>> {
    let token = Uuid::new_v4().to_string();
    let expires_at = i64::try_from(now)
        .ok()
        .and_then(|value| value.checked_add(state.config.download_token_ttl_secs))
        .ok_or_else(|| ApiError::Internal("download token expiry overflow".into()))?;
    state
        .storage
        .issue_attributed_download_token(
            &token,
            &quote.listing_coordinate,
            &quote.buyer_pubkey,
            expires_at,
            "purchase",
            &receipt.id.to_hex(),
        )
        .await?;
    Ok(Json(json!({
        "state": "settled", "receipt": receipt, "download_token": token,
        "token_expires_at": expires_at
    })))
}

pub async fn replace_refund_destination_handler(
    State(state): State<AppState>,
    Extension(clock): Extension<SettlementClock>,
    Path(quote_id): Path<String>,
    Authenticated(buyer): Authenticated,
    Json(body): Json<ReplaceRefundDestinationRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let stored = authorized_quote(&state, buyer, &quote_id).await?;
    let quote = decrypt_quote(&state, &stored)?;
    adp_core::validate_refund_destination(&body.destination, &quote.legs)
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    let inspected = crate::lnurl::inspect_payment_destination(
        &body.destination.endpoint,
        state.config.allow_insecure_lnurl,
    )
    .await
    .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    if inspected.min_sendable_msat > body.destination.min_sendable_msat
        || inspected.max_sendable_msat < body.destination.max_sendable_msat
    {
        return Err(ApiError::BadRequest(
            "declared refund bounds exceed LNURL endpoint bounds".into(),
        ));
    }
    let commitment = adp_core::destination_commitment(&body.destination)
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    let encrypted = encrypt_private(
        &state,
        serde_json::to_string(&body.destination)
            .map_err(|error| ApiError::Internal(error.to_string()))?
            .as_bytes(),
    )?;
    let now = clock.now_secs()?;
    state
        .storage
        .replace_refund_destination(
            &quote_id,
            &Uuid::new_v4().to_string(),
            &commitment,
            &encrypted,
            body.destination.min_sendable_msat,
            body.destination.max_sendable_msat,
            now,
        )
        .await?;
    Ok(Json(
        json!({ "quote_id": quote_id, "commitment": commitment }),
    ))
}

fn recovery_evidence(
    quote: &SignedQuote,
    quote_event_id: &str,
    failure_at: u64,
    paid_leg_ids: Vec<String>,
) -> Result<adp_core::RecoveryEvidence, ApiError> {
    let evidence_commitment = adp_core::canonical_commitment(&(
        adp_core::SETTLEMENT_VERSION,
        &quote.quote_id,
        quote_event_id,
        failure_at,
        &paid_leg_ids,
    ))
    .map_err(|error| ApiError::Internal(error.to_string()))?;
    Ok(adp_core::RecoveryEvidence {
        version: adp_core::SETTLEMENT_VERSION,
        quote_id: quote.quote_id.clone(),
        quote_event_id: quote_event_id.into(),
        buyer_pubkey: quote.buyer_pubkey.clone(),
        processor_pubkey: quote.processor_pubkey.clone(),
        failure_at,
        paid_leg_ids,
        evidence_commitment,
    })
}

pub async fn prepare_recovery_handler(
    State(state): State<AppState>,
    Extension(clock): Extension<SettlementClock>,
    Path(quote_id): Path<String>,
    Authenticated(buyer): Authenticated,
) -> ApiResult<Json<serde_json::Value>> {
    let stored = authorized_quote(&state, buyer, &quote_id).await?;
    let now = clock.now_secs()?;
    state
        .storage
        .process_settlement_deadlines(&quote_id, now)
        .await?;
    let status = state
        .storage
        .settlement_status(&quote_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("quote not found".into()))?;
    let failure_at = status
        .failure_at
        .ok_or_else(|| ApiError::Conflict("quote has not failed".into()))?;
    let quote = decrypt_quote(&state, &stored)?;
    let paid_leg_ids = state
        .storage
        .settlement_attempts(&quote_id)
        .await?
        .into_iter()
        .filter(|attempt| attempt.settled_at.is_some())
        .map(|attempt| attempt.leg_id)
        .collect::<Vec<_>>();
    let evidence = recovery_evidence(
        &quote,
        &stored.quote_event.id.to_hex(),
        failure_at,
        paid_leg_ids,
    )?;
    let plaintext =
        serde_json::to_string(&evidence).map_err(|error| ApiError::Internal(error.to_string()))?;
    if now > failure_at.saturating_add(adp_core::RECOVERY_CLAIM_GRACE_SECS) {
        return Ok(Json(json!({
            "claimant": "buyer", "kind": adp_core::RECOVERY_KIND,
            "plaintext": plaintext, "evidence": evidence
        })));
    }
    let buyer = PublicKey::from_hex(&quote.buyer_pubkey)
        .map_err(|_| ApiError::Internal("corrupt quote buyer".into()))?;
    let encrypted = encrypt_receipt_content(&state.operator_keys, &buyer, &plaintext)?;
    let event = EventBuilder::new(Kind::Custom(adp_core::RECOVERY_KIND), encrypted)
        .custom_created_at(nostr_sdk::Timestamp::from_secs(now))
        .sign_with_keys(&state.operator_keys)
        .map_err(|error| ApiError::Internal(format!("recovery signing failed: {error}")))?;
    let event = state
        .storage
        .save_recovery_claim(
            &quote_id,
            &state.operator_keys.public_key().to_hex(),
            "processor",
            &event,
            &evidence.evidence_commitment,
        )
        .await?;
    state.relay.publish(&event).await.map_err(|error| {
        ApiError::ServiceUnavailable(format!(
            "recovery claim persisted but publication failed: {error}"
        ))
    })?;
    Ok(Json(
        json!({ "claimant": "processor", "event": event, "evidence": evidence }),
    ))
}

pub async fn submit_recovery_handler(
    State(state): State<AppState>,
    Extension(clock): Extension<SettlementClock>,
    Path(quote_id): Path<String>,
    Authenticated(buyer): Authenticated,
    Json(body): Json<SubmitRecoveryRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let stored = authorized_quote(&state, buyer, &quote_id).await?;
    let now = clock.now_secs()?;
    state
        .storage
        .process_settlement_deadlines(&quote_id, now)
        .await?;
    let status = state
        .storage
        .settlement_status(&quote_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("quote not found".into()))?;
    let failure_at = status
        .failure_at
        .ok_or_else(|| ApiError::Conflict("quote has not failed".into()))?;
    if now <= failure_at.saturating_add(adp_core::RECOVERY_CLAIM_GRACE_SECS) {
        return Err(ApiError::Conflict(
            "processor recovery grace period is active".into(),
        ));
    }
    let submitted = adp_core::verify_recovery_event(&body.event, &body.plaintext)
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    let decrypted = nostr_sdk::nips::nip44::decrypt(
        state.operator_keys.secret_key(),
        &body.event.pubkey,
        &body.event.content,
    )
    .map_err(|_| ApiError::BadRequest("invalid recovery ciphertext".into()))?;
    if decrypted != body.plaintext {
        return Err(ApiError::BadRequest(
            "recovery ciphertext does not match submitted evidence".into(),
        ));
    }
    let quote = decrypt_quote(&state, &stored)?;
    let paid_leg_ids = state
        .storage
        .settlement_attempts(&quote_id)
        .await?
        .into_iter()
        .filter(|attempt| attempt.settled_at.is_some())
        .map(|attempt| attempt.leg_id)
        .collect::<Vec<_>>();
    let expected = recovery_evidence(
        &quote,
        &stored.quote_event.id.to_hex(),
        failure_at,
        paid_leg_ids,
    )?;
    if submitted != expected || body.event.pubkey.to_hex() != quote.buyer_pubkey {
        return Err(ApiError::BadRequest(
            "recovery evidence does not match quote".into(),
        ));
    }
    let event = state
        .storage
        .save_recovery_claim(
            &quote_id,
            &quote.buyer_pubkey,
            "buyer",
            &body.event,
            &expected.evidence_commitment,
        )
        .await?;
    state.relay.publish(&event).await.map_err(|error| {
        ApiError::ServiceUnavailable(format!(
            "fallback claim persisted but publication failed: {error}"
        ))
    })?;
    Ok(Json(json!({ "event": event })))
}

pub async fn prepare_refund_handler(
    State(state): State<AppState>,
    Extension(clock): Extension<SettlementClock>,
    Path((quote_id, leg_id)): Path<(String, String)>,
    Authenticated(recipient): Authenticated,
) -> ApiResult<Json<serde_json::Value>> {
    let now = clock.now_secs()?;
    state
        .storage
        .process_settlement_deadlines(&quote_id, now)
        .await?;
    let context = state
        .storage
        .refund_context(&quote_id, &leg_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("refund obligation not found".into()))?;
    if context.recipient_pubkey != recipient.to_hex() {
        return Err(ApiError::Forbidden(
            "refund is owed by another recipient".into(),
        ));
    }
    if context.state == "verified" {
        return Err(ApiError::Conflict("refund is already verified".into()));
    }
    if let Some(attempt) = state
        .storage
        .refund_attempt_for_leg(&quote_id, &leg_id)
        .await?
    {
        return refund_invoice_response(&state, &attempt);
    }
    let destination: RefundDestination =
        serde_json::from_slice(&decrypt_private(&state, &context.encrypted_destination)?)
            .map_err(|_| ApiError::Internal("corrupt refund destination".into()))?;
    let buyer = PublicKey::from_hex(&context.buyer_pubkey)
        .map_err(|_| ApiError::Internal("corrupt refund buyer".into()))?;
    let exact = request_exact_invoice(
        &destination.endpoint,
        context.principal_msat,
        recipient,
        buyer,
        None,
        None,
        now.saturating_add(86_400),
        state.config.allow_insecure_lnurl,
    )
    .await
    .map_err(|error| ApiError::PaymentRequired(error.to_string()))?;
    let attempt = NewSettlementAttempt {
        attempt_id: Uuid::new_v4().to_string(),
        leg_id: context.leg_id.clone(),
        encrypted_bolt11: encrypt_private(&state, exact.bolt11.as_bytes())?,
        lnurl_origin: exact.provenance.endpoint,
        callback: exact.provenance.callback,
        lsp_pubkey: exact.provenance.lsp_pubkey,
        payment_hash: exact.payment_hash,
        amount_msat: context.principal_msat,
        expires_at: exact.expires_at,
    };
    let persisted = state
        .storage
        .save_refund_attempt(&quote_id, &context, &attempt, now)
        .await?;
    refund_invoice_response(&state, &persisted)
}

/// Operational account-scoped reads. They cannot prepare invoices or pay.
pub async fn recipient_refunds_handler(
    State(state): State<AppState>,
    Extension(clock): Extension<SettlementClock>,
    Authenticated(recipient): Authenticated,
) -> ApiResult<Json<serde_json::Value>> {
    let now = clock.now_secs()?;
    let ids:Vec<String>=sqlx::query_scalar("SELECT DISTINCT q.quote_id FROM purchase_quotes q JOIN payment_legs l ON l.quote_id=q.quote_id WHERE l.recipient_pubkey=? AND (q.state='failed' OR (q.state='active' AND q.payment_expires_at<=?)) LIMIT 500")
        .bind(recipient.to_hex()).bind(i64::try_from(now).map_err(|_|ApiError::Internal("invalid clock".into()))?).fetch_all(&state.storage.pool()).await.map_err(crate::storage::PersistenceError::from)?;
    for id in ids {
        state.storage.process_settlement_deadlines(&id, now).await?;
    }
    Ok(Json(
        json!({"obligations":state.storage.refund_views(Some(&recipient.to_hex()),None).await?}),
    ))
}

pub async fn buyer_refunds_handler(
    State(state): State<AppState>,
    Extension(clock): Extension<SettlementClock>,
    Path(quote_id): Path<String>,
    Authenticated(buyer): Authenticated,
) -> ApiResult<Json<serde_json::Value>> {
    authorized_quote(&state, buyer, &quote_id).await?;
    state
        .storage
        .process_settlement_deadlines(&quote_id, clock.now_secs()?)
        .await?;
    Ok(Json(
        json!({"obligations":state.storage.refund_views(None,Some(&quote_id)).await?}),
    ))
}

fn refund_invoice_response(
    state: &AppState,
    attempt: &crate::storage::StoredRefundAttempt,
) -> ApiResult<Json<serde_json::Value>> {
    let bolt11 = String::from_utf8(decrypt_private(state, &attempt.encrypted_bolt11)?)
        .map_err(|_| ApiError::Internal("corrupt refund invoice".into()))?;
    Ok(Json(json!({
        "attempt_id": attempt.attempt_id, "leg_id": attempt.leg_id,
        "amount_msat": attempt.amount_msat, "bolt11": bolt11,
        "expires_at": attempt.expires_at
    })))
}

pub async fn confirm_refund_handler(
    State(state): State<AppState>,
    Extension(clock): Extension<SettlementClock>,
    Path((quote_id, leg_id)): Path<(String, String)>,
    Authenticated(recipient): Authenticated,
    Json(body): Json<ConfirmRefundRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let context = state
        .storage
        .refund_context(&quote_id, &leg_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("refund obligation not found".into()))?;
    if context.recipient_pubkey != recipient.to_hex() {
        return Err(ApiError::Forbidden(
            "refund is owed by another recipient".into(),
        ));
    }
    let attempt = state
        .storage
        .refund_attempt(&quote_id, &body.attempt_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("refund attempt not found".into()))?;
    if attempt.leg_id != leg_id {
        return Err(ApiError::Conflict(
            "refund attempt is not confirmable".into(),
        ));
    }
    let bolt11 = String::from_utf8(decrypt_private(&state, &attempt.encrypted_bolt11)?)
        .map_err(|_| ApiError::Internal("corrupt refund invoice".into()))?;
    let binding = verify_bolt11_preimage(&bolt11, &body.preimage, Some(attempt.amount_msat))
        .map_err(|error| ApiError::PaymentRequired(error.to_string()))?;
    if binding.payment_hash != attempt.payment_hash {
        return Err(ApiError::PaymentRequired(
            "refund payment hash mismatch".into(),
        ));
    }
    if let Some(verified_at) = attempt.verified_at {
        return Ok(Json(json!({"state":"verified","verified_at":verified_at})));
    }
    if context.state == "verified" {
        return Err(ApiError::Conflict(
            "another refund attempt already discharged this obligation".into(),
        ));
    }
    let now = clock.now_secs()?;
    if !state
        .storage
        .verify_refund_attempt(
            &quote_id,
            &attempt.attempt_id,
            &attempt.payment_hash,
            &encrypt_private(&state, body.preimage.as_bytes())?,
            now,
        )
        .await?
    {
        return Err(ApiError::Conflict(
            "refund proof was already consumed".into(),
        ));
    }
    Ok(Json(json!({ "state": "verified", "verified_at": now })))
}
