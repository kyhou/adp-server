use std::str::FromStr;

use lightning_invoice::Bolt11Invoice;
use nostr_sdk::{Event, PublicKey};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum LnurlError {
    #[error("malformed lud16 address")]
    MalformedLud16,
    #[error("lnurl request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("lnurl response did not include a valid nostrPubkey")]
    MissingNostrPubkey,
    #[error("lnurl amount is outside endpoint bounds")]
    AmountOutOfRange,
    #[error("invalid invoice provenance: {0}")]
    InvalidInvoice(String),
}

#[derive(Debug, Deserialize)]
struct LnurlPayResponse {
    #[serde(rename = "nostrPubkey")]
    nostr_pubkey: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvoiceProvenance {
    pub endpoint: String,
    pub callback: String,
    pub lsp_pubkey: String,
    pub min_sendable_msat: u64,
    pub max_sendable_msat: u64,
    pub allows_nostr: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExactInvoice {
    pub provenance: InvoiceProvenance,
    pub bolt11: String,
    pub payment_hash: String,
    pub expires_at: u64,
}

#[derive(Deserialize)]
struct PayMetadata {
    callback: String,
    #[serde(rename = "minSendable")]
    min_sendable: u64,
    #[serde(rename = "maxSendable")]
    max_sendable: u64,
    #[serde(rename = "nostrPubkey")]
    nostr_pubkey: String,
    #[serde(rename = "allowsNostr", default)]
    allows_nostr: bool,
}

#[derive(Deserialize)]
struct InvoiceResponse {
    pr: String,
}

pub async fn inspect_payment_destination(
    destination: &str,
    allow_insecure_loopback: bool,
) -> Result<InvoiceProvenance, LnurlError> {
    let endpoint = if destination.contains('@') {
        let (name, domain) = split_lud16(destination)?;
        format!(
            "{}/.well-known/lnurlp/{name}",
            lud16_base_url(domain, allow_insecure_loopback)
        )
    } else {
        destination.to_string()
    };
    let endpoint_url = reqwest::Url::parse(&endpoint).map_err(|_| LnurlError::MalformedLud16)?;
    validate_transport(&endpoint_url, allow_insecure_loopback)?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let metadata: PayMetadata = client
        .get(endpoint_url.clone())
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    if metadata.min_sendable > metadata.max_sendable {
        return Err(LnurlError::AmountOutOfRange);
    }
    PublicKey::from_hex(&metadata.nostr_pubkey).map_err(|_| LnurlError::MissingNostrPubkey)?;
    let callback = reqwest::Url::parse(&metadata.callback)
        .map_err(|_| LnurlError::InvalidInvoice("malformed callback".into()))?;
    validate_transport(&callback, allow_insecure_loopback)?;
    Ok(InvoiceProvenance {
        endpoint: endpoint_url.to_string(),
        callback: callback.to_string(),
        lsp_pubkey: metadata.nostr_pubkey,
        min_sendable_msat: metadata.min_sendable,
        max_sendable_msat: metadata.max_sendable,
        allows_nostr: metadata.allows_nostr,
    })
}

#[derive(Debug, Clone, Copy)]
pub struct ZapRequestContext<'a> {
    pub listing_coordinate: &'a str,
    pub quote_id: &'a str,
    pub leg_id: &'a str,
}

pub(crate) fn unique_tag(event: &Event, name: &str) -> Option<String> {
    let mut values = event
        .tags
        .iter()
        .map(|tag| tag.clone().to_vec())
        .filter(|values| values.first().is_some_and(|value| value == name));
    let value = values.next()?;
    if values.next().is_some() || value.len() != 2 || value[1].is_empty() {
        return None;
    }
    Some(value[1].clone())
}

pub(crate) fn valid_zap_request(
    event: &Event,
    buyer: PublicKey,
    recipient: PublicKey,
    amount_msat: u64,
    context: ZapRequestContext<'_>,
) -> bool {
    event.kind.as_u16() == 9734
        && event.pubkey == buyer
        && event.verify().is_ok()
        && unique_tag(event, "p").as_deref() == Some(recipient.to_hex().as_str())
        && unique_tag(event, "amount").as_deref() == Some(amount_msat.to_string().as_str())
        && unique_tag(event, "a").as_deref() == Some(context.listing_coordinate)
        && unique_tag(event, "sale_quote").as_deref() == Some(context.quote_id)
        && unique_tag(event, "payment_leg").as_deref() == Some(context.leg_id)
}

pub async fn request_exact_invoice(
    destination: &str,
    amount_msat: u64,
    buyer: PublicKey,
    recipient: PublicKey,
    zap_request: Option<&Event>,
    zap_context: Option<ZapRequestContext<'_>>,
    confirmation_expires_at: u64,
    allow_insecure_loopback: bool,
) -> Result<ExactInvoice, LnurlError> {
    let provenance = inspect_payment_destination(destination, allow_insecure_loopback).await?;
    if amount_msat < provenance.min_sendable_msat || amount_msat > provenance.max_sendable_msat {
        return Err(LnurlError::AmountOutOfRange);
    }
    match (zap_request, zap_context) {
        (Some(event), Some(context))
            if provenance.allows_nostr
                && valid_zap_request(event, buyer, recipient, amount_msat, context) => {}
        (None, None) => {}
        _ => {
            return Err(LnurlError::InvalidInvoice(
                "invalid buyer zap request".into(),
            ))
        }
    }
    let mut callback = reqwest::Url::parse(&provenance.callback)
        .map_err(|_| LnurlError::InvalidInvoice("malformed callback".into()))?;
    {
        let mut query = callback.query_pairs_mut();
        query.append_pair("amount", &amount_msat.to_string());
        if let Some(event) = zap_request {
            query.append_pair(
                "nostr",
                &serde_json::to_string(event)
                    .map_err(|_| LnurlError::InvalidInvoice("zap serialization".into()))?,
            );
        }
    }
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let response: InvoiceResponse = client
        .get(callback)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let invoice = Bolt11Invoice::from_str(&response.pr)
        .map_err(|error| LnurlError::InvalidInvoice(error.to_string()))?;
    if invoice.amount_milli_satoshis() != Some(amount_msat) {
        return Err(LnurlError::InvalidInvoice("amount mismatch".into()));
    }
    let expires_at = invoice
        .duration_since_epoch()
        .as_secs()
        .checked_add(invoice.expiry_time().as_secs())
        .ok_or_else(|| LnurlError::InvalidInvoice("expiry overflow".into()))?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| LnurlError::InvalidInvoice("system clock before epoch".into()))?
        .as_secs();
    if expires_at <= now || expires_at > confirmation_expires_at {
        return Err(LnurlError::InvalidInvoice(
            "invoice expiry outside quote window".into(),
        ));
    }
    Ok(ExactInvoice {
        provenance,
        bolt11: response.pr,
        payment_hash: invoice.payment_hash().to_string(),
        expires_at,
    })
}

