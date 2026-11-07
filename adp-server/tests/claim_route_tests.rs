use std::sync::{Arc, Mutex};

use adp_core::EXPERIMENTAL_ADP_CAMPAIGN_KIND;
use adp_server::config::Config;
use adp_server::keystore::KeyStore;
use adp_server::relay::{Relay, RelayLookup, RelayResult, MIN_SUCCESSFUL_RELAYS};
use adp_server::routes;
use adp_server::state::AppState;
use adp_server::storage::{MarkClaimPublishedResult, Storage};
use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use base64::Engine;
use http_body_util::BodyExt;
use nostr_sdk::{Event, EventBuilder, EventId, Keys, Kind, PublicKey, Tag, Timestamp};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::Barrier;
use tower::ServiceExt;

#[derive(Clone)]
struct ClaimRelay {
    server_url: String,
    listing: Event,
    root: Event,
    chain: Arc<Mutex<Vec<Event>>>,
    published: Arc<Mutex<Vec<Event>>>,
    publish_barrier: Option<Arc<Barrier>>,
}

#[async_trait]
impl Relay for ClaimRelay {
    fn server_url(&self) -> &str {
        &self.server_url
    }

    async fn publish(&self, event: &Event) -> RelayResult<()> {
        self.published
            .lock()
            .expect("published lock")
            .push(event.clone());
        if let Some(barrier) = &self.publish_barrier {
            barrier.wait().await;
        }
        Ok(())
    }

    async fn lookup_listing(&self, developer: PublicKey, d_tag: &str) -> RelayLookup<Event> {
        let expected_d = self
            .listing
            .tags
            .iter()
            .find_map(|tag| {
                let values = tag.clone().to_vec();
                (values.first().map(String::as_str) == Some("d"))
                    .then(|| values.get(1).cloned())
                    .flatten()
            })
            .unwrap_or_default();
        if self.listing.pubkey == developer && expected_d == d_tag {
            RelayLookup::Found(self.listing.clone())
        } else {
            RelayLookup::ConfirmedAbsent {
                successful_relays: MIN_SUCCESSFUL_RELAYS,
            }
        }
    }

    async fn lookup_event(&self, event_id: EventId) -> RelayLookup<Event> {
        if event_id == self.root.id {
            RelayLookup::Found(self.root.clone())
        } else {
            RelayLookup::ConfirmedAbsent {
                successful_relays: MIN_SUCCESSFUL_RELAYS,
            }
        }
    }

    async fn lookup_campaign_chain(
        &self,
        _developer: PublicKey,
        _campaign_d: &str,
    ) -> RelayLookup<Vec<Event>> {
        RelayLookup::Found(self.chain.lock().expect("chain lock").clone())
    }
}

fn tag(values: Vec<&str>) -> Tag {
    Tag::parse(values).expect("valid tag")
}

fn auth_header(keys: &Keys, url: &str) -> String {
    let event = EventBuilder::new(Kind::Custom(27235), "")
        .tags(vec![tag(vec!["u", url]), tag(vec!["method", "POST"])])
        .custom_created_at(Timestamp::now())
        .sign_with_keys(keys)
        .expect("sign auth");
    format!(
        "Nostr {}",
        base64::engine::general_purpose::STANDARD
            .encode(serde_json::to_vec(&event).expect("serialize auth"))
    )
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs()
}

fn listing(developer: &Keys, server_url: &str, d_tag: &str, hash: &str, now: u64) -> Event {
    EventBuilder::new(Kind::Custom(30402), "")
        .tags(vec![
            tag(vec!["d", d_tag]),
            tag(vec!["server", server_url]),
            tag(vec!["file_hash", hash]),
        ])
        .custom_created_at(Timestamp::from_secs(now - 30))
        .sign_with_keys(developer)
        .expect("sign listing")
}

