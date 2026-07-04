use std::env;

#[derive(Clone)]
pub struct Config {
    /// This server's own public base URL — MUST exactly match the value used in the
    /// `server` tags of listings this instance is authorized to distribute, and is
    /// used to construct the exact `u` tag expected on inbound NIP-98 tokens.
    pub server_url: String,
    pub bind_addr: String,
    pub db_path: String,
    pub files_dir: String,
    pub relays: Vec<String>,
    /// bech32 `nsec1...` or hex secret key for the DEDICATED fulfillment key.
    /// This MUST NOT be the developer's identity/listing-signing key — see
    /// ADP-01.md "Security Considerations" § fulfillment key custody.
    pub fulfillment_nsec: String,
    pub download_token_ttl_secs: i64,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            server_url: env::var("ADP_SERVER_URL")
                .unwrap_or_else(|_| "http://localhost:8080".to_string()),
            bind_addr: env::var("ADP_BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_string()),
            db_path: env::var("ADP_DB_PATH").unwrap_or_else(|_| "./data/adp.db".to_string()),
            files_dir: env::var("ADP_FILES_DIR").unwrap_or_else(|_| "./data/files".to_string()),
            relays: env::var("ADP_RELAYS")
                .unwrap_or_else(|_| "wss://relay.damus.io,wss://nos.lol".to_string())
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            fulfillment_nsec: env::var("ADP_FULFILLMENT_NSEC").expect(
                "ADP_FULFILLMENT_NSEC must be set to a DEDICATED fulfillment key \
                 (bech32 nsec or hex) — never the developer's own identity key",
            ),
            download_token_ttl_secs: env::var("ADP_DOWNLOAD_TOKEN_TTL_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(900),
        }
    }
}
