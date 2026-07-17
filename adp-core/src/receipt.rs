use std::collections::{HashMap, HashSet};

use nostr::nips::nip44;
use nostr::{Event, EventBuilder, EventId, Keys, Kind, PublicKey, Tag};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::AdpError;
use crate::listing::AdpListing;
use crate::provisioning::{resolve_authorization, ResolvedAuthorization};

/// The NIP-102 marketplace receipt event kind.
pub const RECEIPT_KIND: u16 = 1020;

/// Generates a new order id (UUID v4), per NIP-102's recommendation.
pub fn new_order_id() -> String {
    Uuid::new_v4().to_string()
}

/// A single line item within a receipt's encrypted `.content`, per NIP-102's content schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceiptItem {
    pub product_id: String,
    pub name: String,
    pub quantity: u64,
    pub unit_price: String,
    pub currency: String,
}

/// The plaintext JSON object that, once NIP-44 encrypted, becomes a receipt's `.content`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceiptContent {
    pub order_id: String,
    pub items: Vec<ReceiptItem>,
    pub subtotal: String,
    pub tax: String,
    pub shipping: String,
    pub total: String,
    pub currency: String,
    pub notes: Option<String>,
    pub fulfilled_at: u64,
}

fn parse_tag(values: Vec<&str>) -> Result<Tag, AdpError> {
    Tag::parse(values).map_err(|e| AdpError::Nostr(e.to_string()))
}

/// NIP-44 encrypts `plaintext` to `buyer_pubkey`, using the signing key's own secret
/// key as the sender side of the conversation key derivation. Per NIP-102, this is
/// what makes `.content` "readable only by the merchant and the customer."
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

/// Inputs needed to build a `kind:1020` receipt's tags. `content` (already NIP-44
/// encrypted) is supplied separately to `build_receipt_event_builder`.
pub struct BuildReceiptParams<'a> {
    pub listing: &'a AdpListing,
    pub buyer_pubkey: PublicKey,
    /// Set when the payment proof is a zap receipt (`e` tag path).
    pub payment_proof_event_id: Option<String>,
    /// Set together with `preimage` when using the self-contained proof path.
    pub bolt11: Option<String>,
    pub preimage: Option<String>,
    /// Immutable kind 30406 authorization root used for portable delegated authority.
    pub authorization_event_id: Option<EventId>,
    pub amount: String,
    pub currency: String,
}

/// Builds (but does not sign) the `kind:1020` receipt event for a confirmed purchase.
/// `encrypted_content` must already be NIP-44 encrypted — see `encrypt_receipt_content`.
/// Caller signs with either the developer's key or an authorized fulfillment key via
/// `.sign_with_keys(...)`.
pub fn build_receipt_event_builder(
    order_id: &str,
    params: &BuildReceiptParams,
    encrypted_content: String,
) -> Result<EventBuilder, AdpError> {
    let mut tags = vec![
        parse_tag(vec!["order", order_id])?,
        parse_tag(vec!["p", &params.buyer_pubkey.to_hex()])?,
        parse_tag(vec!["a", &params.listing.coordinate()])?,
        parse_tag(vec!["status", "paid"])?,
        parse_tag(vec!["amount", &params.amount])?,
        parse_tag(vec!["currency", &params.currency])?,
    ];

    if let Some(eid) = &params.payment_proof_event_id {
        tags.push(parse_tag(vec!["e", eid])?);
    }
    if let Some(bolt11) = &params.bolt11 {
        tags.push(parse_tag(vec!["bolt11", bolt11])?);
    }
    if let Some(preimage) = &params.preimage {
        tags.push(parse_tag(vec!["preimage", preimage])?);
    }
    if let Some(authorization_event_id) = params.authorization_event_id {
        tags.push(parse_tag(vec![
            "authorization_event",
            &authorization_event_id.to_hex(),
        ])?);
    }

    Ok(EventBuilder::new(Kind::Custom(RECEIPT_KIND), encrypted_content).tags(tags))
}

/// A `kind:1020` event's tags, extracted into a structured form. `.content` is left
/// encrypted here — decryption requires the buyer's or signer's private key and is
/// out of scope for adp-server, which only needs the public tags to verify ownership.
#[derive(Debug, Clone)]
pub struct ParsedReceipt {
    pub order_id: String,
    pub buyer_pubkey: PublicKey,
    pub coordinate: String,
    pub status: String,
    pub amount: Option<String>,
    pub currency: Option<String>,
    pub authorization_event_id: Option<EventId>,
    /// The `e` tag, if present — either a payment-proof reference (zap receipt id)
    /// on an initial receipt, or a reference to the immediately preceding receipt
    /// on a status-update event. Disambiguating the two requires chain context;
    /// see `latest_status_in_chain`.
    pub e_tag: Option<String>,
}

