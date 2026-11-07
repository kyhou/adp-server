use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use adp_core::{ADP_VERSION, EXPERIMENTAL_ADP_CAMPAIGN_KIND, EXPERIMENTAL_ENTITLEMENT_GRANT_KIND};
use adp_server::config::Config;
use adp_server::keystore::KeyStore;
use adp_server::relay::{Relay, RelayLookup, RelayResult, MIN_SUCCESSFUL_RELAYS};
use adp_server::routes;
use adp_server::state::AppState;
use adp_server::storage::Storage;
use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use nostr_sdk::{Event, Keys, PublicKey};
use tower::ServiceExt;

#[derive(Clone)]
struct EmptyRelay {
    server_url: String,
    published: Arc<Mutex<Vec<Event>>>,
    listings: Arc<Mutex<HashMap<(PublicKey, String), Event>>>,
    listing_lookup: Arc<Mutex<Option<RelayLookup<Event>>>>,
    receipts_lookup: Arc<Mutex<Option<RelayLookup<Vec<Event>>>>>,
}

impl EmptyRelay {
    fn new() -> Self {
        Self {
            server_url: "http://localhost:8080".to_string(),
            published: Arc::new(Mutex::new(Vec::new())),
            listings: Arc::new(Mutex::new(HashMap::new())),
            listing_lookup: Arc::new(Mutex::new(None)),
            receipts_lookup: Arc::new(Mutex::new(None)),
        }
    }
}

#[async_trait]
impl Relay for EmptyRelay {
    fn server_url(&self) -> &str {
        &self.server_url
    }

    async fn publish(&self, event: &Event) -> RelayResult<()> {
        self.published.lock().expect("lock").push(event.clone());
        Ok(())
    }

    async fn fetch_listing(
        &self,
        developer_pubkey: PublicKey,
        d_tag: &str,
    ) -> RelayResult<Option<Event>> {
        Ok(self
            .listings
            .lock()
            .expect("lock")
            .get(&(developer_pubkey, d_tag.to_string()))
            .cloned())
    }

    async fn fetch_receipts(
        &self,
        _buyer_pubkey: PublicKey,
        _game_coordinate: &str,
    ) -> RelayResult<Vec<Event>> {
        Ok(Vec::new())
    }

    async fn lookup_listing(&self, developer_pubkey: PublicKey, d_tag: &str) -> RelayLookup<Event> {
        if let Some(outcome) = self.listing_lookup.lock().expect("lock").clone() {
            return outcome;
        }
        self.listings
            .lock()
            .expect("lock")
            .get(&(developer_pubkey, d_tag.to_string()))
            .cloned()
            .map(RelayLookup::Found)
            .unwrap_or(RelayLookup::ConfirmedAbsent {
                successful_relays: MIN_SUCCESSFUL_RELAYS,
            })
    }

    async fn lookup_receipts(
        &self,
        _buyer_pubkey: PublicKey,
        _game_coordinate: &str,
    ) -> RelayLookup<Vec<Event>> {
        if let Some(outcome) = self.receipts_lookup.lock().expect("lock").clone() {
            return outcome;
        }
        RelayLookup::ConfirmedAbsent {
            successful_relays: MIN_SUCCESSFUL_RELAYS,
        }
    }
}

async fn app() -> axum::Router {
    let dir = tempfile::tempdir().expect("tempdir").keep();
    let db_path = dir.join("adp.db");
    let storage = Storage::connect(db_path.to_str().expect("utf8 db path"))
        .await
        .expect("connect storage");
    let operator = Keys::generate();
    let config = Config {
        server_url: "http://localhost:8080".to_string(),
        bind_addr: "127.0.0.1:0".to_string(),
        db_path: db_path.to_string_lossy().to_string(),
        files_dir: dir.join("files").to_string_lossy().to_string(),
        max_upload_bytes: 8 * 1024 * 1024 * 1024,
        relays: vec!["ws://relay-a".to_string(), "ws://relay-b".to_string()],
        operator_nsec: operator.secret_key().to_secret_hex(),
        master_key: [5u8; 32],
        allow_insecure_lnurl: false,
        download_token_ttl_secs: 900,
        file_reverify_interval_secs: 3600,
    };
    routes::router(AppState {
        config: Arc::new(config),
        storage: storage.clone(),
        relay: Arc::new(EmptyRelay::new()),
        keystore: Arc::new(KeyStore::new(storage.pool(), [5u8; 32])),
        operator_keys: operator,
    })
}

#[tokio::test]
async fn health_endpoint_returns_ok() {
    let response = app()
        .await
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn well_known_advertises_version_and_provisional_kinds() {
    let response = app()
        .await
        .oneshot(
            Request::builder()
                .uri("/.well-known/adp")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(json["adp_version"], ADP_VERSION);
    assert_eq!(
        json["event_kinds"]["entitlement_grant"],
        EXPERIMENTAL_ENTITLEMENT_GRANT_KIND
    );
    assert_eq!(
        json["event_kinds"]["adp_campaign"],
        EXPERIMENTAL_ADP_CAMPAIGN_KIND
    );
    assert_eq!(json["event_kind_status"], "provisional");
}

#[tokio::test]
async fn empty_relay_supports_configurable_unavailable_lookups() {
    let relay = EmptyRelay::new();
    let listing_unavailable = RelayLookup::Unavailable {
        successful_relays: 1,
        failed_relays: 1,
    };
    let receipts_unavailable = RelayLookup::Unavailable {
        successful_relays: 1,
        failed_relays: 1,
    };
    *relay.listing_lookup.lock().expect("lock") = Some(listing_unavailable.clone());
    *relay.receipts_lookup.lock().expect("lock") = Some(receipts_unavailable.clone());
    let key = Keys::generate().public_key();

    assert_eq!(relay.lookup_listing(key, "game").await, listing_unavailable);
    assert_eq!(
        relay.lookup_receipts(key, "coordinate").await,
        receipts_unavailable
    );
}
