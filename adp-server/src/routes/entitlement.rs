use adp_core::campaign::{parse_campaign_event, resolve_campaign};
use adp_core::{
    build_entitlement_event_builder, parse_authorization_event, resolve_authorization_at,
    AdpListing, BuildEntitlementParams, FulfillmentAuthorization, GrantStatus,
};
use axum::extract::State;
use axum::http::HeaderMap;
use axum::Json;
use nostr_sdk::{Event, EventId, Keys, PublicKey, Timestamp};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{ApiError, ApiResult};
use crate::relay::{
    complete_authorization_successors, complete_predecessor_chain, RelayCompletionError,
    RelayLookup,
};
use crate::state::AppState;
use crate::storage::{ClaimState, MarkClaimPublishedResult};

#[derive(Debug, Deserialize)]
pub struct ClaimRequest {
    game_coordinate: String,
    campaign_event_id: String,
}

#[derive(Debug, Serialize)]
pub struct ClaimResponse {
    grant: Event,
    download_token: String,
    token_expires_at: i64,
    already_claimed: bool,
}

pub async fn claim_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ClaimRequest>,
) -> ApiResult<Json<ClaimResponse>> {
    let buyer = verify_buyer(&state, &headers)?;
    let (developer, d_tag) = parse_coordinate(&body.game_coordinate)?;
    let campaign_root_id = EventId::from_hex(&body.campaign_event_id)
        .map_err(|_| ApiError::BadRequest("invalid campaign_event_id".into()))?;
    let campaign_root = lookup_event(&state, campaign_root_id, "campaign root").await?;
    campaign_root
        .verify()
        .map_err(|_| ApiError::BadRequest("invalid campaign root signature".into()))?;
    let parsed_root = parse_campaign_event(&campaign_root)?;
    if campaign_root.kind.as_u16() != adp_core::EXPERIMENTAL_ADP_CAMPAIGN_KIND
        || campaign_root.pubkey != developer
        || parsed_root.coordinate != body.game_coordinate
        || parsed_root.predecessor.is_some()
    {
        return Err(ApiError::BadRequest(
            "campaign_event_id does not identify the requested campaign root".into(),
        ));
    }
    if let Some(existing) = state
        .storage
        .find_entitlement_claim(
            &buyer.to_hex(),
            &body.game_coordinate,
            &parsed_root.campaign_id,
        )
        .await?
    {
        if existing.campaign_root_event_id != campaign_root_id.to_hex() {
            return Err(ApiError::Conflict(
                "stored claim belongs to a different campaign root".into(),
            ));
        }
        return finish_claim(&state, existing, true).await;
    }

    let listing = current_listing(&state, developer, &d_tag, &body.game_coordinate).await?;
    if !listing.distributes_via(state.relay.server_url()) {
        return Err(ApiError::DistributionUnauthorized(
            "this server is not named in the current listing".into(),
        ));
    }
    let file_hash = listing
        .file_hash
        .as_deref()
        .ok_or_else(|| ApiError::NotFound("listing has no file_hash".into()))?;
    if state.storage.file_path_for_hash(file_hash).await?.is_none() {
        return Err(ApiError::NotFound("game is not hosted locally".into()));
    }

    let campaign_events = match state
        .relay
        .lookup_campaign_chain(developer, &parsed_root.campaign_id)
        .await
    {
        RelayLookup::Found(mut events) => {
            if !events.iter().any(|event| event.id == campaign_root.id) {
                events.push(campaign_root.clone());
            }
            events
        }
        RelayLookup::ConfirmedAbsent { .. } | RelayLookup::Unavailable { .. } => {
            return Err(ApiError::ServiceUnavailable(
                "campaign chain lookup incomplete".into(),
            ))
        }
    };
    let campaign_events = complete_predecessor_chain(&*state.relay, campaign_events, |event| {
        Ok(parse_campaign_event(event)
            .ok()
            .and_then(|parsed| parsed.predecessor))
    })
    .await
    .map_err(completion_error)?;
    let parsed_campaigns = campaign_events
        .iter()
        .filter_map(|event| parse_campaign_event(event).ok())
        .collect::<Vec<_>>();
    let campaign = resolve_campaign(&parsed_campaigns, developer, &body.game_coordinate)
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    if campaign.root_event_id != campaign_root_id {
        return Err(ApiError::BadRequest(
            "campaign_event_id does not identify the campaign root".into(),
        ));
    }
    let now = unix_now();
    if !campaign.is_claimable_at(now) {
        return Err(ApiError::Forbidden("campaign is not claimable".into()));
    }

    let (signing_keys, authorization) = select_signing_authority(
        &state,
        &listing,
        developer,
        &body.game_coordinate,
        adp_core::CAPABILITY_ISSUE_GRANT,
        now,
    )
    .await?;

    let params = BuildEntitlementParams {
        grant_id: Uuid::new_v4().to_string(),
        recipient: buyer,
        coordinate: body.game_coordinate.clone(),
        source_event: Some(campaign_root_id),
        authorization,
        reason: Some("promotional-claim".into()),
        status: GrantStatus::Granted,
        predecessor: None,
    };
    let grant = build_entitlement_event_builder(&params)?
        .custom_created_at(Timestamp::from_secs(now))
        .sign_with_keys(&signing_keys)
        .map_err(|error| ApiError::Internal(format!("failed to sign grant: {error}")))?;
    let (stored, inserted) = state
        .storage
        .insert_pending_entitlement_claim(
            &buyer.to_hex(),
            &body.game_coordinate,
            &campaign.campaign_id,
            &campaign_root_id.to_hex(),
            &grant,
        )
        .await?;
    finish_claim(&state, stored, !inserted).await
}

