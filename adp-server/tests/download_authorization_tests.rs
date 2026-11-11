use std::sync::Arc;

use adp_core::{
    build_entitlement_event_builder, BuildEntitlementParams, GrantStatus,
    EXPERIMENTAL_ADP_CAMPAIGN_KIND,
};
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
use nostr_sdk::{Event, EventBuilder, EventId, Keys, Kind, PublicKey, Tag, Timestamp};
use sha2::{Digest, Sha256};
use tower::ServiceExt;

include!("support/gate7_portable_receipts.rs");

#[derive(Clone)]
struct DownloadRelay {
    server_url: String,
    listing: Event,
    immutable: Vec<Event>,
    campaign: Vec<Event>,
    grants: RelayLookup<Vec<Event>>,
    receipts: RelayLookup<Vec<Event>>,
}

#[async_trait]
impl Relay for DownloadRelay {
    fn server_url(&self) -> &str {
        &self.server_url
    }

    async fn publish(&self, _event: &Event) -> RelayResult<()> {
        Ok(())
    }

    async fn lookup_listing(&self, developer: PublicKey, d_tag: &str) -> RelayLookup<Event> {
        if self.listing.pubkey == developer && d_tag == "game-1" {
            RelayLookup::Found(self.listing.clone())
        } else {
            absent()
        }
    }

    async fn lookup_event(&self, id: EventId) -> RelayLookup<Event> {
        self.immutable
            .iter()
            .find(|event| event.id == id)
            .cloned()
            .map(RelayLookup::Found)
            .unwrap_or_else(absent)
    }

    async fn lookup_campaign_chain(
        &self,
        _developer: PublicKey,
        _campaign_d: &str,
    ) -> RelayLookup<Vec<Event>> {
        RelayLookup::Found(self.campaign.clone())
    }

    async fn lookup_entitlements(
        &self,
        _buyer: PublicKey,
        _coordinate: &str,
    ) -> RelayLookup<Vec<Event>> {
        self.grants.clone()
    }

    async fn lookup_receipts(
        &self,
        _buyer: PublicKey,
        _coordinate: &str,
    ) -> RelayLookup<Vec<Event>> {
        self.receipts.clone()
    }

    async fn lookup_authorization_successors(
        &self,
        developer: PublicKey,
        id: EventId,
    ) -> RelayLookup<Vec<Event>> {
        let events=self.immutable.iter().filter(|event|event.pubkey==developer && event.tags.iter().any(|tag|matches!(tag.as_slice(),[name,value] if name=="e" && value==&id.to_hex()))).cloned().collect::<Vec<_>>();
        if events.is_empty() {
            absent()
        } else {
            RelayLookup::Found(events)
        }
    }
}

fn absent<T>() -> RelayLookup<T> {
    RelayLookup::ConfirmedAbsent {
        successful_relays: MIN_SUCCESSFUL_RELAYS,
    }
}

fn unavailable<T>() -> RelayLookup<T> {
    RelayLookup::Unavailable {
        successful_relays: 1,
        failed_relays: 1,
    }
}

fn tag(values: Vec<&str>) -> Tag {
    Tag::parse(values).expect("tag")
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs()
}

fn auth(keys: &Keys, coordinate: &str) -> String {
    let url = format!("http://localhost:8080/game/{coordinate}");
    let event = EventBuilder::new(Kind::Custom(27235), "")
        .tags(vec![tag(vec!["u", &url]), tag(vec!["method", "GET"])])
        .custom_created_at(Timestamp::now())
        .sign_with_keys(keys)
        .expect("auth");
    format!(
        "Nostr {}",
        base64::engine::general_purpose::STANDARD
            .encode(serde_json::to_vec(&event).expect("serialize"))
    )
}

fn listing(developer: &Keys, hash: &str, acquisition: Option<Vec<&str>>, zero: bool) -> Event {
    let mut tags = vec![
        tag(vec!["d", "game-1"]),
        tag(vec!["server", "http://localhost:8080"]),
        tag(vec!["file_hash", hash]),
        tag(vec!["price", if zero { "0" } else { "100" }, "SATS"]),
    ];
    if let Some(values) = acquisition {
        tags.push(tag(values));
    }
    EventBuilder::new(Kind::Custom(30402), "")
        .tags(tags)
        .custom_created_at(Timestamp::from_secs(now() - 30))
        .sign_with_keys(developer)
        .expect("listing")
}

fn campaign(developer: &Keys, coordinate: &str, issued_at: u64) -> Event {
    EventBuilder::new(Kind::Custom(EXPERIMENTAL_ADP_CAMPAIGN_KIND), "")
        .tags(vec![
            tag(vec!["d", "campaign-1"]),
            tag(vec!["a", coordinate]),
            tag(vec!["mode", "claim"]),
            tag(vec!["starts", &(issued_at - 10).to_string()]),
            tag(vec!["ends", &(issued_at + 10).to_string()]),
            tag(vec!["status", "active"]),
        ])
        .custom_created_at(Timestamp::from_secs(issued_at - 20))
        .sign_with_keys(developer)
        .expect("campaign")
}