fn validate_transport(url: &reqwest::Url, allow_insecure_loopback: bool) -> Result<(), LnurlError> {
    if url.scheme() == "https" {
        return Ok(());
    }
    if url.scheme() == "http"
        && allow_insecure_loopback
        && url.host_str().is_some_and(is_loopback_domain)
    {
        return Ok(());
    }
    Err(LnurlError::InvalidInvoice(
        "insecure non-loopback endpoint".into(),
    ))
}

pub async fn resolve_lud16_lsp_pubkey(
    lud16: &str,
    allow_insecure_loopback: bool,
) -> Result<PublicKey, LnurlError> {
    let (_, domain) = split_lud16(lud16)?;
    resolve_lud16_lsp_pubkey_with_base(lud16, &lud16_base_url(domain, allow_insecure_loopback))
        .await
}

pub async fn resolve_lud16_lsp_pubkey_with_base(
    lud16: &str,
    base_url: &str,
) -> Result<PublicKey, LnurlError> {
    let (name, _) = split_lud16(lud16)?;
    let url = format!(
        "{}/.well-known/lnurlp/{}",
        base_url.trim_end_matches('/'),
        name
    );
    let response: LnurlPayResponse = reqwest::get(url).await?.error_for_status()?.json().await?;
    let pubkey = response
        .nostr_pubkey
        .ok_or(LnurlError::MissingNostrPubkey)?;
    PublicKey::from_hex(&pubkey).map_err(|_| LnurlError::MissingNostrPubkey)
}

fn split_lud16(lud16: &str) -> Result<(&str, &str), LnurlError> {
    let (name, domain) = lud16.split_once('@').ok_or(LnurlError::MalformedLud16)?;
    if name.is_empty() || domain.is_empty() || domain.contains('@') {
        return Err(LnurlError::MalformedLud16);
    }
    Ok((name, domain))
}

fn lud16_base_url(domain: &str, allow_insecure_loopback: bool) -> String {
    let scheme = if allow_insecure_loopback && is_loopback_domain(domain) {
        "http"
    } else {
        "https"
    };
    format!("{scheme}://{domain}")
}

fn is_loopback_domain(domain: &str) -> bool {
    if domain == "::1" {
        return true;
    }
    let host = domain.split(':').next().unwrap_or(domain);
    matches!(host, "localhost" | "127.0.0.1")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_domain_requires_exact_host_match() {
        for domain in [
            "localhost",
            "localhost:8081",
            "127.0.0.1",
            "127.0.0.1:8081",
            "::1",
        ] {
            assert!(is_loopback_domain(domain), "{domain} should be loopback");
        }

        for domain in ["example.com", "notlocalhost.com", "127.0.0.1.evil.com"] {
            assert!(
                !is_loopback_domain(domain),
                "{domain} should not be loopback"
            );
        }
    }

    #[test]
    fn loopback_without_flag_still_uses_https() {
        assert_eq!(
            lud16_base_url("localhost:8081", false),
            "https://localhost:8081"
        );
    }

    #[test]
    fn loopback_with_flag_uses_http() {
        assert_eq!(
            lud16_base_url("localhost:8081", true),
            "http://localhost:8081"
        );
    }

    #[test]
    fn non_loopback_with_flag_still_uses_https() {
        assert_eq!(lud16_base_url("example.com", true), "https://example.com");
    }
}
