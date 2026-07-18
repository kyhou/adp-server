use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use adp_server::config::Config;
use adp_server::keystore::KeyStore;
use adp_server::relay::{Relay, RelayLookup, RelayResult, MIN_SUCCESSFUL_RELAYS};
use adp_server::routes;
use adp_server::state::AppState;
use adp_server::storage::Storage;
use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use base64::Engine;
use bitcoin::hashes::{sha256, Hash};
use bitcoin::secp256k1::{Secp256k1, SecretKey};
use http_body_util::BodyExt;
use lightning_invoice::{Currency, InvoiceBuilder, PaymentSecret};
use nostr_sdk::{Event, EventBuilder, Keys, Kind, PublicKey, Tag, Timestamp};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tower::ServiceExt;

#[derive(Clone)]
struct MockRelay {
    server_url: String,
    listings: Arc<Mutex<HashMap<(PublicKey, String), Event>>>,
    published: Arc<Mutex<Vec<Event>>>,
    listing_lookup: Arc<Mutex<Option<RelayLookup<Event>>>>,
    receipts_lookup: Arc<Mutex<Option<RelayLookup<Vec<Event>>>>>,
    authorizations: Arc<Mutex<Vec<Event>>>,
}

impl MockRelay {
    fn new(server_url: &str) -> Self {
        Self {
            server_url: server_url.to_string(),
            listings: Arc::new(Mutex::new(HashMap::new())),
            published: Arc::new(Mutex::new(Vec::new())),
            listing_lookup: Arc::new(Mutex::new(None)),
            receipts_lookup: Arc::new(Mutex::new(None)),
            authorizations: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn add_listing(&self, event: Event, d_tag: &str) {
        self.listings
            .lock()
            .expect("lock listings")
            .insert((event.pubkey, d_tag.to_string()), event);
    }

    fn add_authorization(&self, event: Event) {
        self.authorizations
            .lock()
            .expect("lock authorizations")
            .push(event);
    }
}

#[async_trait]
impl Relay for MockRelay {
    fn server_url(&self) -> &str {
        &self.server_url
    }

    async fn publish(&self, event: &Event) -> RelayResult<()> {
        self.published
            .lock()
            .expect("lock published")
            .push(event.clone());
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
            .expect("lock listings")
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
        if let Some(outcome) = self
            .listing_lookup
            .lock()
            .expect("lock listing lookup")
            .clone()
        {
            return outcome;
        }
        self.listings
            .lock()
            .expect("lock listings")
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
        if let Some(outcome) = self
            .receipts_lookup
            .lock()
            .expect("lock receipts lookup")
            .clone()
        {
            return outcome;
        }
        RelayLookup::ConfirmedAbsent {
            successful_relays: MIN_SUCCESSFUL_RELAYS,
        }
    }

    async fn lookup_authorizations(
        &self,
        developer: PublicKey,
        coordinate: &str,
    ) -> RelayLookup<Vec<Event>> {
        let events = self
            .authorizations
            .lock()
            .expect("lock authorizations")
            .iter()
            .filter(|event| {
                event.pubkey == developer
                    && event.tags.iter().any(|tag| {
                        let values = tag.clone().to_vec();
                        values.first().map(String::as_str) == Some("a")
                            && values.get(1).map(String::as_str) == Some(coordinate)
                    })
            })
            .cloned()
            .collect::<Vec<_>>();
        if events.is_empty() {
            RelayLookup::ConfirmedAbsent {
                successful_relays: MIN_SUCCESSFUL_RELAYS,
            }
        } else {
            RelayLookup::Found(events)
        }
    }

    async fn lookup_authorization_successors(
        &self,
        _developer: PublicKey,
        _event_id: nostr_sdk::EventId,
    ) -> RelayLookup<Vec<Event>> {
        RelayLookup::ConfirmedAbsent {
            successful_relays: MIN_SUCCESSFUL_RELAYS,
        }
    }
}

fn tag(values: Vec<&str>) -> Tag {
    Tag::parse(values).expect("valid tag")
}

fn auth_header(keys: &Keys, method: &str, url: &str) -> String {
    let event = EventBuilder::new(Kind::Custom(27235), "")
        .tags(vec![tag(vec!["u", url]), tag(vec!["method", method])])
        .custom_created_at(Timestamp::now())
        .sign_with_keys(keys)
        .expect("sign nip98");
    let json = serde_json::to_vec(&event).expect("serialize nip98");
    format!(
        "Nostr {}",
        base64::engine::general_purpose::STANDARD.encode(json)
    )
}

fn listing_event(
    developer: &Keys,
    server_url: &str,
    fulfillment_pubkey: &str,
    file_hash: &str,
) -> Event {
    EventBuilder::new(Kind::Custom(30402), "")
        .tags(vec![
            tag(vec!["d", "game-1"]),
            tag(vec!["server", server_url]),
            tag(vec!["file_hash", file_hash]),
            tag(vec!["price", "1", "SATS"]),
            tag(vec!["fulfillment_pubkey", fulfillment_pubkey, "1", ""]),
        ])
        .custom_created_at(Timestamp::from(1_700_000_000))
        .sign_with_keys(developer)
        .expect("sign listing")
}

fn bolt11_for_preimage(preimage: &[u8; 32]) -> String {
    let payment_hash = sha256::Hash::hash(preimage);
    let private_key = SecretKey::from_slice(&[1u8; 32]).expect("secret key");
    let secp = Secp256k1::new();
    InvoiceBuilder::new(Currency::Bitcoin)
        .amount_milli_satoshis(1_000)
        .description("ADP test".to_string())
        .payment_hash(payment_hash)
        .payment_secret(PaymentSecret([2u8; 32]))
        .duration_since_epoch(std::time::Duration::from_secs(1_700_000_000))
        .min_final_cltv_expiry_delta(144)
        .build_signed(|hash| secp.sign_ecdsa_recoverable(hash, &private_key))
        .expect("build invoice")
        .to_string()
}

async fn response_json(response: axum::response::Response) -> Value {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("json body")
}

#[tokio::test]
async fn provision_confirm_download_revoke_confirm_fails() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("adp.db");
    let files_dir = dir.path().join("files");
    tokio::fs::create_dir_all(&files_dir)
        .await
        .expect("files dir");
    let server_url = "http://localhost:8080";
    let operator = Keys::generate();
    let developer = Keys::generate();
    let buyer = Keys::generate();
    let relay = MockRelay::new(server_url);
    let storage = Storage::connect(db_path.to_str().expect("utf8 db path"))
        .await
        .expect("connect storage");
    let keystore = KeyStore::new(storage.pool(), [9u8; 32]);
    let config = Config {
        server_url: server_url.to_string(),
        bind_addr: "127.0.0.1:0".to_string(),
        db_path: db_path.to_string_lossy().to_string(),
        files_dir: files_dir.to_string_lossy().to_string(),
        relays: vec!["ws://relay-a".to_string(), "ws://relay-b".to_string()],
        operator_nsec: operator.secret_key().to_secret_hex(),
        master_key: [9u8; 32],
        allow_insecure_lnurl: false,
        download_token_ttl_secs: 900,
        file_reverify_interval_secs: 3600,
    };
    let state = AppState {
        config: Arc::new(config),
        storage: storage.clone(),
        relay: Arc::new(relay.clone()),
        keystore: Arc::new(keystore),
        operator_keys: operator,
    };
    let app = routes::router(state);

    let provision_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/provision")
                .header("content-type", "application/json")
                .header(
                    "authorization",
                    auth_header(&developer, "POST", &format!("{server_url}/provision")),
                )
                .body(Body::from(r#"{"scope":"game-1"}"#))
                .expect("request"),
        )
        .await
        .expect("provision response");
    assert_eq!(provision_response.status(), StatusCode::OK);
    let provision_json = response_json(provision_response).await;
    let fulfillment_pubkey = provision_json["fulfillment_pubkey"]
        .as_str()
        .expect("fulfillment pubkey")
        .to_string();

    let file_bytes = b"game archive bytes";
    let file_hash = hex::encode(Sha256::digest(file_bytes));
    let file_path = files_dir.join(&file_hash);
    tokio::fs::write(&file_path, file_bytes)
        .await
        .expect("write file");
    storage
        .record_file(
            &file_hash,
            "30402:placeholder:game-1",
            file_path.to_str().expect("utf8 path"),
        )
        .await
        .expect("record file");

    let listing = listing_event(&developer, server_url, &fulfillment_pubkey, &file_hash);
    let coordinate = format!("30402:{}:game-1", developer.public_key().to_hex());
    storage
        .record_file(
            &file_hash,
            &coordinate,
            file_path.to_str().expect("utf8 path"),
        )
        .await
        .expect("record coordinate file");
    relay.add_listing(listing.clone(), "game-1");
    let authorization = build_authorization_root_event_builder(
        &AuthorizationTerms {
            authorization_id: "game-1-authorization".into(),
            coordinate: coordinate.clone(),
            fulfillment_pubkey: PublicKey::from_hex(&fulfillment_pubkey)
                .expect("fulfillment pubkey"),
            valid_from: 1,
            valid_until: None,
        },
        "authorization-root",
    )
    .expect("authorization builder")
    .sign_with_keys(&developer)
    .expect("authorization signature");
    relay.add_authorization(authorization);

    let preimage = [42u8; 32];
    let bolt11 = bolt11_for_preimage(&preimage);
    let confirm_body = serde_json::json!({
        "game_coordinate": coordinate,
        "listing_event": listing,
        "bolt11": bolt11,
        "preimage": hex::encode(preimage),
    });
    let confirm_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/purchase/confirm")
                .header("content-type", "application/json")
                .header(
                    "authorization",
                    auth_header(&buyer, "POST", &format!("{server_url}/purchase/confirm")),
                )
                .body(Body::from(confirm_body.to_string()))
                .expect("request"),
        )
        .await
        .expect("confirm response");
    assert_eq!(confirm_response.status(), StatusCode::OK);
    let confirm_json = response_json(confirm_response).await;
    assert_eq!(
        confirm_json["receipt"]["pubkey"].as_str(),
        Some(fulfillment_pubkey.as_str())
    );
    let token = confirm_json["download_token"]
        .as_str()
        .expect("download token");

    let game_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/game/{coordinate}?token={token}"))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("game response");
    let game_status = game_response.status();
    let game_body = game_response
        .into_body()
        .collect()
        .await
        .expect("collect game body")
        .to_bytes();
    assert_eq!(
        game_status,
        StatusCode::OK,
        "game response body: {}",
        String::from_utf8_lossy(&game_body)
    );
    assert_eq!(game_body.as_ref(), file_bytes);

    let revoke_body = serde_json::json!({ "fulfillment_pubkey": fulfillment_pubkey });
    let revoke_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/provision/revoke")
                .header("content-type", "application/json")
                .header(
                    "authorization",
                    auth_header(
                        &developer,
                        "POST",
                        &format!("{server_url}/provision/revoke"),
                    ),
                )
                .body(Body::from(revoke_body.to_string()))
                .expect("request"),
        )
        .await
        .expect("revoke response");
    assert_eq!(revoke_response.status(), StatusCode::OK);