fn exact_receipt_tag(
    event: &Event,
    name: &str,
    required: bool,
    allow_trailing_fields: bool,
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
            Err(AdpError::MissingTag(match name {
                "order" => "order",
                "p" => "p",
                "a" => "a",
                "status" => "status",
                _ => "receipt tag",
            }))
        } else {
            Ok(None)
        };
    };
    if values.len() < 2 || values[1].is_empty() || (!allow_trailing_fields && values.len() != 2) {
        return Err(AdpError::MalformedTag(name.into()));
    }
    Ok(Some(values[1].clone()))
}

fn receipt_coordinate_developer(coordinate: &str) -> Option<PublicKey> {
    let mut parts = coordinate.splitn(3, ':');
    let kind = parts.next()?;
    let developer = parts.next()?;
    let identifier = parts.next()?;
    if kind != "30402" || identifier.is_empty() {
        return None;
    }
    PublicKey::from_hex(developer).ok()
}

/// Parses and verifies a signed `kind:1020` event's public receipt metadata.
pub fn parse_receipt_event(event: &Event) -> Result<ParsedReceipt, AdpError> {
    if event.kind.as_u16() != RECEIPT_KIND {
        return Err(AdpError::WrongKind {
            expected: RECEIPT_KIND,
            found: event.kind.as_u16(),
        });
    }
    if event.verify().is_err() {
        return Err(AdpError::InvalidSignature);
    }

    let order_id =
        exact_receipt_tag(event, "order", true, false)?.ok_or(AdpError::MissingTag("order"))?;
    let buyer_pubkey = PublicKey::from_hex(
        &exact_receipt_tag(event, "p", true, true)?.ok_or(AdpError::MissingTag("p"))?,
    )
    .map_err(|_| AdpError::MalformedTag("p".into()))?;
    let coordinate = exact_receipt_tag(event, "a", true, true)?.ok_or(AdpError::MissingTag("a"))?;
    receipt_coordinate_developer(&coordinate).ok_or(AdpError::InvalidAdpReceiptCoordinate)?;
    let status = exact_receipt_tag(event, "status", false, false)?.unwrap_or_else(|| "paid".into());
    if !matches!(
        status.as_str(),
        "paid" | "fulfilled" | "refunded" | "disputed"
    ) {
        return Err(AdpError::MalformedTag("status".into()));
    }
    let amount = exact_receipt_tag(event, "amount", false, false)?;
    let currency = exact_receipt_tag(event, "currency", false, false)?;
    let e_tag = exact_receipt_tag(event, "e", false, true)?;
    if let Some(value) = &e_tag {
        EventId::from_hex(value).map_err(|_| AdpError::MalformedTag("e".into()))?;
    }
    let authorization_event_id = exact_receipt_tag(event, "authorization_event", false, false)?
        .map(|value| {
            EventId::from_hex(&value)
                .map_err(|_| AdpError::MalformedTag("authorization_event".into()))
        })
        .transpose()?;

    Ok(ParsedReceipt {
        order_id,
        buyer_pubkey,
        coordinate,
        status,
        amount,
        currency,
        authorization_event_id,
        e_tag,
    })
}

fn invalid_receipt_chain(error: impl ToString) -> AdpError {
    AdpError::InvalidReceiptChain(error.to_string())
}