fn verify_buyer(state: &AppState, headers: &HeaderMap) -> ApiResult<PublicKey> {
    let header = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| ApiError::Unauthorized("missing Authorization header".into()))?;
    let event = adp_core::decode_authorization_header(header)?;
    let url = format!("{}/entitlement/claim", state.config.server_url);
    Ok(adp_core::verify_nip98(&event, "POST", &url)?.pubkey)
}

async fn current_listing(
    state: &AppState,
    developer: PublicKey,
    d_tag: &str,
    coordinate: &str,
) -> ApiResult<AdpListing> {
    let event = match state.relay.lookup_listing(developer, d_tag).await {
        RelayLookup::Found(event) => event,
        RelayLookup::ConfirmedAbsent { .. } => {
            return Err(ApiError::NotFound("listing not found".into()))
        }
        RelayLookup::Unavailable { .. } => {
            return Err(ApiError::ServiceUnavailable(
                "listing lookup unavailable".into(),
            ))
        }
    };
    event
        .verify()
        .map_err(|_| ApiError::BadRequest("invalid listing signature".into()))?;
    let listing = AdpListing::from_event(event)?;
    if listing.developer_pubkey != developer || listing.coordinate() != coordinate {
        return Err(ApiError::BadRequest(
            "listing does not match requested coordinate".into(),
        ));
    }
    Ok(listing)
}

async fn lookup_event(state: &AppState, id: EventId, name: &str) -> ApiResult<Event> {
    match state.relay.lookup_event(id).await {
        RelayLookup::Found(event) if event.id == id => Ok(event),
        RelayLookup::Found(_) => Err(ApiError::BadRequest(format!(
            "relay returned wrong {name} event"
        ))),
        RelayLookup::ConfirmedAbsent { .. } => Err(ApiError::NotFound(format!("{name} not found"))),
        RelayLookup::Unavailable { .. } => Err(ApiError::ServiceUnavailable(format!(
            "{name} lookup unavailable"
        ))),
    }
}

pub(super) async fn select_signing_authority(
    state: &AppState,
    listing: &AdpListing,
    developer: PublicKey,
    coordinate: &str,
    capability: &str,
    now: u64,
) -> ApiResult<(Keys, Option<EventId>)> {
    if state.operator_keys.public_key() == developer {
        return Ok((state.operator_keys.clone(), None));
    }
    let candidates = ordered_authorizations(listing);
    let mut unavailable = false;
    for candidate in candidates {
        let Some(provisioned) = state
            .keystore
            .attestation_for_fulfillment_pubkey(
                &developer.to_hex(),
                &candidate.fulfillment_pubkey.to_hex(),
            )
            .await
            .map_err(|error| ApiError::Internal(error.to_string()))?
        else {
            continue;
        };
        match resolve_listing_authorization(
            state, developer, coordinate, &candidate, capability, now,
        )
        .await
        {
            Ok(true) => return Ok((provisioned.keys, Some(candidate.root_event_id))),
            Ok(false) => {}
            Err(ApiError::ServiceUnavailable(_)) => unavailable = true,
            Err(_) => {}
        }
    }
    if unavailable {
        return Err(ApiError::ServiceUnavailable(
            "authorization evidence unavailable".into(),
        ));
    }
    Err(ApiError::Internal(
        "no held key has current listing and authorization authority".into(),
    ))
}

fn ordered_authorizations(listing: &AdpListing) -> Vec<FulfillmentAuthorization> {
    let mut candidates = listing.fulfillment_authorizations.clone();
    candidates.sort_by_key(|candidate| candidate.root_event_id);
    candidates
}

