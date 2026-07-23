use adp_core::{
    build_attestation_event_builder, decode_authorization_header, parse_attestation_event,
};
use axum::{extract::State, http::HeaderMap, Json};
use nostr_sdk::{Keys, PublicKey, Timestamp};
use serde::{Deserialize, Serialize};

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct ProvisionRequest {
    pub scope: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ProvisionResponse {
    pub fulfillment_pubkey: String,
    pub attestation_event_id: String,
    pub scope: String,
}

#[derive(Debug, Deserialize)]
pub struct RevokeRequest {
    pub fulfillment_pubkey: String,
}

#[derive(Debug, Serialize)]
pub struct RevokeResponse {
    pub fulfillment_pubkey: String,
    pub attestation_event_id: String,
    pub revoked_at: u64,
}

pub async fn provision_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ProvisionRequest>,
) -> ApiResult<Json<ProvisionResponse>> {
    let developer_pubkey = verify_developer(&state, &headers, "POST", "/provision")?;
    let developer_pubkey_hex = developer_pubkey.to_hex();
    let scope = body.scope.unwrap_or_default();

    if let Some(existing) = state
        .keystore
        .active_key_for_scope(&developer_pubkey_hex, &scope)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
    {
        return Ok(Json(ProvisionResponse {
            fulfillment_pubkey: existing.fulfillment_pubkey,
            attestation_event_id: existing.attestation_event_id,
            scope: existing.scope,
        }));
    }

    let keys = Keys::generate();
    let valid_from = Timestamp::now().as_secs();
    let builder = build_attestation_event_builder(
        &developer_pubkey,
        &keys.public_key(),
        valid_from,
        None,
        Some(scope.as_str()),
    )?;
    let attestation = builder
        .sign_with_keys(&state.operator_keys)
        .map_err(|e| ApiError::Internal(format!("failed to sign attestation: {e}")))?;
    state
        .relay
        .publish(&attestation)
        .await
        .map_err(|e| ApiError::Internal(format!("failed to publish attestation: {e}")))?;

    state
        .keystore
        .insert_provisioned_key(
            &developer_pubkey_hex,
            &scope,
            &keys,
            &attestation.id.to_hex(),
            valid_from,
        )
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(ProvisionResponse {
        fulfillment_pubkey: keys.public_key().to_hex(),
        attestation_event_id: attestation.id.to_hex(),
        scope,
    }))
}

pub async fn revoke_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<RevokeRequest>,
) -> ApiResult<Json<RevokeResponse>> {
    let developer_pubkey = verify_developer(&state, &headers, "POST", "/provision/revoke")?;
    let developer_pubkey_hex = developer_pubkey.to_hex();
    let fulfillment_pubkey = PublicKey::from_hex(&body.fulfillment_pubkey)
        .map_err(|_| ApiError::BadRequest("malformed fulfillment_pubkey".into()))?;
    let existing = state
        .keystore
        .attestation_for_fulfillment_pubkey(&developer_pubkey_hex, &body.fulfillment_pubkey)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or_else(|| {
            ApiError::Forbidden("fulfillment key is not owned by this developer".into())
        })?;

    let revoked_at = Timestamp::now().as_secs();
    let builder = build_attestation_event_builder(
        &developer_pubkey,
        &fulfillment_pubkey,
        existing.valid_from,
        Some(revoked_at),
        Some(existing.scope.as_str()),
    )?;
    let attestation = builder
        .sign_with_keys(&state.operator_keys)
        .map_err(|e| ApiError::Internal(format!("failed to sign revocation attestation: {e}")))?;
    let parsed = parse_attestation_event(&attestation)?;
    if parsed.revoked_at != Some(revoked_at) {
        return Err(ApiError::Internal(
            "revocation attestation malformed".into(),
        ));
    }

    state.relay.publish(&attestation).await.map_err(|e| {
        ApiError::Internal(format!("failed to publish revocation attestation: {e}"))
    })?;

    // Operator-side revocation only stops this server signing with the key. Existing
    // credentials remain governed by their developer-signed authorization lifecycle.
    state
        .keystore
        .revoke(&developer_pubkey_hex, &body.fulfillment_pubkey, revoked_at)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(RevokeResponse {
        fulfillment_pubkey: body.fulfillment_pubkey,
        attestation_event_id: attestation.id.to_hex(),
        revoked_at,
    }))
}

fn verify_developer(
    state: &AppState,
    headers: &HeaderMap,
    method: &str,
    path: &str,
) -> ApiResult<PublicKey> {
    let auth_header = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| ApiError::Unauthorized("missing Authorization header".into()))?;
    let token_event = decode_authorization_header(auth_header)?;
    let expected_url = format!("{}{}", state.config.server_url, path);
    Ok(adp_core::verify_nip98(&token_event, method, &expected_url)?.pubkey)
}
