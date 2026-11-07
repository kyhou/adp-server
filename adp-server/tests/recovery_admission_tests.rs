//! Gate 1's adversarial recovery admission tests. No wallet/network traffic.
use adp_core::{PaymentLeg, PaymentRole, QuoteState, RefundDestination, SignedQuote};
use adp_server::{
    config::Config,
    crypto::MasterCipher,
    keystore::KeyStore,
    relay::{Relay, RelayResult},
    routes,
    settlement_clock::SettlementClock,
    state::AppState,
    storage::Storage,
};
use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use base64::Engine;
use http_body_util::BodyExt;
use nostr_sdk::{Event, EventBuilder, Keys, Kind, Tag};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;
struct Quiet;
#[async_trait]
impl Relay for Quiet {
    fn server_url(&self) -> &str {
        "http://localhost:9099"
    }
    async fn publish(&self, _: &Event) -> RelayResult<()> {
        Ok(())
    }
}
async fn fixture(failed: bool) -> (tempfile::TempDir, Storage, axum::Router, Keys, Keys, String) {
    let dir = tempfile::tempdir().expect("tempdir");
    let storage = Storage::connect(dir.path().join("adp.db").to_str().expect("path"))
        .await
        .expect("storage");
    let buyer = Keys::generate();
    let operator = Keys::generate();
    let listing = EventBuilder::new(Kind::Custom(30402), "fixture")
        .tags([Tag::parse(["d", "game"]).expect("d")])
        .sign_with_keys(&operator)
        .expect("listing");
    let mut quote = SignedQuote::new(
        "q".into(),
        buyer.public_key().to_hex(),
        listing.id.to_hex(),
        format!("30402:{}:game", operator.public_key()),
        operator.public_key().to_hex(),
        1000,
        100,
        &RefundDestination {
            endpoint: "buyer@example.com".into(),
            min_sendable_msat: 1,
            max_sendable_msat: 1000,
        },
        vec![PaymentLeg {
            leg_id: "developer".into(),
            role: PaymentRole::Developer,
            recipient_pubkey: operator.public_key().to_hex(),
            destination: "dev@example.com".into(),
            amount_msat: 1000,
            accepted_terms_id: None,
        }],
    )
    .expect("quote");
    quote.state = QuoteState::Active;
    let quote_event = EventBuilder::new(
        Kind::Custom(1033),
        adp_core::encrypt_receipt_content(
            &operator,
            &buyer.public_key(),
            &serde_json::to_string(&quote).expect("JSON"),
        )
        .expect("encrypted"),
    )
    .sign_with_keys(&operator)
    .expect("signed quote");
    let encrypted = MasterCipher::new(&[7; 32])
        .expect("cipher")
        .encrypt(&serde_json::to_vec(&quote).expect("JSON"));
    sqlx::query("INSERT INTO purchase_quotes (quote_id,quote_event_id,quote_event_json,buyer_pubkey,listing_coordinate,listing_event_id,processor_pubkey,gross_msat,state,issued_at,payment_expires_at,confirmation_expires_at,failure_at,listing_event_json,quote_plaintext_encrypted) VALUES ('q',?,?,?,?,?,?,1000,?,100,1900,86500,?,?,?)")
        .bind(quote_event.id.to_hex()).bind(serde_json::to_string(&quote_event).expect("JSON")).bind(buyer.public_key().to_hex()).bind(&quote.listing_coordinate).bind(listing.id.to_hex()).bind(operator.public_key().to_hex())
        .bind(if failed{"failed"}else{"active"}).bind(if failed{Some(1900i64)}else{None}).bind(serde_json::to_string(&listing).expect("JSON")).bind(serde_json::to_vec(&encrypted).expect("JSON")).execute(&storage.pool()).await.expect("quote row");
    sqlx::query("INSERT INTO payment_legs (quote_id,leg_id,leg_order,role,recipient_pubkey,destination_encrypted,amount_msat) VALUES ('q','developer',0,'developer',?,X'01',1000)")
        .bind(operator.public_key().to_hex()).execute(&storage.pool()).await.expect("leg");
    let commitment = adp_core::canonical_commitment(&(
        adp_core::SETTLEMENT_VERSION,
        "q",
        quote_event.id.to_hex(),
        1900u64,
        Vec::<String>::new(),
    ))
    .expect("commitment");
    let plaintext = serde_json::to_string(&adp_core::RecoveryEvidence {
        version: adp_core::SETTLEMENT_VERSION,
        quote_id: "q".into(),
        quote_event_id: quote_event.id.to_hex(),
        buyer_pubkey: buyer.public_key().to_hex(),
        processor_pubkey: operator.public_key().to_hex(),
        failure_at: 1900,
        paid_leg_ids: vec![],
        evidence_commitment: commitment,
    })
    .expect("evidence");
    let config = Config {
        server_url: "http://localhost:9099".into(),
        bind_addr: "127.0.0.1:0".into(),
        db_path: "unused".into(),
        files_dir: dir.path().join("files").to_string_lossy().into(),
        max_upload_bytes: 1024,
        relays: vec![],
        operator_nsec: operator.secret_key().to_secret_hex(),
        master_key: [7; 32],
        allow_insecure_lnurl: true,
        download_token_ttl_secs: 900,
        file_reverify_interval_secs: 3600,
    };
    let app = routes::router_with_settlement_clock(
        AppState {
            config: Arc::new(config),
            storage: storage.clone(),
            keystore: Arc::new(KeyStore::new(storage.pool(), [7; 32])),
            relay: Arc::new(Quiet),
            operator_keys: operator.clone(),
        },
        SettlementClock::new(|| Some(6000)),
    );
    (dir, storage, app, buyer, operator, plaintext)
}
async fn submit(
    app: axum::Router,
    buyer: &Keys,
    event: Event,
    plaintext: String,
) -> (StatusCode, Value) {
    let path = "/settlement/q/recovery/submit";
    let auth = EventBuilder::new(Kind::Custom(27235), "")
        .tags([
            Tag::parse(["u", &format!("http://localhost:9099{path}")]).expect("u"),
            Tag::parse(["method", "POST"]).expect("method"),
        ])
        .sign_with_keys(buyer)
        .expect("auth");
    let response = app
        .oneshot(
            Request::post(path)
                .header(
                    "Authorization",
                    format!(
                        "Nostr {}",
                        base64::engine::general_purpose::STANDARD
                            .encode(serde_json::to_vec(&auth).expect("JSON"))
                    ),
                )
                .header("Content-Type", "application/json")
                .body(Body::from(
                    json!({"event":event,"plaintext":plaintext}).to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
#[tokio::test]
async fn fallback_rejects_signed_content_not_bound_to_supplied_evidence() {
    let (_dir, storage, app, buyer, operator, plaintext) = fixture(true).await;
    let wrong_encrypted = adp_core::encrypt_receipt_content(
        &buyer,
        &operator.public_key(),
        "different signed evidence",
    )
    .expect("encrypt");
    for content in ["not encrypted evidence".to_owned(), wrong_encrypted] {
        let event = EventBuilder::new(Kind::Custom(1034), content)
            .sign_with_keys(&buyer)
            .expect("event");
        assert_eq!(
            submit(app.clone(), &buyer, event, plaintext.clone())
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM recovery_claims")
        .fetch_one(&storage.pool())
        .await
        .expect("count");
    assert_eq!(count, 0);
}
#[tokio::test]
async fn valid_encrypted_fallback_is_idempotent_and_materializes_expired_deadlines() {
    let (_dir, storage, app, buyer, operator, plaintext) = fixture(false).await;
    let encrypted = adp_core::encrypt_receipt_content(&buyer, &operator.public_key(), &plaintext)
        .expect("encrypt");
    let event = EventBuilder::new(Kind::Custom(1034), encrypted)
        .sign_with_keys(&buyer)
        .expect("event");
    for _ in 0..2 {
        let (status, value) = submit(app.clone(), &buyer, event.clone(), plaintext.clone()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(value["event"]["id"], event.id.to_hex());
    }
    assert_eq!(
        storage
            .settlement_status("q")
            .await
            .expect("status")
            .expect("quote")
            .failure_at,
        Some(1900)
    );
}