pub(super) async fn resolve_listing_authorization(
    state: &AppState,
    developer: PublicKey,
    coordinate: &str,
    reference: &FulfillmentAuthorization,
    capability: &str,
    at: u64,
) -> ApiResult<bool> {
    let root = match state.relay.lookup_event(reference.root_event_id).await {
        RelayLookup::Found(event) if event.id == reference.root_event_id => event,
        RelayLookup::Found(_) => return Ok(false),
        RelayLookup::ConfirmedAbsent { .. } => return Ok(false),
        RelayLookup::Unavailable { .. } => {
            return Err(ApiError::ServiceUnavailable(
                "authorization root lookup unavailable".into(),
            ))
        }
    };
    let parsed_root = match parse_authorization_event(&root) {
        Ok(parsed) => parsed,
        Err(_) => return Ok(false),
    };
    if parsed_root.predecessor.is_some()
        || parsed_root.terms.coordinate != coordinate
        || parsed_root.terms.fulfillment_pubkey != reference.fulfillment_pubkey
        || parsed_root.terms.operator_pubkey != state.operator_keys.public_key()
        || root.pubkey != developer
    {
        return Ok(false);
    }
    let events = complete_authorization_successors(&*state.relay, developer, root)
        .await
        .map_err(completion_error)?;
    let authorization = resolve_authorization_at(reference.root_event_id, &events, at)
        .map_err(|_| ApiError::BadRequest("invalid authorization chain".into()))?;
    Ok(authorization.authorizes(&reference.fulfillment_pubkey, coordinate, capability, at))
}

async fn finish_claim(
    state: &AppState,
    claim: crate::storage::StoredEntitlementClaim,
    already_claimed: bool,
) -> ApiResult<Json<ClaimResponse>> {
    if claim.state == ClaimState::Pending {
        state
            .storage
            .record_claim_publish_attempt(
                &claim.buyer_pubkey,
                &claim.game_coordinate,
                &claim.campaign_id,
            )
            .await?;
        state
            .relay
            .publish(&claim.grant)
            .await
            .map_err(|error| ApiError::ServiceUnavailable(error.to_string()))?;
        let mark_result = state
            .storage
            .mark_claim_published_if_identical(
                &claim.buyer_pubkey,
                &claim.game_coordinate,
                &claim.campaign_id,
                &claim.grant,
            )
            .await?;
        if !matches!(
            mark_result,
            MarkClaimPublishedResult::Updated | MarkClaimPublishedResult::AlreadyPublishedIdentical
        ) {
            return Err(ApiError::Internal(format!(
                "claim publication state conflicts with persisted grant: {mark_result:?}"
            )));
        }
    }
    let token = Uuid::new_v4().to_string();
    let expires_at = unix_now() as i64 + state.config.download_token_ttl_secs;
    state
        .storage
        .issue_attributed_download_token(
            &token,
            &claim.game_coordinate,
            &claim.buyer_pubkey,
            expires_at,
            "entitlement",
            &claim.grant_event_id,
        )
        .await?;
    Ok(Json(ClaimResponse {
        grant: claim.grant,
        download_token: token,
        token_expires_at: expires_at,
        already_claimed,
    }))
}

fn completion_error(error: RelayCompletionError) -> ApiError {
    match error {
        RelayCompletionError::Unavailable { .. } => ApiError::ServiceUnavailable(error.to_string()),
        RelayCompletionError::Malformed(_) => ApiError::BadRequest(error.to_string()),
    }
}

fn parse_coordinate(coordinate: &str) -> ApiResult<(PublicKey, String)> {
    let mut parts = coordinate.splitn(3, ':');
    if parts.next() != Some("30402") {
        return Err(ApiError::BadRequest(
            "coordinate must use kind 30402".into(),
        ));
    }
    let developer = parts
        .next()
        .and_then(|value| PublicKey::from_hex(value).ok())
        .ok_or_else(|| ApiError::BadRequest("invalid coordinate developer".into()))?;
    let d_tag = parts
        .next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::BadRequest("invalid coordinate identifier".into()))?;
    Ok((developer, d_tag.into()))
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr_sdk::{EventBuilder, Keys, Kind, Tag};

    #[test]
    fn signer_candidates_are_ordered_by_root_id() {
        let developer = Keys::generate();
        let fulfillment = Keys::generate();
        let high = EventId::from_hex(&"ff".repeat(32)).unwrap();
        let low = EventId::from_hex(&"01".repeat(32)).unwrap();
        let event = EventBuilder::new(Kind::Custom(30402), "")
            .tags(vec![
                Tag::parse(["d", "game"]).unwrap(),
                Tag::parse([
                    "fulfillment_authorization",
                    &high.to_hex(),
                    &fulfillment.public_key().to_hex(),
                ])
                .unwrap(),
                Tag::parse([
                    "fulfillment_authorization",
                    &low.to_hex(),
                    &fulfillment.public_key().to_hex(),
                ])
                .unwrap(),
            ])
            .sign_with_keys(&developer)
            .unwrap();
        let listing = AdpListing::from_event(event).unwrap();
        let ordered = ordered_authorizations(&listing);
        assert_eq!(ordered[0].root_event_id, low);
        assert_eq!(ordered[1].root_event_id, high);
    }
}
