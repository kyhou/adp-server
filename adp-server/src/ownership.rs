#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessBasis {
    DownloadToken,
    EntitlementGrant,
    PurchaseReceipt,
    PublicPolicy,
    TimedAccessPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialSource {
    Entitlements,
    Receipts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialCheck {
    Valid(AccessBasis),
    Absent,
    Invalid { candidate_count: usize },
    Unavailable { source: CredentialSource },
}

enum CandidateFailure {
    Invalid,
    Unavailable,
}

pub async fn verify_ownership_via_entitlements(
    state: &AppState,
    buyer: PublicKey,
    coordinate: &str,
    developer: PublicKey,
) -> CredentialCheck {
    let events = match state.relay.lookup_entitlements(buyer, coordinate).await {
        RelayLookup::Found(events) => events,
        RelayLookup::ConfirmedAbsent { .. } => return CredentialCheck::Absent,
        RelayLookup::Unavailable { .. } => {
            return CredentialCheck::Unavailable {
                source: CredentialSource::Entitlements,
            }
        }
    };
    let mut invalid_count = 0usize;
    let parsed = events
        .iter()
        .filter_map(|event| match parse_entitlement_event(event) {
            Ok(parsed) if parsed.recipient == buyer && parsed.coordinate == coordinate => {
                Some(parsed)
            }
            _ => {
                invalid_count += 1;
                None
            }
        })
        .collect::<Vec<_>>();
    let mut unavailable = false;
    for component in group_parsed_entitlements(&parsed) {
        match verify_entitlement_component(state, component, developer, coordinate).await {
            Ok(()) => return CredentialCheck::Valid(AccessBasis::EntitlementGrant),
            Err(CandidateFailure::Invalid) => invalid_count += 1,
            Err(CandidateFailure::Unavailable) => unavailable = true,
        }
    }
    if unavailable {
        CredentialCheck::Unavailable {
            source: CredentialSource::Entitlements,
        }
    } else if invalid_count > 0 {
        CredentialCheck::Invalid {
            candidate_count: invalid_count,
        }
    } else {
        CredentialCheck::Absent
    }
}

async fn verify_entitlement_component(
    state: &AppState,
    component: Vec<adp_core::ParsedEntitlementGrant>,
    developer: PublicKey,
    coordinate: &str,
) -> Result<(), CandidateFailure> {
    let events = complete_predecessor_chain(
        &*state.relay,
        component.iter().map(|node| node.event.clone()).collect(),
        |event| {
            parse_entitlement_event(event)
                .map(|parsed| parsed.predecessor)
                .map_err(|error| ChainResolutionError::InvalidTransition(error.to_string()))
        },
    )
    .await
    .map_err(completion_failure)?;
    let parsed = events
        .iter()
        .map(parse_entitlement_event)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| CandidateFailure::Invalid)?;
    let grant = resolve_entitlement_grant(&parsed).map_err(|_| CandidateFailure::Invalid)?;
    let root = grant.events.first().ok_or(CandidateFailure::Invalid)?;
    if grant.events.last().map(|event| event.status) != Some(GrantStatus::Granted)
        || root.recipient.to_hex().is_empty()
        || root.coordinate != coordinate
    {
        return Err(CandidateFailure::Invalid);
    }
    let campaign_root_id = root.source_event.ok_or(CandidateFailure::Invalid)?;
    let campaign_root = candidate_event(state, campaign_root_id).await?;
    let parsed_campaign_root =
        parse_campaign_event(&campaign_root).map_err(|_| CandidateFailure::Invalid)?;
    let campaign_events = match state
        .relay
        .lookup_campaign_chain(developer, &parsed_campaign_root.campaign_id)
        .await
    {
        RelayLookup::Found(mut events) => {
            if !events.iter().any(|event| event.id == campaign_root_id) {
                events.push(campaign_root);
            }
            events
        }
        RelayLookup::ConfirmedAbsent { .. } | RelayLookup::Unavailable { .. } => {
            return Err(CandidateFailure::Unavailable)
        }
    };
    let campaign_events = complete_predecessor_chain(&*state.relay, campaign_events, |event| {
        parse_campaign_event(event)
            .map(|parsed| parsed.predecessor)
            .map_err(|error| ChainResolutionError::InvalidTransition(error.to_string()))
    })
    .await
    .map_err(completion_failure)?;
    let campaign_nodes = campaign_events
        .iter()
        .map(parse_campaign_event)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| CandidateFailure::Invalid)?;
    let campaign = resolve_campaign(&campaign_nodes, developer, coordinate)
        .map_err(|_| CandidateFailure::Invalid)?;
    if campaign.root_event_id != campaign_root_id {
        return Err(CandidateFailure::Invalid);
    }
    let authorization = match root.authorization_event {
        Some(root_id) => {
            Some(candidate_authorization(state, developer, coordinate, root_id).await?)
        }
        None => None,
    };
    verify_adp_entitlement(&grant, &campaign, authorization.as_ref())
        .map_err(|_| CandidateFailure::Invalid)
}

