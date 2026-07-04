use std::collections::HashSet;

use nostr::nips::nip44;
use nostr::{Event, EventBuilder, Keys, Kind, PublicKey, Tag};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::AdpError;
use crate::listing::AdpListing;

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
    /// The `e` tag, if present — either a payment-proof reference (zap receipt id)
    /// on an initial receipt, or a reference to the immediately preceding receipt
    /// on a status-update event. Disambiguating the two requires chain context;
    /// see `latest_status_in_chain`.
    pub e_tag: Option<String>,
}

/// Parses a `kind:1020` event's required tags. Does not verify the event's signature.
pub fn parse_receipt_event(event: &Event) -> Result<ParsedReceipt, AdpError> {
    if event.kind.as_u16() != RECEIPT_KIND {
        return Err(AdpError::MissingTag("kind:1020"));
    }

    let mut order_id = None;
    let mut buyer_pubkey = None;
    let mut coordinate = None;
    let mut status = None;
    let mut amount = None;
    let mut currency = None;
    let mut e_tag = None;

    for t in event.tags.iter() {
        let v = t.clone().to_vec();
        match v.first().map(|s| s.as_str()) {
            Some("order") => order_id = v.get(1).cloned(),
            Some("p") => {
                if let Some(pk) = v.get(1) {
                    buyer_pubkey = PublicKey::from_hex(pk).ok();
                }
            }
            Some("a") => coordinate = v.get(1).cloned(),
            Some("status") => status = v.get(1).cloned(),
            Some("amount") => amount = v.get(1).cloned(),
            Some("currency") => currency = v.get(1).cloned(),
            Some("e") => e_tag = v.get(1).cloned(),
            _ => {}
        }
    }

    Ok(ParsedReceipt {
        order_id: order_id.ok_or(AdpError::MissingTag("order"))?,
        buyer_pubkey: buyer_pubkey.ok_or(AdpError::MissingTag("p"))?,
        coordinate: coordinate.ok_or(AdpError::MissingTag("a"))?,
        status: status.ok_or(AdpError::MissingTag("status"))?,
        amount,
        currency,
        e_tag,
    })
}

/// Given all `kind:1020` events sharing a single `order` id, walks the append-only
/// status chain and returns the terminal (authoritative) event — the one no other
/// event in the set references via its `e` tag.
///
/// Ambiguous or malformed chains return `None` instead of falling back to relay-controlled
/// timestamps. Callers should treat that as no authoritative ownership status.
pub fn latest_status_in_chain(events_for_order: &[Event]) -> Option<&Event> {
    if events_for_order.is_empty() {
        return None;
    }

    let referenced: HashSet<String> = events_for_order
        .iter()
        .filter_map(|e| {
            e.tags.iter().find_map(|t| {
                let v = t.clone().to_vec();
                (v.first().map(|k| k == "e").unwrap_or(false))
                    .then(|| v.get(1).cloned())
                    .flatten()
            })
        })
        .collect();

    let tips: Vec<&Event> = events_for_order
        .iter()
        .filter(|e| !referenced.contains(&e.id.to_hex()))
        .collect();

    if tips.len() == 1 {
        Some(tips[0])
    } else {
        None
    }
}
