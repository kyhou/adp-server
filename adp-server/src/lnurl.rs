use nostr_sdk::PublicKey;
use serde::Deserialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum LnurlError {
    #[error("malformed lud16 address")]
    MalformedLud16,
    #[error("lnurl request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("lnurl response did not include a valid nostrPubkey")]
    MissingNostrPubkey,
}

#[derive(Debug, Deserialize)]
struct LnurlPayResponse {
    #[serde(rename = "nostrPubkey")]
    nostr_pubkey: Option<String>,
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
        for domain in ["localhost", "localhost:8081", "127.0.0.1", "127.0.0.1:8081", "::1"] {
            assert!(is_loopback_domain(domain), "{domain} should be loopback");
        }

        for domain in ["example.com", "notlocalhost.com", "127.0.0.1.evil.com"] {
            assert!(!is_loopback_domain(domain), "{domain} should not be loopback");
        }
    }

    #[test]
    fn loopback_without_flag_still_uses_https() {
        assert_eq!(lud16_base_url("localhost:8081", false), "https://localhost:8081");
    }

    #[test]
    fn loopback_with_flag_uses_http() {
        assert_eq!(lud16_base_url("localhost:8081", true), "http://localhost:8081");
    }

    #[test]
    fn non_loopback_with_flag_still_uses_https() {
        assert_eq!(lud16_base_url("example.com", true), "https://example.com");
    }
}