pub async fn verify_ownership_via_receipts(
    state: &AppState,
    buyer: PublicKey,
    coordinate: &str,
    developer: PublicKey,
) -> CredentialCheck {
    let events = match state.relay.lookup_receipts(buyer, coordinate).await {
        RelayLookup::Found(events) => events,
        RelayLookup::ConfirmedAbsent { .. } => return CredentialCheck::Absent,
        RelayLookup::Unavailable { .. } => {
            return CredentialCheck::Unavailable {
                source: CredentialSource::Receipts,
            }
        }
    };
    let mut invalid_count = 0usize;
    let mut by_order: HashMap<String, Vec<Event>> = HashMap::new();
    for event in events {
        match parse_receipt_event(&event) {
            Ok(parsed) if parsed.buyer_pubkey == buyer && parsed.coordinate == coordinate => {
                by_order.entry(parsed.order_id).or_default().push(event);
            }
            _ => invalid_count += 1,
        }
    }
    let mut unavailable = false;
    for events in by_order.into_values() {
        match verify_receipt_component(state, events, developer, coordinate).await {
            Ok(true) => return CredentialCheck::Valid(AccessBasis::PurchaseReceipt),
            Ok(false) | Err(CandidateFailure::Invalid) => invalid_count += 1,
            Err(CandidateFailure::Unavailable) => unavailable = true,
        }
    }
    if unavailable {
        CredentialCheck::Unavailable {
            source: CredentialSource::Receipts,
        }
    } else if invalid_count > 0 {
        CredentialCheck::Invalid {
            candidate_count: invalid_count,
        }
    } else {
        CredentialCheck::Absent
    }
}

async fn verify_receipt_component(
    state: &AppState,
    events: Vec<Event>,
    developer: PublicKey,
    coordinate: &str,
) -> Result<bool, CandidateFailure> {
    let events = complete_predecessor_chain(&*state.relay, events, |event| {
        parse_receipt_event(event)
            .map(|parsed| {
                parsed
                    .e_tag
                    .as_deref()
                    .and_then(|id| EventId::from_hex(id).ok())
            })
            .map_err(|error| ChainResolutionError::InvalidTransition(error.to_string()))
    })
    .await
    .map_err(completion_failure)?;
    let root = events
        .iter()
        .filter_map(|event| {
            parse_receipt_event(event)
                .ok()
                .map(|parsed| (event, parsed))
        })
        .find(|(_, parsed)| parsed.e_tag.is_none())
        .ok_or(CandidateFailure::Invalid)?;
    let authorization = match root.1.authorization_event_id {
        Some(root_id) => {
            Some(candidate_authorization(state, developer, coordinate, root_id).await?)
        }
        None => None,
    };
    let tip = resolve_receipt_chain(&events, developer, authorization.as_ref())
        .map_err(|_| CandidateFailure::Invalid)?;
    let parsed_tip = parse_receipt_event(tip).map_err(|_| CandidateFailure::Invalid)?;
    Ok(parsed_tip.status == "paid" || parsed_tip.status == "fulfilled")
}

async fn candidate_authorization(
    state: &AppState,
    developer: PublicKey,
    coordinate: &str,
    root_id: EventId,
) -> Result<ResolvedAuthorization, CandidateFailure> {
    let root = candidate_event(state, root_id).await?;
    let events = complete_authorization_successors(&*state.relay, developer, root)
        .await
        .map_err(completion_failure)?;
    let nodes = events
        .iter()
        .map(parse_authorization_event)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| CandidateFailure::Invalid)?;
    let authorization =
        resolve_authorization(root_id, &nodes).map_err(|_| CandidateFailure::Invalid)?;
    if authorization.developer_pubkey != developer || authorization.terms.coordinate != coordinate {
        return Err(CandidateFailure::Invalid);
    }
    Ok(authorization)
}

async fn candidate_event(state: &AppState, id: EventId) -> Result<Event, CandidateFailure> {
    match state.relay.lookup_event(id).await {
        RelayLookup::Found(event) if event.id == id => Ok(event),
        RelayLookup::Found(_) | RelayLookup::ConfirmedAbsent { .. } => {
            Err(CandidateFailure::Invalid)
        }
        RelayLookup::Unavailable { .. } => Err(CandidateFailure::Unavailable),
    }
}

fn completion_failure(error: RelayCompletionError) -> CandidateFailure {
    match error {
        RelayCompletionError::Unavailable { .. } => CandidateFailure::Unavailable,
        RelayCompletionError::Malformed(_) => CandidateFailure::Invalid,
    }
}
use std::collections::HashMap;

use adp_core::campaign::{parse_campaign_event, resolve_campaign};
use adp_core::{
    group_parsed_entitlements, parse_authorization_event, parse_entitlement_event,
    parse_receipt_event, resolve_authorization, resolve_entitlement_grant, resolve_receipt_chain,
    verify_adp_entitlement, ChainResolutionError, GrantStatus, ResolvedAuthorization,
};
use nostr_sdk::{Event, EventId, PublicKey};

use crate::relay::{
    complete_authorization_successors, complete_predecessor_chain, RelayCompletionError,
    RelayLookup,
};
use crate::state::AppState;
