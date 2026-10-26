use std::env;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacyPaidSalesPolicy {
    AcceptFeeFree,
    Decline,
}

/// Typed configuration for the experimental split-settlement profile.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SettlementConfig {
    pub version: u16,
    pub fee: adp_core::FeePolicy,
    pub payment_destination: Option<String>,
    pub terms_event_id: Option<String>,
    pub legacy_paid_sales: LegacyPaidSalesPolicy,
}

impl SettlementConfig {
    pub fn from_env() -> Self {
        let fee = env::var("ADP_FEE_POLICY")
            .ok()
            .map(|value| {
                serde_json::from_str(&value).expect("ADP_FEE_POLICY must be valid FeePolicy JSON")
            })
            .unwrap_or(adp_core::FeePolicy::None);
        let legacy_paid_sales = match env::var("ADP_LEGACY_PAID_SALES").as_deref() {
            Ok("decline") => LegacyPaidSalesPolicy::Decline,
            Ok("accept_fee_free") | Err(_) => LegacyPaidSalesPolicy::AcceptFeeFree,
            Ok(_) => panic!("ADP_LEGACY_PAID_SALES must be accept_fee_free or decline"),
        };
        Self {
            version: adp_core::SETTLEMENT_VERSION,
            fee,
            payment_destination: env::var("ADP_PAYMENT_DESTINATION").ok(),
            terms_event_id: env::var("ADP_TERMS_EVENT_ID").ok(),
            legacy_paid_sales,
        }
    }

    pub fn split_enabled(&self) -> bool {
        self.payment_destination.is_some() && self.terms_event_id.is_some()
    }
}

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
    /// bech32 `nsec1...` or hex secret key for the operator identity that signs
    /// server announcements and provisioning attestations.
    pub operator_nsec: String,
    pub master_key: [u8; 32],
    pub allow_insecure_lnurl: bool,
    pub download_token_ttl_secs: i64,
    pub file_reverify_interval_secs: u64,
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
            operator_nsec: env::var("ADP_OPERATOR_NSEC").expect(
                "ADP_OPERATOR_NSEC must be set to the operator identity key \
                 (bech32 nsec or hex)",
            ),
            master_key: decode_master_key(),
            allow_insecure_lnurl: env::var("ADP_ALLOW_INSECURE_LNURL")
                .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
                .unwrap_or(false),
            download_token_ttl_secs: env::var("ADP_DOWNLOAD_TOKEN_TTL_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(900),
            file_reverify_interval_secs: env::var("ADP_FILE_REVERIFY_INTERVAL_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(3600),
        }
    }
}

fn decode_master_key() -> [u8; 32] {
    let value = env::var("ADP_MASTER_KEY")
        .expect("ADP_MASTER_KEY must be set to 32 raw bytes encoded as 64 hex characters");
    let bytes = hex::decode(value).expect("ADP_MASTER_KEY must be valid hex");
    bytes
        .try_into()
        .expect("ADP_MASTER_KEY must decode to exactly 32 bytes")
}
