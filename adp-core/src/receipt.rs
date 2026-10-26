use std::collections::{HashMap, HashSet};

use nostr::nips::nip44;
use nostr::{Event, EventBuilder, EventId, Keys, Kind, PublicKey, Tag};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::AdpError;
use crate::listing::AdpListing;
use crate::provisioning::{ResolvedAuthorization, CAPABILITY_ISSUE_RECEIPT};

pub const RECEIPT_KIND: u16 = 1020;

pub fn new_order_id() -> String {
    Uuid::new_v4().to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceiptItem {
    pub product_id: String,
    pub name: String,
    pub quantity: u64,
    pub unit_price: String,
    pub currency: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceiptContent {
    pub order_id: String,
    pub items: Vec<ReceiptItem>,
    pub subtotal: String,
    pub tax: String,
    pub shipping: String,
    pub total: String,
    pub currency: String,
    pub bolt11: Option<String>,
    pub preimage: Option<String>,
    pub notes: Option<String>,
    pub settled_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ReceiptProof {
    Zap(EventId),
    Bolt11Preimage,
}

fn parse_tag(values: Vec<&str>) -> Result<Tag, AdpError> {
    Tag::parse(values).map_err(|e| AdpError::Nostr(e.to_string()))
}

pub fn encrypt_receipt_content(
    signing_keys: &Keys,
    buyer_pubkey: &PublicKey,
    plaintext: &str,
) -> Result<String, AdpError> {
    nip44::encrypt(
        signing_keys.secret_key(),
        buyer_pubkey,
        plaintext,
        nip44::Version::V2,
    )
    .map_err(|e| AdpError::Encryption(e.to_string()))
}

pub struct BuildReceiptParams<'a> {
    pub listing: &'a AdpListing,
    pub buyer_pubkey: PublicKey,
    pub authorization: Option<EventId>,
    pub payment_hash: String,
    pub amount_msat: u64,
    pub settled_at: u64,
    pub proofs: Vec<ReceiptProof>,
    pub amount: String,
    pub currency: String,
}

pub struct BuildAggregateReceiptParams<'a> {
    pub listing: &'a AdpListing,
    pub buyer_pubkey: PublicKey,
    pub authorization: Option<EventId>,
    pub quote_event_id: EventId,
    pub processor_pubkey: PublicKey,
    pub processor_url: Option<&'a str>,
    pub gross_msat: u64,
    pub settled_at: u64,
    pub legs: &'a [crate::settlement::AggregateProofLeg],
}

pub fn build_receipt_event_builder(
    order_id: &str,
    params: &BuildReceiptParams,
    encrypted_content: String,
) -> Result<EventBuilder, AdpError> {
    if params.proofs.is_empty() {
        return Err(AdpError::MissingTag("proof"));
    }
    if hex::decode(&params.payment_hash).is_err() || params.payment_hash.len() != 64 {
        return Err(AdpError::MalformedTag("payment_hash".into()));
    }
    let mut tags = vec![
        parse_tag(vec!["order", order_id])?,
        parse_tag(vec!["p", &params.buyer_pubkey.to_hex()])?,
        parse_tag(vec!["a", &params.listing.coordinate()])?,
        parse_tag(vec!["payment_hash", &params.payment_hash])?,
        parse_tag(vec!["amount_msat", &params.amount_msat.to_string()])?,
        parse_tag(vec!["settled_at", &params.settled_at.to_string()])?,
        parse_tag(vec!["status", "paid"])?,
        parse_tag(vec!["amount", &params.amount])?,
        parse_tag(vec!["currency", &params.currency])?,
    ];
    if let Some(root) = params.authorization {
        tags.push(parse_tag(vec!["authorization", &root.to_hex()])?);
    }
    let mut seen = HashSet::new();
    for proof in &params.proofs {
        if !seen.insert(proof.clone()) {
            return Err(AdpError::DuplicateTag("proof".into()));
        }
        tags.push(match proof {
            ReceiptProof::Zap(id) => parse_tag(vec!["proof", "zap", &id.to_hex()])?,
            ReceiptProof::Bolt11Preimage => parse_tag(vec!["proof", "bolt11-preimage"])?,
        });
    }
    Ok(EventBuilder::new(Kind::Custom(RECEIPT_KIND), encrypted_content).tags(tags))
}

pub fn build_aggregate_receipt_event_builder(
    order_id: &str,
    params: &BuildAggregateReceiptParams<'_>,
    encrypted_content: String,
) -> Result<EventBuilder, AdpError> {
    let commitment = crate::settlement::aggregate_commitment(params.gross_msat, params.legs)
        .map_err(|e| AdpError::InvalidPaymentProof(e.to_string()))?;
    let mut tags = vec![
        parse_tag(vec!["order", order_id])?,
        parse_tag(vec!["p", &params.buyer_pubkey.to_hex()])?,
        parse_tag(vec!["a", &params.listing.coordinate()])?,
        parse_tag(vec!["sale_listing", &params.listing.event.id.to_hex()])?,
        parse_tag(vec!["sale_quote", &params.quote_event_id.to_hex()])?,
        parse_tag(vec![
            "payment_set",
            "1",
            &params.gross_msat.to_string(),
            &commitment,
        ])?,
        parse_tag(vec!["settled_at", &params.settled_at.to_string()])?,
        parse_tag(vec!["status", "paid"])?,
    ];
    let processor = params.processor_pubkey.to_hex();
    tags.push(if let Some(url) = params.processor_url {
        parse_tag(vec!["sale_processor", &processor, url])?
    } else {
        parse_tag(vec!["sale_processor", &processor])?
    });
    if let Some(root) = params.authorization {
        tags.push(parse_tag(vec!["authorization", &root.to_hex()])?);
    }
    for leg in params.legs {
        let role = match leg.role {
            crate::settlement::PaymentRole::Developer => "developer",
            crate::settlement::PaymentRole::Operator => "operator",
        };
        tags.push(parse_tag(vec![
            "payment_leg",
            &leg.leg_id,
            role,
            &leg.recipient_pubkey,
            &leg.amount_msat.to_string(),
            &leg.payment_hash,
            &leg.proof_type,
            &leg.proof_reference,
            leg.accepted_terms_id.as_deref().unwrap_or(""),
        ])?);
    }
    Ok(EventBuilder::new(Kind::Custom(RECEIPT_KIND), encrypted_content).tags(tags))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AggregateReceipt {
    pub version: u16,
    pub gross_msat: u64,
    pub commitment: String,
    pub listing_event_id: EventId,
    pub quote_event_id: EventId,
    pub processor_pubkey: PublicKey,
    pub processor_url: Option<String>,
    pub legs: Vec<crate::settlement::AggregateProofLeg>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedReceipt {
    pub event: Event,
    pub order_id: String,
    pub buyer_pubkey: PublicKey,
    pub coordinate: String,
    pub status: String,
    pub authorization: Option<EventId>,
    pub payment_hash: Option<String>,
    pub amount_msat: Option<u64>,
    pub settled_at: Option<u64>,
    pub proofs: Vec<ReceiptProof>,
    pub aggregate: Option<AggregateReceipt>,
    pub predecessor: Option<EventId>,
}

fn exact_tag(
    event: &Event,
    name: &'static str,
    required: bool,
) -> Result<Option<String>, AdpError> {
    let matches = event
        .tags
        .iter()
        .map(|tag| tag.clone().to_vec())
        .filter(|values| values.first().is_some_and(|value| value == name))
        .collect::<Vec<_>>();
    if matches.len() > 1 {
        return Err(AdpError::DuplicateTag(name.into()));
    }
    let Some(values) = matches.first() else {
        return if required {
            Err(AdpError::MissingTag(name))
        } else {
            Ok(None)
        };
    };
    if values.len() < 2 || values[1].is_empty() {
        return Err(AdpError::MalformedTag(name.into()));
    }
    Ok(Some(values[1].clone()))
}

fn coordinate_developer(coordinate: &str) -> Option<PublicKey> {
    let mut parts = coordinate.splitn(3, ':');
    match (parts.next(), parts.next(), parts.next()) {
        (Some("30402"), Some(developer), Some(identifier)) if !identifier.is_empty() => {
            PublicKey::from_hex(developer).ok()
        }
        _ => None,
    }
}

fn parse_proofs(event: &Event) -> Result<Vec<ReceiptProof>, AdpError> {
    let mut proofs = Vec::new();
    let mut seen = HashSet::new();
    for tag in event.tags.iter() {
        let values = tag.clone().to_vec();
        if values.first().is_none_or(|name| name != "proof") {
            continue;
        }
        let proof = match values.as_slice() {
            [_, kind] if kind == "bolt11-preimage" => ReceiptProof::Bolt11Preimage,
            [_, kind, id] if kind == "zap" => ReceiptProof::Zap(
                EventId::from_hex(id).map_err(|_| AdpError::MalformedTag("proof".into()))?,
            ),
            _ => return Err(AdpError::MalformedTag("proof".into())),
        };
        if !seen.insert(proof.clone()) {
            return Err(AdpError::DuplicateTag("proof".into()));
        }
        proofs.push(proof);
    }
    Ok(proofs)
}

fn parse_aggregate_receipt(event: &Event) -> Result<Option<AggregateReceipt>, AdpError> {
    let values_for = |name: &str| {
        event
            .tags
            .iter()
            .map(|tag| tag.clone().to_vec())
            .filter(|values| values.first().is_some_and(|value| value == name))
            .collect::<Vec<_>>()
    };
    let payment_sets = values_for("payment_set");
    if payment_sets.is_empty() {
        if [
            "payment_leg",
            "sale_listing",
            "sale_quote",
            "sale_processor",
        ]
        .iter()
        .any(|name| !values_for(name).is_empty())
        {
            return Err(AdpError::MissingTag("payment_set"));
        }
        return Ok(None);
    }
    if payment_sets.len() != 1 {
        return Err(AdpError::DuplicateTag("payment_set".into()));
    }
    let set = &payment_sets[0];
    let [_, version, gross, commitment] = set.as_slice() else {
        return Err(AdpError::MalformedTag("payment_set".into()));
    };
    let version = version
        .parse::<u16>()
        .map_err(|_| AdpError::MalformedTag("payment_set".into()))?;
    if version != crate::settlement::SETTLEMENT_VERSION
        || commitment.len() != 64
        || !commitment
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(AdpError::MalformedTag("payment_set".into()));
    }
    let gross_msat = gross
        .parse::<u64>()
        .map_err(|_| AdpError::MalformedTag("payment_set".into()))?;
    let listing_event_id = EventId::from_hex(
        &exact_tag(event, "sale_listing", true)?.ok_or(AdpError::MissingTag("sale_listing"))?,
    )
    .map_err(|_| AdpError::MalformedTag("sale_listing".into()))?;
    let quote_event_id = EventId::from_hex(
        &exact_tag(event, "sale_quote", true)?.ok_or(AdpError::MissingTag("sale_quote"))?,
    )
    .map_err(|_| AdpError::MalformedTag("sale_quote".into()))?;
    let processors = values_for("sale_processor");
    if processors.len() != 1 {
        return Err(if processors.is_empty() {
            AdpError::MissingTag("sale_processor")
        } else {
            AdpError::DuplicateTag("sale_processor".into())
        });
    }
    let (processor, processor_url) = match processors[0].as_slice() {
        [_, processor] => (processor, None),
        [_, processor, url] if !url.is_empty() => (processor, Some(url.clone())),
        _ => return Err(AdpError::MalformedTag("sale_processor".into())),
    };
    let processor_pubkey = PublicKey::from_hex(processor)
        .map_err(|_| AdpError::MalformedTag("sale_processor".into()))?;
    let mut legs = Vec::new();
    for values in values_for("payment_leg") {
        let [_, leg_id, role, recipient, amount, payment_hash, proof_type, proof_reference, terms] =
            values.as_slice()
        else {
            return Err(AdpError::MalformedTag("payment_leg".into()));
        };
        let role = match role.as_str() {
            "developer" => crate::settlement::PaymentRole::Developer,
            "operator" => crate::settlement::PaymentRole::Operator,
            _ => return Err(AdpError::MalformedTag("payment_leg".into())),
        };
        PublicKey::from_hex(recipient).map_err(|_| AdpError::MalformedTag("payment_leg".into()))?;
        let accepted_terms_id = if terms.is_empty() {
            None
        } else {
            EventId::from_hex(terms).map_err(|_| AdpError::MalformedTag("payment_leg".into()))?;
            Some(terms.clone())
        };
        legs.push(crate::settlement::AggregateProofLeg {
            leg_id: leg_id.clone(),
            role,
            recipient_pubkey: recipient.clone(),
            amount_msat: amount
                .parse()
                .map_err(|_| AdpError::MalformedTag("payment_leg".into()))?,
            payment_hash: payment_hash.clone(),
            proof_type: proof_type.clone(),
            proof_reference: proof_reference.clone(),
            accepted_terms_id,
        });
    }
    let calculated = crate::settlement::aggregate_commitment(gross_msat, &legs)
        .map_err(|error| AdpError::InvalidPaymentProof(error.to_string()))?;
    if &calculated != commitment {
        return Err(AdpError::InvalidPaymentProof(
            "aggregate commitment mismatch".into(),
        ));
    }
    Ok(Some(AggregateReceipt {
        version,
        gross_msat,
        commitment: commitment.clone(),
        listing_event_id,
        quote_event_id,
        processor_pubkey,
        processor_url,
        legs,
    }))
}

pub fn parse_receipt_event(event: &Event) -> Result<ParsedReceipt, AdpError> {
    if event.kind.as_u16() != RECEIPT_KIND {
        return Err(AdpError::WrongKind {
            expected: RECEIPT_KIND,
            found: event.kind.as_u16(),
        });
    }
    event.verify().map_err(|_| AdpError::InvalidSignature)?;
    let order_id = exact_tag(event, "order", true)?.ok_or(AdpError::MissingTag("order"))?;
    let buyer_pubkey =
        PublicKey::from_hex(&exact_tag(event, "p", true)?.ok_or(AdpError::MissingTag("p"))?)
            .map_err(|_| AdpError::MalformedTag("p".into()))?;
    let coordinate = exact_tag(event, "a", true)?.ok_or(AdpError::MissingTag("a"))?;
    coordinate_developer(&coordinate).ok_or(AdpError::InvalidAdpReceiptCoordinate)?;
    let status = exact_tag(event, "status", true)?.ok_or(AdpError::MissingTag("status"))?;
    if !matches!(
        status.as_str(),
        "paid" | "fulfilled" | "refunded" | "disputed"
    ) {
        return Err(AdpError::MalformedTag("status".into()));
    }
    let authorization = exact_tag(event, "authorization", false)?
        .map(|value| {
            EventId::from_hex(&value).map_err(|_| AdpError::MalformedTag("authorization".into()))
        })
        .transpose()?;
    let payment_hash = exact_tag(event, "payment_hash", false)?;
    if payment_hash.as_ref().is_some_and(|hash| {
        hash.len() != 64
            || !hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }) {
        return Err(AdpError::MalformedTag("payment_hash".into()));
    }
    let amount_msat = exact_tag(event, "amount_msat", false)?
        .map(|value| {
            value
                .parse()
                .map_err(|_| AdpError::MalformedTag("amount_msat".into()))
        })
        .transpose()?;
    let settled_at = exact_tag(event, "settled_at", false)?
        .map(|value| {
            value
                .parse()
                .map_err(|_| AdpError::MalformedTag("settled_at".into()))
        })
        .transpose()?;
    let proofs = parse_proofs(event)?;
    let aggregate = parse_aggregate_receipt(event)?;
    if aggregate.is_some()
        && (payment_hash.is_some() || amount_msat.is_some() || !proofs.is_empty())
    {
        return Err(AdpError::InvalidPaymentProof(
            "aggregate receipts must omit singular payment tags".into(),
        ));
    }
    let predecessor = exact_tag(event, "e", false)?
        .map(|value| EventId::from_hex(&value).map_err(|_| AdpError::MalformedTag("e".into())))
        .transpose()?;
    Ok(ParsedReceipt {
        event: event.clone(),
        order_id,
        buyer_pubkey,
        coordinate,
        status,
        authorization,
        payment_hash,
        amount_msat,
        settled_at,
        proofs,
        aggregate,
        predecessor,
    })
}

fn valid_root(parsed: &ParsedReceipt) -> bool {
    parsed.predecessor.is_none()
        && parsed.status == "paid"
        && parsed.payment_hash.is_some()
        && parsed.amount_msat.is_some()
        && parsed.settled_at.is_some()
        && !parsed.proofs.is_empty()
}

pub fn resolve_receipt_chain<'a>(
    events: &'a [Event],
    developer: PublicKey,
    authorization: Option<&ResolvedAuthorization>,
) -> Result<&'a Event, AdpError> {
    let parsed = events
        .iter()
        .filter_map(|event| {
            parse_receipt_event(event)
                .ok()
                .map(|parsed| (event.id, (event, parsed)))
        })
        .collect::<HashMap<_, _>>()
        .into_values()
        .collect::<Vec<_>>();
    let roots = parsed
        .iter()
        .filter(|(_, receipt)| valid_root(receipt))
        .collect::<Vec<_>>();
    if roots.len() != 1 {
        return Err(AdpError::InvalidReceiptChain(
            "chain must have exactly one valid paid root".into(),
        ));
    }
    let (root_event, root) = roots[0];
    if coordinate_developer(&root.coordinate) != Some(developer) {
        return Err(AdpError::ReceiptAuthorizationDeveloperMismatch);
    }
    let delegated = root_event.pubkey != developer;
    match (delegated, root.authorization, authorization) {
        (false, None, _) => {}
        (false, Some(_), _) => return Err(AdpError::ReceiptAuthorizationDenied),
        (true, None, _) => return Err(AdpError::MissingReceiptAuthorizationAnchor),
        (true, Some(_), None) => return Err(AdpError::MissingResolvedReceiptAuthorization),
        (true, Some(anchor), Some(resolved)) => {
            if resolved.root_event_id != anchor {
                return Err(AdpError::ReceiptAuthorizationRootMismatch);
            }
            if resolved.developer_pubkey != developer {
                return Err(AdpError::ReceiptAuthorizationDeveloperMismatch);
            }
            if !resolved.authorizes(
                &root_event.pubkey,
                &root.coordinate,
                CAPABILITY_ISSUE_RECEIPT,
                root_event.created_at.as_secs(),
            ) {
                return Err(AdpError::ReceiptAuthorizationDenied);
            }
        }
    }

    let mut current_event = *root_event;
    let mut current = root.clone();
    let mut visited = HashSet::from([current_event.id]);
    loop {
        let mut valid = parsed
            .iter()
            .filter(|(_, candidate)| candidate.predecessor == Some(current_event.id))
            .filter(|(event, candidate)| {
                event.created_at > current_event.created_at
                    && candidate.order_id == root.order_id
                    && candidate.buyer_pubkey == root.buyer_pubkey
                    && candidate.coordinate == root.coordinate
                    && candidate.authorization == root.authorization
                    && candidate.payment_hash.is_none()
                    && candidate.amount_msat.is_none()
                    && candidate.settled_at.is_none()
                    && candidate.proofs.is_empty()
                    && (event.pubkey == developer
                        || (delegated && event.pubkey == root_event.pubkey))
                    && current.status != "refunded"
                    && candidate.status != "paid"
            })
            .collect::<Vec<_>>();
        if valid.len() > 1 {
            return Err(AdpError::InvalidReceiptChain(format!(
                "fork at {}",
                current_event.id
            )));
        }
        let Some((next_event, next)) = valid.pop() else {
            break;
        };
        if !visited.insert(next_event.id) {
            return Err(AdpError::InvalidReceiptChain("cycle".into()));
        }
        current_event = next_event;
        current = next.clone();
    }
    Ok(current_event)
}

pub fn latest_status_in_chain(events_for_order: &[Event]) -> Option<&Event> {
    let first = events_for_order
        .iter()
        .find_map(|event| parse_receipt_event(event).ok())?;
    let developer = coordinate_developer(&first.coordinate)?;
    resolve_receipt_chain(events_for_order, developer, None).ok()
}
