//! Operator refund tooling for partial-settlement refunds owed by this
//! operator. It is an authenticated client of this server's own recipient
//! refund routes, signed (NIP-98) with the operator key, so the server stays the
//! single writer and enforces: recipient identity, one original refund invoice
//! per leg, preimage-to-invoice verification and idempotent confirmation.
//! It never pays: the operator pays the original invoice once, with any
//! wallet, then confirms the preimage. Uncertain payments are looked up in that
//! wallet, never paid again.
use base64::Engine;
use nostr_sdk::{EventBuilder, Keys, Kind, Tag};
use serde_json::{json, Value};

pub struct OperatorRefundClient {
    base: String,
    keys: Keys,
    http: reqwest::Client,
}

fn safe_segment(value: &str) -> anyhow::Result<&str> {
    if value.is_empty()
        || value.len() > 160
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
    {
        anyhow::bail!("invalid identifier: {value}");
    }
    Ok(value)
}

impl OperatorRefundClient {
    pub fn new(server_url: &str, keys: Keys) -> anyhow::Result<Self> {
        let base = server_url.trim_end_matches('/').to_owned();
        if !(base.starts_with("http://") || base.starts_with("https://")) {
            anyhow::bail!("ADP_SERVER_URL must be an http(s) origin");
        }
        Ok(Self {
            base,
            keys,
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
        })
    }

    pub fn operator_pubkey(&self) -> String {
        self.keys.public_key().to_hex()
    }

    fn authorization(&self, url: &str) -> anyhow::Result<String> {
        let event = EventBuilder::new(Kind::Custom(27235), "")
            .tags([Tag::parse(["u", url])?, Tag::parse(["method", "POST"])?])
            .sign_with_keys(&self.keys)?;
        Ok(format!(
            "Nostr {}",
            base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(&event)?)
        ))
    }

    async fn post(&self, path: &str, body: Value) -> anyhow::Result<Value> {
        let url = format!("{}{path}", self.base);
        let response = self
            .http
            .post(&url)
            .header("Authorization", self.authorization(&url)?)
            .json(&body)
            .send()
            .await?;
        let status = response.status();
        let value: Value = response.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            anyhow::bail!(
                "server refused ({status}): {}",
                value["error"].as_str().unwrap_or("no detail")
            );
        }
        Ok(value)
    }

    /// Obligations owed by this operator key (server-side recipient scoping).
    pub async fn obligations(&self) -> anyhow::Result<Vec<Value>> {
        let value = self.post("/settlement/refunds", json!({})).await?;
        Ok(value["obligations"].as_array().cloned().unwrap_or_default())
    }

    /// Returns the original refund invoice; repeat calls return the same one.
    pub async fn prepare(&self, quote_id: &str, leg_id: &str) -> anyhow::Result<Value> {
        let path = format!(
            "/settlement/{}/refund/{}/prepare",
            safe_segment(quote_id)?,
            safe_segment(leg_id)?
        );
        self.post(&path, json!({})).await
    }

    /// Records the payment proof; the server verifies it against the original invoice.
    pub async fn confirm(
        &self,
        quote_id: &str,
        leg_id: &str,
        attempt_id: &str,
        preimage: &str,
    ) -> anyhow::Result<Value> {
        if preimage.len() != 64 || !preimage.bytes().all(|b| b.is_ascii_hexdigit()) {
            anyhow::bail!("preimage must be 32-byte hex");
        }
        let path = format!(
            "/settlement/{}/refund/{}/confirm",
            safe_segment(quote_id)?,
            safe_segment(leg_id)?
        );
        self.post(
            &path,
            json!({"attempt_id": safe_segment(attempt_id)?, "preimage": preimage}),
        )
        .await
    }
}

pub fn render_obligations(obligations: &[Value]) -> String {
    if obligations.is_empty() {
        return "No refund obligations are owed by this operator.\n".into();
    }
    let mut out = String::from("STATE        PRINCIPAL_MSAT  DUE_AT      QUOTE / LEG\n");
    for o in obligations {
        out.push_str(&format!(
            "{:<12} {:>14}  {:<10}  {} / {}\n",
            o["state"].as_str().unwrap_or("?"),
            o["principal_msat"].as_u64().unwrap_or(0),
            o["due_at"].as_u64().unwrap_or(0),
            o["quote_id"].as_str().unwrap_or("?"),
            o["leg_id"].as_str().unwrap_or("?"),
        ));
    }
    out
}

pub const PAY_ONCE_WARNING: &str = "Pay this exact invoice ONCE with your own wallet. If a previous payment attempt may have been sent, check that wallet for the payment hash first; never pay again. After payment, run `confirm` with the preimage.";
