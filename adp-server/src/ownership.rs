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
    let grant = adp_core::resolve_adp_entitlement_grant(&parsed, developer)
        .map_err(|_| CandidateFailure::Invalid)?;
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
        Ok(parse_campaign_event(event)
            .ok()
            .and_then(|parsed| parsed.predecessor))
    })
    .await
    .map_err(completion_failure)?;
    let campaign_nodes = campaign_events
        .iter()
        .filter_map(|event| parse_campaign_event(event).ok())
        .collect::<Vec<_>>();
    let campaign = adp_core::resolve_campaign_at(
        &campaign_nodes,
        developer,
        coordinate,
        root.event.created_at.as_secs(),
    )
    .map_err(|_| CandidateFailure::Invalid)?;
    if campaign.root_event_id != campaign_root_id {
        return Err(CandidateFailure::Invalid);
    }
    let authorization = match root.authorization {
        Some(root_id) => Some(
            candidate_authorization(
                state,
                developer,
                coordinate,
                root_id,
                root.event.created_at.as_secs(),
            )
            .await?,
        ),
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
    listing: &adp_core::AdpListing,
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
        match verify_receipt_component(state, events, developer, coordinate, listing).await {
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
    listing: &adp_core::AdpListing,
) -> Result<bool, CandidateFailure> {
    let events = complete_predecessor_chain(&*state.relay, events, |event| {
        parse_receipt_event(event)
            .map(|parsed| parsed.predecessor)
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
        .find(|(_, parsed)| parsed.predecessor.is_none())
        .ok_or(CandidateFailure::Invalid)?;
    let authorization = match root.1.authorization {
        Some(root_id) => Some(
            candidate_authorization(
                state,
                developer,
                coordinate,
                root_id,
                root.0.created_at.as_secs(),
            )
            .await?,
        ),
        None => None,
    };
    // Validate structure/authority before resolving any public payment destinations.
    let tip = resolve_receipt_chain(&events, developer, authorization.as_ref())
        .map_err(|_| CandidateFailure::Invalid)?;
    if let Some(aggregate) = &root.1.aggregate {
        for leg in aggregate.legs.iter().filter(|leg| leg.proof_type == "zap") {
            let id =
                EventId::from_hex(&leg.proof_reference).map_err(|_| CandidateFailure::Invalid)?;
            let zap = candidate_event(state, id).await?;
            let destination = aggregate_zap_destination(state, &root.1, leg).await?;
            // Read metadata only. Never request an invoice or decrypt private receipt content.
            let provenance = crate::lnurl::inspect_payment_destination(
                &destination,
                state.config.allow_insecure_lnurl,
            )
            .await
            .map_err(|_| CandidateFailure::Unavailable)?;
            let lsp = PublicKey::from_hex(&provenance.lsp_pubkey)
                .map_err(|_| CandidateFailure::Invalid)?;
            validate_aggregate_zap(&root.1, leg, &zap, lsp)?;
        }
    }
    for proof in &root.1.proofs {
        if let adp_core::ReceiptProof::Zap(event_id) = proof {
            let zap = candidate_event(state, *event_id).await?;
            let lud16 = listing.lud16.as_deref().ok_or(CandidateFailure::Invalid)?;
            let lsp =
                crate::lnurl::resolve_lud16_lsp_pubkey(lud16, state.config.allow_insecure_lnurl)
                    .await
                    .map_err(|_| CandidateFailure::Unavailable)?;
            let binding =
                adp_core::verify_zap_receipt(&zap, &root.1.buyer_pubkey.to_hex(), coordinate, &lsp)
                    .map_err(|_| CandidateFailure::Invalid)?;
            if root.1.payment_hash.as_deref() != Some(binding.payment_hash.as_str())
                || root.1.amount_msat != Some(binding.amount_msat)
            {
                return Err(CandidateFailure::Invalid);
            }
        }
    }
    let parsed_tip = parse_receipt_event(tip).map_err(|_| CandidateFailure::Invalid)?;
    Ok(parsed_tip.status == "paid" || parsed_tip.status == "fulfilled")
}

async fn aggregate_zap_destination(
    state: &AppState,
    receipt: &adp_core::ParsedReceipt,
    leg: &adp_core::AggregateProofLeg,
) -> Result<String, CandidateFailure> {
    let aggregate = receipt
        .aggregate
        .as_ref()
        .ok_or(CandidateFailure::Invalid)?;
    let event = candidate_event(state, aggregate.listing_event_id).await?;
    event.verify().map_err(|_| CandidateFailure::Invalid)?;
    let listing =
        adp_core::AdpListing::from_event(event.clone()).map_err(|_| CandidateFailure::Invalid)?;
    if listing.coordinate() != receipt.coordinate {
        return Err(CandidateFailure::Invalid);
    }
    match leg.role {
        adp_core::PaymentRole::Developer => {
            if leg.recipient_pubkey != listing.developer_pubkey.to_hex()
                || leg.accepted_terms_id.is_some()
            {
                return Err(CandidateFailure::Invalid);
            }
            crate::lnurl::unique_tag(&event, "lud16").ok_or(CandidateFailure::Invalid)
        }
        adp_core::PaymentRole::Operator => {
            let terms_id = leg
                .accepted_terms_id
                .as_ref()
                .ok_or(CandidateFailure::Invalid)?;
            let terms = candidate_event(
                state,
                EventId::from_hex(terms_id).map_err(|_| CandidateFailure::Invalid)?,
            )
            .await?;
            let accepted = listing
                .accepted_terms
                .iter()
                .filter(|reference| {
                    reference.operator_pubkey == leg.recipient_pubkey
                        && reference.terms_event_id == *terms_id
                })
                .count();
            let adp_core::TermsPayload::Offer { terms } =
                adp_core::parse_terms_event(&terms).map_err(|_| CandidateFailure::Invalid)?
            else {
                return Err(CandidateFailure::Invalid);
            };
            if accepted != 1 || terms.operator_pubkey != leg.recipient_pubkey {
                return Err(CandidateFailure::Invalid);
            }
            Ok(terms.payment_destination)
        }
    }
}

fn validate_aggregate_zap(
    receipt: &adp_core::ParsedReceipt,
    leg: &adp_core::AggregateProofLeg,
    zap: &Event,
    lsp: PublicKey,
) -> Result<(), CandidateFailure> {
    use crate::lnurl::{unique_tag, valid_zap_request, ZapRequestContext};
    let invalid = || CandidateFailure::Invalid;
    let binding = adp_core::verify_zap_receipt(
        zap,
        &receipt.buyer_pubkey.to_hex(),
        &receipt.coordinate,
        &lsp,
    )
    .map_err(|_| invalid())?;
    if binding.amount_msat != leg.amount_msat
        || binding.payment_hash != leg.payment_hash
        || receipt
            .settled_at
            .is_none_or(|at| zap.created_at.as_secs() > at)
        || unique_tag(zap, "P").as_deref() != Some(receipt.buyer_pubkey.to_hex().as_str())
        || unique_tag(zap, "p").as_deref() != Some(leg.recipient_pubkey.as_str())
    {
        return Err(invalid());
    }
    if zap.tags.iter().any(|tag| {
        tag.as_slice()
            .first()
            .is_some_and(|name| name == "preimage")
    }) {
        let preimage = unique_tag(zap, "preimage").ok_or_else(invalid)?;
        let bytes = hex::decode(preimage).map_err(|_| invalid())?;
        if bytes.len() != 32
            || hex::encode(<sha2::Sha256 as sha2::Digest>::digest(bytes)) != leg.payment_hash
        {
            return Err(invalid());
        }
    }
    let description = unique_tag(zap, "description").ok_or_else(invalid)?;
    let request: Event = serde_json::from_str(&description).map_err(|_| invalid())?;
    let recipient = PublicKey::from_hex(&leg.recipient_pubkey).map_err(|_| invalid())?;
    if request.created_at > zap.created_at
        || !valid_zap_request(
            &request,
            receipt.buyer_pubkey,
            recipient,
            leg.amount_msat,
            ZapRequestContext {
                listing_coordinate: &receipt.coordinate,
                quote_id: &receipt.order_id,
                leg_id: &leg.leg_id,
            },
        )
    {
        return Err(invalid());
    }
    let invoice: lightning_invoice::Bolt11Invoice = unique_tag(zap, "bolt11")
        .ok_or_else(invalid)?
        .parse()
        .map_err(|_| invalid())?;
    let expected = hex::encode(<sha2::Sha256 as sha2::Digest>::digest(
        description.as_bytes(),
    ));
    match invoice.description() {
        lightning_invoice::Bolt11InvoiceDescription::Hash(hash)
            if hash.0.to_string() == expected =>
        {
            Ok(())
        }
        _ => Err(invalid()),
    }
}

async fn candidate_authorization(
    state: &AppState,
    developer: PublicKey,
    coordinate: &str,
    root_id: EventId,
    at: u64,
) -> Result<ResolvedAuthorization, CandidateFailure> {
    let root = candidate_event(state, root_id).await?;
    let events = complete_authorization_successors(&*state.relay, developer, root)
        .await
        .map_err(completion_failure)?;
    let authorization = adp_core::resolve_authorization_at(root_id, &events, at)
        .map_err(|_| CandidateFailure::Invalid)?;
    if authorization.developer_pubkey != developer || authorization.terms.coordinate != coordinate {
        return Err(CandidateFailure::Invalid);
    }
    Ok(authorization)
}

async fn candidate_event(state: &AppState, id: EventId) -> Result<Event, CandidateFailure> {
    match state.relay.lookup_event(id).await {
        RelayLookup::Found(event) if event.id == id => Ok(event),
        RelayLookup::Found(_) => Err(CandidateFailure::Invalid),
        RelayLookup::ConfirmedAbsent { .. } => Err(CandidateFailure::Unavailable),
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

use adp_core::campaign::parse_campaign_event;
use adp_core::{
    group_parsed_entitlements, parse_entitlement_event, parse_receipt_event, resolve_receipt_chain,
    verify_adp_entitlement, ChainResolutionError, GrantStatus, ResolvedAuthorization,
};
use nostr_sdk::{Event, EventId, PublicKey};

use crate::relay::{
    complete_authorization_successors, complete_predecessor_chain, RelayCompletionError,
    RelayLookup,
};
use crate::state::AppState;