fn grant(
    developer: &Keys,
    buyer: PublicKey,
    coordinate: &str,
    campaign: EventId,
    issued_at: u64,
) -> Event {
    build_entitlement_event_builder(&BuildEntitlementParams {
        grant_id: "grant-1".into(),
        recipient: buyer,
        coordinate: coordinate.into(),
        source_event: Some(campaign),
        authorization: None,
        reason: Some("promotional-claim".into()),
        status: GrantStatus::Granted,
        predecessor: None,
    })
    .expect("grant builder")
    .custom_created_at(Timestamp::from_secs(issued_at))
    .sign_with_keys(developer)
    .expect("grant")
}

fn receipt(developer: &Keys, buyer: PublicKey, coordinate: &str) -> Event {
    EventBuilder::new(Kind::Custom(1020), "")
        .tags(vec![
            tag(vec!["order", "order-1"]),
            tag(vec!["p", &buyer.to_hex()]),
            tag(vec!["a", coordinate]),
            tag(vec!["status", "paid"]),
            tag(vec!["payment_hash", &"11".repeat(32)]),
            tag(vec!["amount_msat", "1000"]),
            tag(vec!["settled_at", &now().to_string()]),
            tag(vec!["proof", "bolt11-preimage"]),
        ])
        .sign_with_keys(developer)
        .expect("receipt")
}

async fn app(
    developer: &Keys,
    listing: Event,
    immutable: Vec<Event>,
    campaign: Vec<Event>,
    grants: RelayLookup<Vec<Event>>,
    receipts: RelayLookup<Vec<Event>>,
) -> axum::Router {
    app_at(
        "http://localhost:8080",
        developer,
        listing,
        immutable,
        campaign,
        grants,
        receipts,
    )
    .await
}

async fn app_at(
    server_url: &str,
    developer: &Keys,
    listing: Event,
    immutable: Vec<Event>,
    campaign: Vec<Event>,
    grants: RelayLookup<Vec<Event>>,
    receipts: RelayLookup<Vec<Event>>,
) -> axum::Router {
    let dir = tempfile::tempdir().expect("tempdir").keep();
    let db = dir.join("adp.db");
    let file = dir.join("game.bin");
    tokio::fs::write(&file, b"game").await.expect("file");
    let hash = hex::encode(Sha256::digest(b"game"));
    let coordinate = format!("30402:{}:game-1", developer.public_key());
    let storage = Storage::connect(db.to_str().expect("db"))
        .await
        .expect("storage");
    storage
        .record_file(&hash, &coordinate, file.to_str().expect("path"))
        .await
        .expect("record");
    let config = Config {
        server_url: server_url.into(),
        bind_addr: "127.0.0.1:0".into(),
        db_path: db.to_string_lossy().into(),
        files_dir: dir.to_string_lossy().into(),
        max_upload_bytes: 8 * 1024 * 1024 * 1024,
        relays: vec!["ws://a".into(), "ws://b".into()],
        operator_nsec: developer.secret_key().to_secret_hex(),
        master_key: [8; 32],
        allow_insecure_lnurl: true,
        download_token_ttl_secs: 900,
        file_reverify_interval_secs: 3600,
    };
    let keystore = KeyStore::new(storage.pool(), config.master_key);
    routes::router(AppState {
        config: Arc::new(config),
        storage,
        relay: Arc::new(DownloadRelay {
            server_url: server_url.into(),
            listing,
            immutable,
            campaign,
            grants,
            receipts,
        }),
        keystore: Arc::new(keystore),
        operator_keys: developer.clone(),
    })
}

async fn download(app: axum::Router, buyer: &Keys, coordinate: &str) -> StatusCode {
    app.oneshot(
        Request::builder()
            .method(Method::GET)
            .uri(format!("/game/{coordinate}"))
            .header("authorization", auth(buyer, coordinate))
            .body(Body::empty())
            .expect("request"),
    )
    .await
    .expect("response")
    .status()
}

#[tokio::test]
async fn valid_grant_permits_download_after_listing_becomes_paid() {
    let developer = Keys::generate();
    let buyer = Keys::generate();
    let coordinate = format!("30402:{}:game-1", developer.public_key());
    let issued = now() - 60;
    let campaign = campaign(&developer, &coordinate, issued);
    let grant = grant(
        &developer,
        buyer.public_key(),
        &coordinate,
        campaign.id,
        issued,
    );
    let hash = hex::encode(Sha256::digest(b"game"));
    let app = app(
        &developer,
        listing(&developer, &hash, None, false),
        vec![campaign.clone()],
        vec![campaign],
        RelayLookup::Found(vec![grant]),
        absent(),
    )
    .await;
    assert_eq!(download(app, &buyer, &coordinate).await, StatusCode::OK);
}