fn campaign_root(developer: &Keys, coordinate: &str, now: u64) -> Event {
    EventBuilder::new(Kind::Custom(EXPERIMENTAL_ADP_CAMPAIGN_KIND), "")
        .tags(vec![
            tag(vec!["d", "campaign-1"]),
            tag(vec!["a", coordinate]),
            tag(vec!["mode", "claim"]),
            tag(vec!["starts", &(now - 10).to_string()]),
            tag(vec!["ends", &(now + 600).to_string()]),
            tag(vec!["status", "active"]),
        ])
        .custom_created_at(Timestamp::from_secs(now - 20))
        .sign_with_keys(developer)
        .expect("sign campaign")
}

fn cancellation(developer: &Keys, root: &Event, coordinate: &str, now: u64) -> Event {
    EventBuilder::new(Kind::Custom(EXPERIMENTAL_ADP_CAMPAIGN_KIND), "")
        .tags(vec![
            tag(vec!["d", "campaign-1"]),
            tag(vec!["a", coordinate]),
            tag(vec!["status", "cancelled"]),
            tag(vec!["e", &root.id.to_hex()]),
        ])
        .custom_created_at(Timestamp::from_secs(now - 5))
        .sign_with_keys(developer)
        .expect("sign cancellation")
}

async fn fixture(
    requested_d: &str,
    campaign_d: &str,
    cancelled: bool,
) -> (
    axum::Router,
    Keys,
    Keys,
    Event,
    Arc<Mutex<Vec<Event>>>,
    Arc<Mutex<Vec<Event>>>,
    Storage,
) {
    fixture_with_publish_barrier(requested_d, campaign_d, cancelled, None).await
}

async fn fixture_with_publish_barrier(
    requested_d: &str,
    campaign_d: &str,
    cancelled: bool,
    publish_barrier: Option<Arc<Barrier>>,
) -> (
    axum::Router,
    Keys,
    Keys,
    Event,
    Arc<Mutex<Vec<Event>>>,
    Arc<Mutex<Vec<Event>>>,
    Storage,
) {
    fixture_with_options(
        requested_d,
        campaign_d,
        cancelled,
        publish_barrier,
        true,
        true,
    )
    .await
}

async fn fixture_with_options(
    requested_d: &str,
    campaign_d: &str,
    cancelled: bool,
    publish_barrier: Option<Arc<Barrier>>,
    distributed: bool,
    developer_signer: bool,
) -> (
    axum::Router,
    Keys,
    Keys,
    Event,
    Arc<Mutex<Vec<Event>>>,
    Arc<Mutex<Vec<Event>>>,
    Storage,
) {
    let dir = tempfile::tempdir().expect("tempdir").keep();
    let db_path = dir.join("adp.db");
    let files_dir = dir.join("files");
    tokio::fs::create_dir_all(&files_dir)
        .await
        .expect("files dir");
    let developer = Keys::generate();
    let buyer = Keys::generate();
    let now = unix_now();
    let server_url = "http://localhost:8080";
    let bytes = b"game";
    let hash = hex::encode(Sha256::digest(bytes));
    let file_path = files_dir.join(&hash);
    tokio::fs::write(&file_path, bytes)
        .await
        .expect("write file");
    let listing_server = if distributed {
        server_url
    } else {
        "http://unauthorized.example"
    };
    let current_listing = listing(&developer, listing_server, requested_d, &hash, now);
    let campaign_coordinate = format!("30402:{}:{campaign_d}", developer.public_key());
    let root = campaign_root(&developer, &campaign_coordinate, now);
    let mut chain = vec![root.clone()];
    if cancelled {
        chain.push(cancellation(&developer, &root, &campaign_coordinate, now));
    }
    let published = Arc::new(Mutex::new(Vec::new()));
    let chain = Arc::new(Mutex::new(chain));
    let relay = ClaimRelay {
        server_url: server_url.into(),
        listing: current_listing,
        root: root.clone(),
        chain: chain.clone(),
        published: published.clone(),
        publish_barrier,
    };
    let storage = Storage::connect(db_path.to_str().expect("db path"))
        .await
        .expect("storage");
    storage
        .record_file(
            &hash,
            &format!("30402:{}:{requested_d}", developer.public_key()),
            file_path.to_str().expect("file path"),
        )
        .await
        .expect("record file");
    let operator = if developer_signer {
        developer.clone()
    } else {
        Keys::generate()
    };
    let config = Config {
        server_url: server_url.into(),
        bind_addr: "127.0.0.1:0".into(),
        db_path: db_path.to_string_lossy().into(),
        files_dir: files_dir.to_string_lossy().into(),
        max_upload_bytes: 8 * 1024 * 1024 * 1024,
        relays: vec!["ws://a".into(), "ws://b".into()],
        operator_nsec: operator.secret_key().to_secret_hex(),
        master_key: [7u8; 32],
        allow_insecure_lnurl: true,
        download_token_ttl_secs: 900,
        file_reverify_interval_secs: 3600,
    };
    let keystore = KeyStore::new(storage.pool(), config.master_key);
    let state = AppState {
        config: Arc::new(config),
        storage: storage.clone(),
        relay: Arc::new(relay),
        keystore: Arc::new(keystore),
        operator_keys: operator,
    };
    (
        routes::router(state),
        developer,
        buyer,
        root,
        chain,
        published,
        storage,
    )
}