/// Resolves one signed ADP receipt chain and returns its authoritative tip.
///
/// The supplied authorization is treated as an untrusted portable dependency: its signed
/// events are reparsed and resolved before any detached public fields are consulted.
pub fn resolve_receipt_chain<'a>(
    events: &'a [Event],
    developer: PublicKey,
    authorization: Option<&ResolvedAuthorization>,
) -> Result<&'a Event, AdpError> {
    if events.is_empty() {
        return Err(invalid_receipt_chain("chain is empty"));
    }

    let parsed = events
        .iter()
        .map(parse_receipt_event)
        .collect::<Result<Vec<_>, _>>()?;
    let by_id = events
        .iter()
        .enumerate()
        .map(|(index, event)| (event.id, index))
        .collect::<HashMap<_, _>>();
    if by_id.len() != events.len() {
        return Err(invalid_receipt_chain("duplicate event"));
    }

    let coordinate_developer = receipt_coordinate_developer(&parsed[0].coordinate)
        .ok_or(AdpError::InvalidAdpReceiptCoordinate)?;
    if coordinate_developer != developer {
        return Err(AdpError::ReceiptAuthorizationDeveloperMismatch);
    }

    for node in &parsed {
        if node.order_id != parsed[0].order_id {
            return Err(invalid_receipt_chain("order invariant changed"));
        }
        if node.buyer_pubkey != parsed[0].buyer_pubkey {
            return Err(invalid_receipt_chain("p invariant changed"));
        }
        if node.coordinate != parsed[0].coordinate {
            return Err(invalid_receipt_chain("a invariant changed"));
        }
        if node.authorization_event_id != parsed[0].authorization_event_id {
            return Err(invalid_receipt_chain(
                "authorization_event invariant changed",
            ));
        }
    }

    let resolved_authorization = if let Some(anchor) = parsed[0].authorization_event_id {
        let supplied = authorization.ok_or(AdpError::MissingResolvedReceiptAuthorization)?;
        let resolved = resolve_authorization(supplied.root_event_id, &supplied.events)
            .map_err(|error| AdpError::InvalidReceiptAuthorizationDependency(error.to_string()))?;
        if resolved != *supplied {
            return Err(AdpError::InvalidReceiptAuthorizationDependency(
                "detached resolved metadata".into(),
            ));
        }
        if resolved.root_event_id != anchor {
            return Err(AdpError::ReceiptAuthorizationRootMismatch);
        }
        if resolved.developer_pubkey != developer {
            return Err(AdpError::ReceiptAuthorizationDeveloperMismatch);
        }
        Some(resolved)
    } else {
        None
    };

    let predecessors = parsed
        .iter()
        .map(|node| {
            node.e_tag
                .as_deref()
                .map(EventId::from_hex)
                .transpose()
                .map_err(|_| AdpError::MalformedTag("e".into()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let roots = parsed
        .iter()
        .enumerate()
        .filter(|(index, node)| {
            node.status == "paid" && predecessors[*index].is_none_or(|id| !by_id.contains_key(&id))
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if roots.len() != 1 {
        return Err(invalid_receipt_chain(
            "chain must have exactly one paid root",
        ));
    }
    let root = roots[0];

    let mut successors = HashMap::new();
    for (index, predecessor) in predecessors.iter().enumerate() {
        if index == root {
            continue;
        }
        let predecessor =
            predecessor.ok_or_else(|| invalid_receipt_chain("missing predecessor"))?;
        if !by_id.contains_key(&predecessor) {
            return Err(invalid_receipt_chain(format!(
                "missing predecessor {predecessor}"
            )));
        }
        if successors.insert(predecessor, index).is_some() {
            return Err(invalid_receipt_chain(format!("fork at {predecessor}")));
        }
    }

    let mut ordered = Vec::with_capacity(events.len());
    let mut visited = HashSet::with_capacity(events.len());
    let mut current = root;
    loop {
        if !visited.insert(events[current].id) {
            return Err(invalid_receipt_chain("cycle"));
        }
        ordered.push(current);
        match successors.get(&events[current].id) {
            Some(next) => current = *next,
            None => break,
        }
    }
    if ordered.len() != events.len() {
        return Err(invalid_receipt_chain("disconnected events"));
    }

    for (position, index) in ordered.iter().copied().enumerate() {
        let event = &events[index];
        let node = &parsed[index];
        if position > 0 {
            let predecessor = &events[ordered[position - 1]];
            if predecessors[index] != Some(predecessor.id) {
                return Err(invalid_receipt_chain("predecessor is not the current tip"));
            }
            if event.created_at <= predecessor.created_at {
                return Err(invalid_receipt_chain(
                    "successor timestamp does not increase",
                ));
            }
            if parsed[ordered[position - 1]].status == "refunded" {
                return Err(invalid_receipt_chain("refund is terminal"));
            }
        }

        let direct = event.pubkey == developer;
        match (position == 0, direct, resolved_authorization.as_ref()) {
            (true, true, None) | (false, true, _) => {}
            (true, false, None) | (false, false, None) => {
                return Err(AdpError::MissingReceiptAuthorizationAnchor);
            }
            (_, _, Some(resolved)) => {
                if !resolved.authorizes(&event.pubkey, &node.coordinate, event.created_at.as_secs())
                {
                    return Err(AdpError::ReceiptAuthorizationDenied);
                }
            }
        }
    }

    let tip = ordered
        .last()
        .copied()
        .ok_or_else(|| invalid_receipt_chain("chain is empty"))?;
    Ok(&events[tip])
}

/// Compatibility helper for fully validated direct-developer receipt chains.
///
/// Portable delegated ownership must use [`resolve_receipt_chain`] with the complete signed
/// authorization lifecycle; this wrapper intentionally cannot validate delegated authority.
pub fn latest_status_in_chain(events_for_order: &[Event]) -> Option<&Event> {
    let first = parse_receipt_event(events_for_order.first()?).ok()?;
    let developer = receipt_coordinate_developer(&first.coordinate)?;
    resolve_receipt_chain(events_for_order, developer, None).ok()
}