#[tokio::test]
async fn zero_price_without_public_policy_is_denied() {
    let developer = Keys::generate();
    let buyer = Keys::generate();
    let coordinate = format!("30402:{}:game-1", developer.public_key());
    let hash = hex::encode(Sha256::digest(b"game"));
    let app = app(
        &developer,
        listing(&developer, &hash, None, true),
        vec![],
        vec![],
        absent(),
        absent(),
    )
    .await;
    assert_eq!(
        download(app, &buyer, &coordinate).await,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn explicit_public_policy_allows_when_credentials_are_unavailable() {
    let developer = Keys::generate();
    let buyer = Keys::generate();
    let coordinate = format!("30402:{}:game-1", developer.public_key());
    let hash = hex::encode(Sha256::digest(b"game"));
    let app = app(
        &developer,
        listing(
            &developer,
            &hash,
            Some(vec!["acquisition", "public"]),
            false,
        ),
        vec![],
        vec![],
        unavailable(),
        unavailable(),
    )
    .await;
    assert_eq!(download(app, &buyer, &coordinate).await, StatusCode::OK);
}

#[tokio::test]
async fn expired_timed_access_is_denied() {
    let developer = Keys::generate();
    let buyer = Keys::generate();
    let coordinate = format!("30402:{}:game-1", developer.public_key());
    let hash = hex::encode(Sha256::digest(b"game"));
    let current = now();
    let starts = (current - 100).to_string();
    let ends = (current - 10).to_string();
    let app = app(
        &developer,
        listing(
            &developer,
            &hash,
            Some(vec!["acquisition", "timed-access", &starts, &ends]),
            false,
        ),
        vec![],
        vec![],
        absent(),
        absent(),
    )
    .await;
    assert_eq!(
        download(app, &buyer, &coordinate).await,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn valid_developer_receipt_still_allows_download() {
    let developer = Keys::generate();
    let buyer = Keys::generate();
    let coordinate = format!("30402:{}:game-1", developer.public_key());
    let hash = hex::encode(Sha256::digest(b"game"));
    let receipt = receipt(&developer, buyer.public_key(), &coordinate);
    let app = app(
        &developer,
        listing(&developer, &hash, None, false),
        vec![],
        vec![],
        absent(),
        RelayLookup::Found(vec![receipt]),
    )
    .await;
    assert_eq!(download(app, &buyer, &coordinate).await, StatusCode::OK);
}

#[tokio::test]
async fn download_nip98_binds_raw_target_origin_and_actual_method() {
    let developer = Keys::generate();
    let buyer = Keys::generate();
    let coordinate = format!("30402:{}:game-1", developer.public_key());
    let raw = format!("/game/{}", coordinate.replace(':', "%3A"));
    let decoded = format!("/game/{coordinate}");
    let query = format!("{raw}?x=%2f&y=+&x=last/");
    let hash = hex::encode(Sha256::digest(b"game"));
    let app = app(
        &developer,
        listing(
            &developer,
            &hash,
            Some(vec!["acquisition", "public"]),
            false,
        ),
        vec![],
        vec![],
        absent(),
        absent(),
    )
    .await;
    let origin = "http://localhost:8080";
    let url = format!("{origin}{raw}");
    // target, signed URL, request method, signed method, optional second u, status
    let cases = vec![
        (raw.clone(), url.clone(), "GET", "GET", None, StatusCode::OK),
        (
            decoded.clone(),
            format!("{origin}{decoded}"),
            "GET",
            "GET",
            None,
            StatusCode::OK,
        ),
        (
            raw.clone(),
            format!("{origin}{decoded}"),
            "GET",
            "GET",
            None,
            StatusCode::UNAUTHORIZED,
        ),
        (
            decoded.clone(),
            url.clone(),
            "GET",
            "GET",
            None,
            StatusCode::UNAUTHORIZED,
        ),
        (
            raw.clone(),
            format!("{origin}{}", raw.replace("%3A", "%3a")),
            "GET",
            "GET",
            None,
            StatusCode::UNAUTHORIZED,
        ),
        (
            raw.clone(),
            format!("{url}/"),
            "GET",
            "GET",
            None,
            StatusCode::UNAUTHORIZED,
        ),
        (
            query.clone(),
            format!("{origin}{query}"),
            "GET",
            "GET",
            None,
            StatusCode::OK,
        ),
        (
            query.clone(),
            url.clone(),
            "GET",
            "GET",
            None,
            StatusCode::UNAUTHORIZED,
        ),
        (
            query.clone(),
            format!("{origin}{}", query.replace("%2f", "/")),
            "GET",
            "GET",
            None,
            StatusCode::UNAUTHORIZED,
        ),
        (
            query.clone(),
            format!("{origin}{}", query.replace("%2f", "%2F")),
            "GET",
            "GET",
            None,
            StatusCode::UNAUTHORIZED,
        ),
        (
            query.clone(),
            format!("{origin}{}", query.replace("y=+", "y=%20")),
            "GET",
            "GET",
            None,
            StatusCode::UNAUTHORIZED,
        ),
        (
            query.clone(),
            format!("{url}?y=+&x=%2f&x=last/"),
            "GET",
            "GET",
            None,
            StatusCode::UNAUTHORIZED,
        ),
        (
            query.clone(),
            format!("{origin}{}", query.trim_end_matches('/')),
            "GET",
            "GET",
            None,
            StatusCode::UNAUTHORIZED,
        ),
        (
            raw.clone(),
            format!("https://localhost:8080{raw}"),
            "GET",
            "GET",
            None,
            StatusCode::UNAUTHORIZED,
        ),
        (
            raw.clone(),
            format!("http://evil.example:8080{raw}"),
            "GET",
            "GET",
            None,
            StatusCode::UNAUTHORIZED,
        ),
        (
            raw.clone(),
            format!("http://localhost:9090{raw}"),
            "GET",
            "GET",
            None,
            StatusCode::UNAUTHORIZED,
        ),
        (
            raw.clone(),
            url.clone(),
            "GET",
            "POST",
            None,
            StatusCode::UNAUTHORIZED,
        ),
        (
            raw.clone(),
            url.clone(),
            "HEAD",
            "GET",
            None,
            StatusCode::UNAUTHORIZED,
        ),
        (
            raw.clone(),
            url.clone(),
            "HEAD",
            "HEAD",
            None,
            StatusCode::OK,
        ),
        (
            raw.clone(),
            url.clone(),
            "POST",
            "POST",
            None,
            StatusCode::METHOD_NOT_ALLOWED,
        ),
        (
            raw.clone(),
            format!("{url}/"),
            "GET",
            "GET",
            Some(url.clone()),
            StatusCode::UNAUTHORIZED,
        ),
        (
            raw.clone(),
            url.clone(),
            "GET",
            "GET",
            Some(format!("{url}/")),
            StatusCode::OK,
        ),
    ];
    for (target, signed_url, method, signed_method, second_url, expected) in cases {
        let mut tags = vec![
            tag(vec!["u", &signed_url]),
            tag(vec!["method", signed_method]),
        ];
        if let Some(second) = &second_url {
            tags.push(tag(vec!["u", second]));
        }
        let event = EventBuilder::new(Kind::Custom(27235), "")
            .tags(tags)
            .sign_with_keys(&buyer)
            .expect("auth");
        let auth = format!(
            "Nostr {}",
            base64::engine::general_purpose::STANDARD
                .encode(serde_json::to_vec(&event).expect("serialize"))
        );
        // Even headers matching a forged origin cannot authorize it; valid tokens
        // also work with headers that disagree with the configured origin.
        let header_url: axum::http::Uri = if expected == StatusCode::OK {
            "https://evil.example:9090".parse().expect("forged origin")
        } else {
            signed_url.parse().expect("signed url")
        };
        let host = header_url.authority().expect("authority").as_str();
        let scheme = header_url.scheme_str().expect("scheme");
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(&target)
                    .header("authorization", auth)
                    .header("host", host)
                    .header("forwarded", format!("host={host};proto={scheme}"))
                    .header("x-forwarded-host", host)
                    .header("x-forwarded-proto", scheme)
                    .header("x-forwarded-port", header_url.port_u16().expect("port"))
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(
            response.status(),
            expected,
            "{method} {target}, signed {signed_method} {signed_url}"
        );
        if expected == StatusCode::OK {
            assert_eq!(response.headers()["etag"], format!("\"{hash}\""));
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body");
            assert_eq!(
                body.as_ref(),
                if method == "HEAD" {
                    b"".as_slice()
                } else {
                    b"game".as_slice()
                }
            );
        }
    }

    for (kind, timestamp, tamper) in [
        (1, Timestamp::now(), false),
        (27235, Timestamp::from_secs(now() - 3600), false),
        (27235, Timestamp::now(), true),
    ] {
        let mut event = EventBuilder::new(Kind::Custom(kind), "")
            .tags(vec![tag(vec!["u", &url]), tag(vec!["method", "GET"])])
            .custom_created_at(timestamp)
            .sign_with_keys(&buyer)
            .expect("auth");
        if tamper {
            event.content = "tampered".into();
        }
        let auth = format!(
            "Nostr {}",
            base64::engine::general_purpose::STANDARD
                .encode(serde_json::to_vec(&event).expect("serialize"))
        );
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(&raw)
                    .header("authorization", auth)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}