async fn claim(
    app: axum::Router,
    buyer: &Keys,
    coordinate: &str,
    root: &Event,
) -> (StatusCode, Value) {
    let url = "http://localhost:8080/entitlement/claim";
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/entitlement/claim")
                .header("content-type", "application/json")
                .header("authorization", auth_header(buyer, url))
                .body(Body::from(
                    json!({
                        "game_coordinate": coordinate,
                        "campaign_event_id": root.id.to_hex(),
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let json = serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
    (status, json)
}

#[tokio::test]
async fn successful_claim_and_repeat_return_same_grant() {
    let (app, _, buyer, root, _, published, _) = fixture("game-1", "game-1", false).await;
    let actual_coordinate = root
        .tags
        .iter()
        .find_map(|tag| {
            let values = tag.clone().to_vec();
            (values.first().map(String::as_str) == Some("a"))
                .then(|| values.get(1).cloned())
                .flatten()
        })
        .expect("coordinate");
    let (first_status, first) = claim(app.clone(), &buyer, &actual_coordinate, &root).await;
    assert_eq!(first_status, StatusCode::OK);
    assert_eq!(first["already_claimed"], false);
    let (second_status, second) = claim(app, &buyer, &actual_coordinate, &root).await;
    assert_eq!(second_status, StatusCode::OK);
    assert_eq!(second["already_claimed"], true);
    assert_eq!(first["grant"]["id"], second["grant"]["id"]);
    assert_eq!(published.lock().expect("published").len(), 1);
}

#[tokio::test]
async fn repeat_after_campaign_cancellation_returns_persisted_grant() {
    let (app, developer, buyer, root, chain, published, _) =
        fixture("game-1", "game-1", false).await;
    let coordinate = root
        .tags
        .iter()
        .find_map(|tag| {
            let values = tag.clone().to_vec();
            (values.first().map(String::as_str) == Some("a")).then(|| values[1].clone())
        })
        .expect("coordinate");

    let (first_status, first) = claim(app.clone(), &buyer, &coordinate, &root).await;
    assert_eq!(first_status, StatusCode::OK);
    chain
        .lock()
        .expect("chain")
        .push(cancellation(&developer, &root, &coordinate, unix_now()));

    let (second_status, second) = claim(app, &buyer, &coordinate, &root).await;
    assert_eq!(second_status, StatusCode::OK);
    assert_eq!(first["grant"]["id"], second["grant"]["id"]);
    assert_ne!(first["download_token"], second["download_token"]);
    assert_eq!(second["already_claimed"], true);
    assert_eq!(published.lock().expect("published").len(), 1);
}

#[tokio::test]
async fn concurrent_duplicate_claims_return_same_published_grant() {
    let barrier = Arc::new(Barrier::new(2));
    let (app, _, buyer, root, _, published, storage) =
        fixture_with_publish_barrier("game-1", "game-1", false, Some(barrier)).await;
    let coordinate = root
        .tags
        .iter()
        .find_map(|tag| {
            let values = tag.clone().to_vec();
            (values.first().map(String::as_str) == Some("a")).then(|| values[1].clone())
        })
        .expect("coordinate");

    let (first, second) = tokio::join!(
        claim(app.clone(), &buyer, &coordinate, &root),
        claim(app, &buyer, &coordinate, &root),
    );

    assert_eq!(first.0, StatusCode::OK);
    assert_eq!(second.0, StatusCode::OK);
    assert_eq!(first.1["grant"]["id"], second.1["grant"]["id"]);
    {
        let published = published.lock().expect("published");
        // Publish-before-finalize can send twice, but both sends use the canonical stored event.
        assert_eq!(published.len(), 2);
        assert_eq!(published[0], published[1]);
    }
    let claim_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM entitlement_claims")
        .fetch_one(&storage.pool())
        .await
        .expect("claim count");
    assert_eq!(claim_count, 1);
    let stored = storage
        .find_entitlement_claim(&buyer.public_key().to_hex(), &coordinate, "campaign-1")
        .await
        .expect("stored claim")
        .expect("claim row");
    assert_eq!(stored.state, adp_server::storage::ClaimState::Published);
}

#[tokio::test]
async fn zero_row_publish_update_accepts_identical_published_claim() {
    let (app, _, buyer, root, _, _, storage) = fixture("game-1", "game-1", false).await;
    let coordinate = root
        .tags
        .iter()
        .find_map(|tag| {
            let values = tag.clone().to_vec();
            (values.first().map(String::as_str) == Some("a")).then(|| values[1].clone())
        })
        .expect("coordinate");
    let (status, _) = claim(app, &buyer, &coordinate, &root).await;
    assert_eq!(status, StatusCode::OK);
    let stored = storage
        .find_entitlement_claim(&buyer.public_key().to_hex(), &coordinate, "campaign-1")
        .await
        .expect("stored claim")
        .expect("claim row");

    let result = storage
        .mark_claim_published_if_identical(
            &stored.buyer_pubkey,
            &stored.game_coordinate,
            &stored.campaign_id,
            &stored.grant,
        )
        .await
        .expect("finalize claim");

    assert_eq!(result, MarkClaimPublishedResult::AlreadyPublishedIdentical);
}

#[tokio::test]
async fn cancelled_campaign_is_rejected() {
    let (app, _, buyer, root, _, _, _) = fixture("game-1", "game-1", true).await;
    let actual_coordinate = root
        .tags
        .iter()
        .find_map(|tag| {
            let values = tag.clone().to_vec();
            (values.first().map(String::as_str) == Some("a")).then(|| values[1].clone())
        })
        .expect("coordinate");
    let (status, _) = claim(app, &buyer, &actual_coordinate, &root).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn wrong_game_coordinate_is_rejected() {
    let (app, _, buyer, root, _, _, _) = fixture("game-2", "game-1", false).await;
    let requested = root
        .tags
        .iter()
        .find_map(|tag| {
            let values = tag.clone().to_vec();
            (values.first().map(String::as_str) == Some("a")).then(|| {
                let mut parts = values[1].splitn(3, ':');
                format!("{}:{}:game-2", parts.next().unwrap(), parts.next().unwrap())
            })
        })
        .expect("coordinate");
    let (status, _) = claim(app, &buyer, &requested, &root).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn claim_without_distribution_authority_returns_451() {
    let (app, _, buyer, root, _, _, _) =
        fixture_with_options("game-1", "game-1", false, None, false, true).await;
    let coordinate = format!("30402:{}:game-1", root.pubkey);
    let (status, _) = claim(app, &buyer, &coordinate, &root).await;
    assert_eq!(status.as_u16(), 451);
}

#[tokio::test]
async fn claim_without_held_authorized_signer_returns_500() {
    let (app, _, buyer, root, _, _, _) =
        fixture_with_options("game-1", "game-1", false, None, true, false).await;
    let coordinate = format!("30402:{}:game-1", root.pubkey);
    let (status, _) = claim(app, &buyer, &coordinate, &root).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
}
