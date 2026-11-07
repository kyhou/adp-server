//! Local-only LNURL-pay bridge to Polar LND. See polar_lnurl_fixture/README.md.
use std::{
    collections::BTreeMap,
    net::SocketAddr,
    path::Path,
    str::FromStr,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context, Result};
use axum::{
    extract::{Path as RoutePath, RawQuery, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use lightning_invoice::{Bolt11Invoice, Currency};
use serde::Deserialize;
use serde_json::json;

fn default_bind() -> SocketAddr {
    "127.0.0.1:18080".parse().expect("constant socket address")
}
fn default_min() -> u64 {
    1_000
}
fn default_max() -> u64 {
    100_000_000
}
fn default_expiry() -> u64 {
    600
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    #[serde(default = "default_bind")]
    bind: SocketAddr,
    recipients: BTreeMap<String, RecipientConfig>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecipientConfig {
    lnd_rest_url: String,
    tls_cert_path_env: String,
    macaroon_path_env: String,
    nostr_pubkey_env: String,
    node_pubkey_env: String,
    #[serde(default = "default_min")]
    min_sendable_msat: u64,
    #[serde(default = "default_max")]
    max_sendable_msat: u64,
    #[serde(default = "default_expiry")]
    invoice_expiry_secs: u64,
}

struct Recipient {
    client: reqwest::Client,
    invoice_url: reqwest::Url,
    macaroon: reqwest::header::HeaderValue,
    node_pubkey: String,
    nostr_pubkey: String,
    min: u64,
    max: u64,
    expiry: u64,
}

struct Fixture {
    origin: String,
    recipients: BTreeMap<String, Recipient>,
}

fn env_value(name: &str) -> Result<String> {
    std::env::var(name).with_context(|| format!("required environment variable {name} is missing"))
}

fn validate_config(config: &Config) -> Result<()> {
    if !config.bind.ip().is_loopback() || config.bind.port() == 0 {
        bail!("fixture must bind a loopback address with a nonzero port");
    }
    if config.recipients.is_empty() {
        bail!("at least one recipient is required");
    }
    for (name, recipient) in &config.recipients {
        if name.is_empty()
            || !name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            bail!("recipient names must contain only lowercase ASCII letters, digits, or hyphens");
        }
        if recipient.min_sendable_msat == 0
            || recipient.min_sendable_msat > recipient.max_sendable_msat
            || recipient.max_sendable_msat > i64::MAX as u64
        {
            bail!("invalid amount bounds for {name}");
        }
        // Well below ADP's 24-hour confirmation deadline; never use LND's long default.
        if !(60..=3600).contains(&recipient.invoice_expiry_secs) {
            bail!("invoice expiry for {name} must be 60..=3600 seconds");
        }
        let url = reqwest::Url::parse(&recipient.lnd_rest_url).context("invalid LND REST URL")?;
        if url.scheme() != "https"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.path() != "/"
        {
            bail!("LND REST URL for {name} must be a credential-free HTTPS origin");
        }
    }
    Ok(())
}

fn lnd_client(certificate: &[u8]) -> Result<reqwest::Client> {
    reqwest::Client::builder()
        // LND's self-signed server certificate has CA:TRUE. Rustls/webpki rejects
        // it as CaUsedAsEndEntity; native TLS verifies its trust and hostname.
        // This backend is enabled only by the example's dev-dependencies.
        .use_native_tls()
        .add_root_certificate(
            reqwest::Certificate::from_pem(certificate).context("invalid LND TLS certificate")?,
        )
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .context("could not construct LND HTTP client")
}

fn load_fixture(config: Config) -> Result<Fixture> {
    validate_config(&config)?;
    let mut recipients = BTreeMap::new();
    for (name, entry) in config.recipients {
        let certificate = std::fs::read(env_value(&entry.tls_cert_path_env)?)
            .context("could not read LND TLS certificate")?;
        let macaroon_bytes = std::fs::read(env_value(&entry.macaroon_path_env)?)
            .context("could not read binary invoice macaroon")?;
        if macaroon_bytes.is_empty() {
            bail!("empty invoice macaroon for {name}");
        }
        let mut macaroon = reqwest::header::HeaderValue::from_str(&hex::encode(macaroon_bytes))?;
        macaroon.set_sensitive(true);
        let nostr_pubkey = env_value(&entry.nostr_pubkey_env)?;
        nostr_sdk::PublicKey::from_hex(&nostr_pubkey)
            .context("invalid recipient Nostr public key")?;
        let node_pubkey = env_value(&entry.node_pubkey_env)?;
        let node_pubkey = bitcoin::secp256k1::PublicKey::from_str(&node_pubkey)
            .context("invalid LND node public key")?
            .to_string();
        let client = lnd_client(&certificate)?;
        recipients.insert(
            name,
            Recipient {
                client,
                invoice_url: reqwest::Url::parse(&entry.lnd_rest_url)?.join("/v1/invoices")?,
                macaroon,
                node_pubkey: node_pubkey.to_ascii_lowercase(),
                nostr_pubkey,
                min: entry.min_sendable_msat,
                max: entry.max_sendable_msat,
                expiry: entry.invoice_expiry_secs,
            },
        );
    }
    Ok(Fixture {
        origin: format!("http://{}", config.bind),
        recipients,
    })
}

fn router(fixture: Fixture) -> Router {
    Router::new()
        .route("/.well-known/lnurlp/:recipient", get(metadata))
        .route("/lnurlp/:recipient/callback", get(callback))
        .with_state(Arc::new(fixture))
}

fn error(status: StatusCode, reason: &'static str) -> Response {
    (status, Json(json!({"status":"ERROR", "reason":reason}))).into_response()
}

fn metadata_string(name: &str) -> String {
    json!([["text/plain", format!("Polar regtest invoice for {name}")]]).to_string()
}

async fn metadata(
    State(fixture): State<Arc<Fixture>>,
    RoutePath(name): RoutePath<String>,
) -> Response {
    let Some(recipient) = fixture.recipients.get(&name) else {
        return error(StatusCode::NOT_FOUND, "unknown recipient");
    };
    Json(json!({
        "tag":"payRequest",
        "callback":format!("{}/lnurlp/{name}/callback", fixture.origin),
        "minSendable":recipient.min, "maxSendable":recipient.max,
        "metadata":metadata_string(&name),
        "nostrPubkey":recipient.nostr_pubkey, "allowsNostr":false,
        "commentAllowed":0
    }))
    .into_response()
}

fn parse_amount(query: Option<&str>, min: u64, max: u64) -> Option<u64> {
    let pairs: Vec<_> = reqwest::Url::parse(&format!("http://localhost/?{}", query?))
        .ok()?
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    // No zap requests or other unsupported parameters are silently accepted.
    if pairs.len() != 1
        || pairs[0].0 != "amount"
        || pairs[0].1.is_empty()
        || !pairs[0].1.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let amount = pairs[0].1.parse::<u64>().ok()?;
    (amount >= min && amount <= max).then_some(amount)
}

#[derive(Deserialize)]
struct LndInvoiceResponse {
    payment_request: String,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn validate_invoice(
    invoice: &str,
    recipient: &Recipient,
    amount: u64,
    metadata: &str,
) -> Result<()> {
    let invoice =
        Bolt11Invoice::from_str(invoice).map_err(|_| anyhow::anyhow!("invalid invoice"))?;
    if invoice.currency() != Currency::Regtest
        || invoice.amount_milli_satoshis() != Some(amount)
        || invoice.recover_payee_pub_key().to_string() != recipient.node_pubkey
        || invoice.expiry_time().as_secs() != recipient.expiry
    {
        bail!("invoice network, amount, signer, or expiry mismatch");
    }
    let timestamp = invoice.duration_since_epoch().as_secs();
    let expires = timestamp
        .checked_add(recipient.expiry)
        .context("expiry overflow")?;
    if timestamp > now().saturating_add(5)
        || expires <= now()
        || expires > now().saturating_add(3605)
    {
        bail!("invoice outside valid time window");
    }
    use sha2::Digest;
    let expected = hex::encode(sha2::Sha256::digest(metadata.as_bytes()));
    match invoice.description() {
        lightning_invoice::Bolt11InvoiceDescription::Hash(hash)
            if hash.0.to_string() == expected =>
        {
            Ok(())
        }
        _ => bail!("invoice metadata hash mismatch"),
    }
}

async fn callback(
    State(fixture): State<Arc<Fixture>>,
    RoutePath(name): RoutePath<String>,
    RawQuery(query): RawQuery,
) -> Response {
    let Some(recipient) = fixture.recipients.get(&name) else {
        return error(StatusCode::NOT_FOUND, "unknown recipient");
    };
    let Some(amount) = parse_amount(query.as_deref(), recipient.min, recipient.max) else {
        return error(
            StatusCode::BAD_REQUEST,
            "expected one integer amount in millisatoshis within recipient bounds",
        );
    };
    use base64::Engine;
    use sha2::Digest;
    let metadata = metadata_string(&name);
    let hash =
        base64::engine::general_purpose::STANDARD.encode(sha2::Sha256::digest(metadata.as_bytes()));
    let response = recipient.client.post(recipient.invoice_url.clone())
        .header("Grpc-Metadata-macaroon", recipient.macaroon.clone())
        .json(&json!({"value_msat":amount.to_string(), "expiry":recipient.expiry.to_string(), "description_hash":hash}))
        .send().await;
    // Never return/log upstream error bodies, URLs, or credential-bearing errors.
    let Ok(response) = response else {
        return error(StatusCode::BAD_GATEWAY, "LND invoice service unavailable");
    };
    if !response.status().is_success() {
        return error(StatusCode::BAD_GATEWAY, "LND rejected invoice request");
    }
    let Ok(invoice) = response.json::<LndInvoiceResponse>().await else {
        return error(StatusCode::BAD_GATEWAY, "invalid LND invoice response");
    };
    if validate_invoice(&invoice.payment_request, recipient, amount, &metadata).is_err() {
        return error(StatusCode::BAD_GATEWAY, "LND invoice failed validation");
    }
    Json(json!({"pr":invoice.payment_request, "routes":[]})).into_response()
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() != 2 {
        bail!("usage: polar_lnurl_fixture <config.json>");
    }
    let config: Config = serde_json::from_slice(
        &std::fs::read(Path::new(&args[1])).context("could not read fixture config")?,
    )
    .context("invalid fixture config")?;
    let bind = config.bind;
    let fixture = load_fixture(config)?;
    let listener = tokio::net::TcpListener::bind(bind).await?;
    eprintln!("Polar regtest LNURL fixture listening on http://{bind}; invoice creation only, no payments");
    axum::serve(listener, router(fixture))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

#[cfg(test)]
#[path = "polar_lnurl_fixture/tests.rs"]
mod tests;