    let second_confirm_body = serde_json::json!({
        "game_coordinate": coordinate,
        "listing_event": listing,
        "bolt11": bolt11_for_preimage(&[43u8; 32]),
        "preimage": hex::encode([43u8; 32]),
    });
    let second_confirm_response = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/purchase/confirm")
                .header("content-type", "application/json")
                .header(
                    "authorization",
                    auth_header(&buyer, "POST", &format!("{server_url}/purchase/confirm")),
                )
                .body(Body::from(second_confirm_body.to_string()))
                .expect("request"),
        )
        .await
        .expect("second confirm response");
    assert_eq!(
        second_confirm_response.status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
}

#[tokio::test]
async fn mock_relay_supports_configurable_unavailable_lookups() {
    let relay = MockRelay::new("http://localhost:8080");
    let listing_unavailable = RelayLookup::Unavailable {
        successful_relays: 1,
        failed_relays: 1,
    };
    let receipts_unavailable = RelayLookup::Unavailable {
        successful_relays: 1,
        failed_relays: 1,
    };
    *relay.listing_lookup.lock().expect("lock listing lookup") = Some(listing_unavailable.clone());
    *relay.receipts_lookup.lock().expect("lock receipts lookup") =
        Some(receipts_unavailable.clone());
    let key = Keys::generate().public_key();

    assert_eq!(relay.lookup_listing(key, "game").await, listing_unavailable);
    assert_eq!(
        relay.lookup_receipts(key, "coordinate").await,
        receipts_unavailable
    );
}
use adp_core::{build_authorization_root_event_builder, AuthorizationTerms};
