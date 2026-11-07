use std::collections::{BTreeSet, HashMap};
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

    async fn lookup_event(&self, event_id: nostr_sdk::EventId) -> RelayLookup<Event> {
        self.authorizations
            .lock()
            .expect("lock authorizations")
            .iter()
            .find(|event| event.id == event_id)
            .cloned()
            .map(RelayLookup::Found)
            .unwrap_or(RelayLookup::ConfirmedAbsent {
                successful_relays: MIN_SUCCESSFUL_RELAYS,
            })
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
    authorization_root: nostr_sdk::EventId,
    file_hash: &str,
) -> Event {
    EventBuilder::new(Kind::Custom(30402), "")
        .tags(vec![
            tag(vec!["d", "game-1"]),
            tag(vec!["server", server_url]),
            tag(vec!["file_hash", file_hash]),
            tag(vec!["price", "1", "SATS"]),
            tag(vec![
                "fulfillment_authorization",
                &authorization_root.to_hex(),
                fulfillment_pubkey,
            ]),
            tag(vec!["fulfillment_authorization", "malformed", "entry"]),
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

/// Generated chunks exercise the real router without allocating an archive-sized buffer.
async fn upload_route_case(bytes: usize, limit: u64, corrupt: bool) {
    use axum::body::Bytes;
    use futures_util::{stream, StreamExt};
    let dir = tempfile::tempdir().expect("tempdir");
    let files_dir = dir.path().join("files");
    let server_url = "http://localhost:8080";
    let developer = Keys::generate();
    let relay = MockRelay::new(server_url);
    let storage = Storage::connect(dir.path().join("adp.db").to_str().expect("path"))
        .await
        .expect("storage");
    let config = Config {
        server_url: server_url.into(),
        bind_addr: "127.0.0.1:0".into(),
        db_path: String::new(),
        files_dir: files_dir.to_string_lossy().into(),
        max_upload_bytes: limit,
        relays: vec![],
        operator_nsec: developer.secret_key().to_secret_hex(),
        master_key: [9; 32],
        allow_insecure_lnurl: false,
        download_token_ttl_secs: 900,
        file_reverify_interval_secs: 3600,
    };
    let app = routes::router(AppState {
        config: Arc::new(config),
        storage: storage.clone(),
        relay: Arc::new(relay.clone()),
        keystore: Arc::new(KeyStore::new(storage.pool(), [9; 32])),
        operator_keys: developer.clone(),
    });
    let chunk = Bytes::from(vec![42; 64 * 1024]);
    let mut hash = Sha256::new();
    for offset in (0..bytes).step_by(chunk.len()) {
        hash.update(&chunk[..(bytes - offset).min(chunk.len())]);
    }
    let expected_hash = hex::encode(hash.finalize());
    let declared_hash = if corrupt {
        "00".repeat(32)
    } else {
        expected_hash.clone()
    };
    let listing = listing_event(
        &developer,
        server_url,
        &developer.public_key().to_hex(),
        nostr_sdk::EventId::all_zeros(),
        &declared_hash,
    );
    relay.add_listing(listing.clone(), "game-1");
    let prefix = Bytes::from(format!("--test\r\nContent-Disposition: form-data; name=\"listing_event\"\r\n\r\n{}\r\n--test\r\nContent-Disposition: form-data; name=\"file\"; filename=\"game.zip\"\r\n\r\n", serde_json::to_string(&listing).expect("json")));
    let chunks = stream::iter((0..bytes).step_by(chunk.len()).map(move |offset| {
        Ok::<_, std::io::Error>(chunk.slice(..(bytes - offset).min(chunk.len())))
    }));
    let body = stream::once(async { Ok::<_, std::io::Error>(prefix) })
        .chain(chunks)
        .chain(stream::once(async {
            Ok::<_, std::io::Error>(Bytes::from_static(b"\r\n--test--\r\n"))
        }));
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/upload")
                .header("content-type", "multipart/form-data; boundary=test")
                .header(
                    "authorization",
                    auth_header(&developer, "POST", &format!("{server_url}/upload")),
                )
                .body(Body::from_stream(body))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let json = response_json(response).await;
    let expected_status = if bytes as u64 > limit {
        StatusCode::PAYLOAD_TOO_LARGE
    } else if corrupt {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::OK
    };
    assert_eq!(status, expected_status, "{json}");
    let stored: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM files")
        .fetch_one(&storage.pool())
        .await
        .expect("count");
    let success = expected_status == StatusCode::OK;
    assert_eq!(stored, i64::from(success));
    assert_eq!(
        std::fs::read_dir(&files_dir).expect("directory").count(),
        usize::from(success)
    );
    if success {
        assert_eq!(
            std::fs::metadata(files_dir.join(expected_hash))
                .expect("file")
                .len(),
            bytes as u64
        );
    }
    let metadata = response_json(
        app.oneshot(
            Request::builder()
                .uri("/.well-known/adp")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("metadata"),
    )
    .await;
    assert_eq!(metadata["max_upload_bytes"], limit);
}

#[tokio::test]
async fn upload_router_streams_and_persists_exact_limit() {
    upload_route_case(3 * 1024 * 1024, 3 * 1024 * 1024, false).await;
}

#[tokio::test]
async fn upload_router_rejects_oversized_without_persistence() {
    upload_route_case(1025, 1024, false).await;
}

#[tokio::test]
async fn upload_router_hash_mismatch_cleans_staging() {
    upload_route_case(1024, 1024, true).await;
}

#[tokio::test]
#[ignore = "writes and hashes over 1 GiB; run explicitly for the historical upload regression"]
async fn upload_router_above_historical_one_gib_limit() {
    upload_route_case(1024 * 1024 * 1024 + 1, 1024 * 1024 * 1024 + 1, false).await;
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
        max_upload_bytes: 8 * 1024 * 1024 * 1024,
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
        operator_keys: operator.clone(),
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
    let attestation: Event = serde_json::from_value(provision_json["attestation"].clone())
        .expect("signed attestation evidence");
    attestation.verify().expect("attestation signature");
    assert_eq!(attestation.pubkey, operator.public_key());
    assert_eq!(
        provision_json["attestation_event_id"]
            .as_str()
            .expect("attestation event ID"),
        attestation.id.to_hex()
    );
    let repeated_provision = app
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
        .expect("repeated provision response");
    assert_eq!(repeated_provision.status(), StatusCode::OK);
    let repeated_json = response_json(repeated_provision).await;
    let repeated_attestation: Event = serde_json::from_value(repeated_json["attestation"].clone())
        .expect("reconstructed signed attestation evidence");
    repeated_attestation
        .verify()
        .expect("reconstructed attestation signature");
    assert_eq!(repeated_attestation.id, attestation.id);

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

    let coordinate = format!("30402:{}:game-1", developer.public_key().to_hex());
    storage
        .record_file(
            &file_hash,
            &coordinate,
            file_path.to_str().expect("utf8 path"),
        )
        .await
        .expect("record coordinate file");
    let authorization_time = Timestamp::now().as_secs();
    let authorization = build_authorization_root_event_builder(&AuthorizationTerms {
        authorization_id: "game-1-authorization".into(),
        coordinate: coordinate.clone(),
        operator_pubkey: operator.public_key(),
        fulfillment_pubkey: PublicKey::from_hex(&fulfillment_pubkey).expect("fulfillment pubkey"),
        capabilities: BTreeSet::from([adp_core::CAPABILITY_ISSUE_RECEIPT.into()]),
        valid_from: authorization_time,
    })
    .expect("authorization builder")
    .custom_created_at(Timestamp::from_secs(authorization_time))
    .sign_with_keys(&developer)
    .expect("authorization signature");
    let listing = listing_event(
        &developer,
        server_url,
        &fulfillment_pubkey,
        authorization.id,
        &file_hash,
    );
    relay.add_listing(listing.clone(), "game-1");
    relay.add_authorization(authorization);

    let preimage = [42u8; 32];
    let bolt11 = bolt11_for_preimage(&preimage);
    let incomplete_response = app
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
                .body(Body::from(
                    serde_json::json!({
                        "game_coordinate": coordinate.clone(),
                        "listing_event": listing.clone(),
                        "bolt11": bolt11.clone(),
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("incomplete proof response");
    assert_eq!(incomplete_response.status(), StatusCode::BAD_REQUEST);

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
    let stored_proofs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM payment_proofs")
        .fetch_one(&storage.pool())
        .await
        .expect("payment proof count");
    assert_eq!(stored_proofs, 1);
    let stored_key: String = sqlx::query_scalar("SELECT proof_key FROM payment_proofs")
        .fetch_one(&storage.pool())
        .await
        .expect("stored payment proof");
    assert_eq!(stored_key, hex::encode(preimage));
    let stored_order: String = sqlx::query_scalar("SELECT order_id FROM purchases")
        .fetch_one(&storage.pool())
        .await
        .expect("stored order");
    assert!(
        sqlx::query("INSERT INTO payment_proofs (proof_key, order_id) VALUES (?, ?)")
            .bind(&stored_key)
            .bind(&stored_order)
            .execute(&storage.pool())
            .await
            .is_err()
    );

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

    let historical_token_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/game/{coordinate}?token={token}"))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("historical token response");
    assert_eq!(historical_token_response.status(), StatusCode::OK);

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
