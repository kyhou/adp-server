//! Persisted aggregate receipt recovery through the real authenticated route.
//! Synthetic identities and a private temporary DB; no wallet/network access.
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use adp_core::{
    build_aggregate_receipt_event_builder, encrypt_receipt_content, AdpListing, AggregateProofLeg,
    BuildAggregateReceiptParams, PaymentLeg, PaymentRole, QuoteState, RefundDestination,
    SignedQuote,
};
use adp_server::{
    config::Config,
    crypto::MasterCipher,
    keystore::KeyStore,
    relay::{Relay, RelayResult},
    routes,
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
use tower::ServiceExt;

struct RetryRelay(AtomicBool);

#[async_trait]
impl Relay for RetryRelay {
    fn server_url(&self) -> &str {
        "http://localhost:9099"
    }
    async fn publish(&self, _: &Event) -> RelayResult<()> {
        if self.0.load(Ordering::SeqCst) {
            Err(std::io::Error::other("injected publication failure").into())
        } else {
            Ok(())
        }
    }
}

fn auth(keys: &Keys) -> String {
    let event = EventBuilder::new(Kind::Custom(27235), "")
        .tags([
            Tag::parse(["u", "http://localhost:9099/settlement/retry/proof"]).expect("u"),
            Tag::parse(["method", "POST"]).expect("method"),
        ])
        .sign_with_keys(keys)
        .expect("sign auth");
    format!(
        "Nostr {}",
        base64::engine::general_purpose::STANDARD
            .encode(serde_json::to_vec(&event).expect("auth JSON"))
    )
}

fn app(storage: Storage, operator: &Keys, relay: Arc<RetryRelay>) -> axum::Router {
    let config = Config {
        server_url: "http://localhost:9099".into(),
        bind_addr: "127.0.0.1:0".into(),
        db_path: "unused".into(),
        files_dir: "unused".into(),
        max_upload_bytes: 1024,
        relays: vec![],
        operator_nsec: operator.secret_key().to_secret_hex(),
        master_key: [7; 32],
        allow_insecure_lnurl: false,
        download_token_ttl_secs: 900,
        file_reverify_interval_secs: 3600,
    };
    routes::router(AppState {
        config: Arc::new(config),
        keystore: Arc::new(KeyStore::new(storage.pool(), [7; 32])),
        storage,
        relay,
        operator_keys: operator.clone(),
    })
}

async fn retry(app: axum::Router, buyer: &Keys) -> (StatusCode, Value) {
    let response = app.oneshot(Request::post("/settlement/retry/proof")
        .header("Authorization", auth(buyer)).header("Content-Type", "application/json")
        .body(Body::from(json!({"attempt_id":"already-consumed", "preimage":"not-needed-for-republication"}).to_string()))
        .expect("request")).await.expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (status, serde_json::from_slice(&bytes).expect("JSON"))
}

#[tokio::test]
async fn persisted_receipt_retry_after_restart_recovers_download_token() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("retry.db");
    let storage = Storage::connect(path.to_str().expect("path"))
        .await
        .expect("storage");
    let buyer = Keys::generate();
    let stranger = Keys::generate();
    let operator = Keys::generate();
    let listing = EventBuilder::new(Kind::Custom(30402), "snapshot")
        .tags([Tag::parse(["d", "game"]).expect("identifier")])
        .sign_with_keys(&operator)
        .expect("listing");
    let quote_event = EventBuilder::new(Kind::Custom(1033), "encrypted quote")
        .sign_with_keys(&operator)
        .expect("quote event");
    let mut quote = SignedQuote::new(
        "retry".into(),
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
    let parsed_listing = AdpListing::from_event(listing.clone()).expect("listing parsed");
    let proof_legs = vec![AggregateProofLeg {
        leg_id: "developer".into(),
        role: PaymentRole::Developer,
        recipient_pubkey: operator.public_key().to_hex(),
        amount_msat: 1000,
        payment_hash: "01".repeat(32),
        proof_type: "bolt11-preimage".into(),
        proof_reference: "01".repeat(32),
        accepted_terms_id: None,
    }];
    let params = BuildAggregateReceiptParams {
        listing: &parsed_listing,
        buyer_pubkey: buyer.public_key(),
        authorization: None,
        quote_event_id: quote_event.id,
        processor_pubkey: operator.public_key(),
        processor_url: Some("http://localhost:9099"),
        gross_msat: 1000,
        settled_at: 200,
        legs: &proof_legs,
    };
    let encrypted_content =
        encrypt_receipt_content(&operator, &buyer.public_key(), "persisted-receipt")
            .expect("encrypt receipt");
    let receipt = build_aggregate_receipt_event_builder("retry", &params, encrypted_content)
        .expect("receipt builder")
        .sign_with_keys(&operator)
        .expect("receipt");
    adp_core::parse_receipt_event(&receipt).expect("valid aggregate receipt");
    let encrypted = MasterCipher::new(&[7; 32])
        .expect("cipher")
        .encrypt(&serde_json::to_vec(&quote).expect("quote JSON"));
    // Model the durable checkpoint after receipt commit and before successful
    // relay publication/token issuance. Signed snapshots remain byte-identical.
    sqlx::query("INSERT INTO purchase_quotes (quote_id, quote_event_id, quote_event_json, buyer_pubkey,
        listing_coordinate, listing_event_id, processor_pubkey, gross_msat, state, issued_at,
        payment_expires_at, confirmation_expires_at, listing_event_json, quote_plaintext_encrypted,
        receipt_event_id, receipt_event_json) VALUES ('retry', ?, ?, ?, ?, ?, ?, 1000, 'settled', 100, 1900, 86500, ?, ?, ?, ?)")
        .bind(quote_event.id.to_hex()).bind(serde_json::to_string(&quote_event).expect("JSON"))
        .bind(buyer.public_key().to_hex()).bind(&quote.listing_coordinate).bind(listing.id.to_hex())
        .bind(operator.public_key().to_hex()).bind(serde_json::to_string(&listing).expect("JSON"))
        .bind(serde_json::to_vec(&encrypted).expect("encrypted JSON"))
        .bind(receipt.id.to_hex()).bind(serde_json::to_string(&receipt).expect("receipt JSON"))
        .execute(&storage.pool()).await.expect("checkpoint");
    let relay = Arc::new(RetryRelay(AtomicBool::new(true)));
    let (status, _) = retry(app(storage.clone(), &operator, relay.clone()), &buyer).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    storage.pool().close().await;
    let storage = Storage::connect(path.to_str().expect("path"))
        .await
        .expect("restart storage");
    relay.0.store(false, Ordering::SeqCst);
    let (status, _) = retry(app(storage.clone(), &operator, relay.clone()), &stranger).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    for _ in 0..2 {
        let (status, response) =
            retry(app(storage.clone(), &operator, relay.clone()), &buyer).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            response["receipt"],
            serde_json::to_value(&receipt).expect("receipt")
        );
        let token = response["download_token"]
            .as_str()
            .expect("retry must return download token");
        assert_eq!(
            storage
                .resolve_download_token(token, &quote.listing_coordinate)
                .await
                .expect("resolve"),
            Some(buyer.public_key().to_hex())
        );
        assert!(response["token_expires_at"].as_i64().expect("expiry") > 0);
    }
}
