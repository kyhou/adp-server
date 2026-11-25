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
    relay::{Relay, RelayLookup, RelayResult, MIN_SUCCESSFUL_RELAYS},
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
use nostr_sdk::{Event, EventBuilder, Keys, Kind, PublicKey, Tag, Timestamp};
use serde_json::{json, Value};
use tower::ServiceExt;

/// Publication can fail on demand. Current-listing and receipt lookups
/// answer from configurable evidence; unset evidence is unavailable.
struct RetryRelay {
    fail_publish: AtomicBool,
    current_listing: std::sync::Mutex<Option<RelayLookup<Event>>>,
    receipts: std::sync::Mutex<Option<RelayLookup<Vec<Event>>>>,
}

impl RetryRelay {
    fn new(fail_publish: bool) -> Self {
        Self {
            fail_publish: AtomicBool::new(fail_publish),
            current_listing: std::sync::Mutex::new(None),
            receipts: std::sync::Mutex::new(None),
        }
    }
}

fn unavailable<T>() -> RelayLookup<T> {
    RelayLookup::Unavailable {
        successful_relays: 0,
        failed_relays: 2,
    }
}

#[async_trait]
impl Relay for RetryRelay {
    fn server_url(&self) -> &str {
        "http://localhost:9099"
    }
    async fn publish(&self, _: &Event) -> RelayResult<()> {
        if self.fail_publish.load(Ordering::SeqCst) {
            Err(std::io::Error::other("injected publication failure").into())
        } else {
            Ok(())
        }
    }
    async fn lookup_current_listing(&self, _: PublicKey, _: &str) -> RelayLookup<Event> {
        self.current_listing
            .lock()
            .expect("listing evidence")
            .clone()
            .unwrap_or_else(unavailable)
    }
    async fn lookup_receipts(&self, _: PublicKey, _: &str) -> RelayLookup<Vec<Event>> {
        self.receipts
            .lock()
            .expect("receipt evidence")
            .clone()
            .unwrap_or_else(unavailable)
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

struct SettledQuote {
    _dir: tempfile::TempDir,
    path: std::path::PathBuf,
    storage: Storage,
    buyer: Keys,
    operator: Keys,
    listing: Event,
    receipt: Event,
    coordinate: String,
}

/// A settled one-leg quote whose aggregate receipt is committed but whose
/// relay publication and token issuance have not completed.
async fn settled_quote() -> SettledQuote {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("retry.db");
    let storage = Storage::connect(path.to_str().expect("path"))
        .await
        .expect("storage");
    let buyer = Keys::generate();
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
    SettledQuote {
        _dir: dir,
        path,
        storage,
        buyer,
        operator,
        listing,
        receipt,
        coordinate: quote.listing_coordinate,
    }
}

impl SettledQuote {
    /// A newer current listing naming `servers`, as relays would return it.
    fn current_listing(&self, servers: &[&str]) -> RelayLookup<Event> {
        let mut tags = vec![Tag::parse(["d", "game"]).expect("identifier")];
        tags.extend(
            servers
                .iter()
                .map(|url| Tag::parse(["server", url]).expect("server")),
        );
        RelayLookup::Found(
            EventBuilder::new(Kind::Custom(30402), "current")
                .tags(tags)
                .custom_created_at(Timestamp::from_secs(self.listing.created_at.as_secs() + 1))
                .sign_with_keys(&self.operator)
                .expect("current listing"),
        )
    }

    /// The developer's own `refunded` status for the stored receipt.
    fn refunded(&self) -> Event {
        let mut tags = self
            .receipt
            .tags
            .iter()
            .filter(|tag| matches!(tag.as_slice()[0].as_str(), "order" | "p" | "a"))
            .cloned()
            .collect::<Vec<_>>();
        tags.extend([
            Tag::parse(["e", &self.receipt.id.to_hex()]).expect("e"),
            Tag::parse(["status", "refunded"]).expect("status"),
        ]);
        EventBuilder::new(Kind::Custom(1020), self.receipt.content.clone())
            .tags(tags)
            .custom_created_at(Timestamp::from_secs(self.receipt.created_at.as_secs() + 1))
            .sign_with_keys(&self.operator)
            .expect("refund status")
    }

    /// Relay evidence under which a fresh token is permitted.
    fn entitled_relay(&self) -> Arc<RetryRelay> {
        let relay = Arc::new(RetryRelay::new(false));
        *relay.current_listing.lock().expect("listing") =
            Some(self.current_listing(&["http://localhost:9099"]));
        *relay.receipts.lock().expect("receipts") =
            Some(RelayLookup::Found(vec![self.receipt.clone()]));
        relay
    }

    async fn counts(&self) -> (i64, i64) {
        let pool = self.storage.pool();
        let receipts: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM purchase_quotes WHERE receipt_event_id IS NOT NULL",
        )
        .fetch_one(&pool)
        .await
        .expect("receipts");
        let tokens: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM download_tokens")
            .fetch_one(&pool)
            .await
            .expect("tokens");
        (receipts, tokens)
    }
}

#[tokio::test]
async fn persisted_receipt_retry_after_restart_recovers_download_token() {
    let fixture = settled_quote().await;
    let (buyer, operator, receipt) = (&fixture.buyer, &fixture.operator, &fixture.receipt);
    let stranger = Keys::generate();
    let relay = fixture.entitled_relay();
    relay.fail_publish.store(true, Ordering::SeqCst);
    let (status, _) = retry(app(fixture.storage.clone(), operator, relay.clone()), buyer).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    fixture.storage.pool().close().await;
    let storage = Storage::connect(fixture.path.to_str().expect("path"))
        .await
        .expect("restart storage");
    relay.fail_publish.store(false, Ordering::SeqCst);
    let (status, stranger_body) =
        retry(app(storage.clone(), operator, relay.clone()), &stranger).await;
    assert!(
        stranger_body.get("receipt").is_none(),
        "never disclosed to another buyer"
    );
    assert_eq!(status, StatusCode::FORBIDDEN);
    for _ in 0..2 {
        let (status, response) = retry(app(storage.clone(), operator, relay.clone()), buyer).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            response["receipt"],
            serde_json::to_value(receipt).expect("receipt")
        );
        let token = response["download_token"]
            .as_str()
            .expect("retry must return download token");
        assert_eq!(
            storage
                .resolve_download_token(token, &fixture.coordinate)
                .await
                .expect("resolve"),
            Some(buyer.public_key().to_hex())
        );
        assert!(response["token_expires_at"].as_i64().expect("expiry") > 0);
    }
}

/// Settles a quote, applies `evidence` to an otherwise entitled relay, and
/// retries twice: each retry recovers the identical receipt without a token,
/// creates no token row, and never a second receipt or settlement.
async fn assert_recovered_without_token(evidence: impl FnOnce(&SettledQuote, &RetryRelay)) {
    let fixture = settled_quote().await;
    let relay = fixture.entitled_relay();
    evidence(&fixture, &relay);
    let before = fixture.counts().await;
    for _ in 0..2 {
        let (status, response) = retry(
            app(fixture.storage.clone(), &fixture.operator, relay.clone()),
            &fixture.buyer,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "receipt recovery must succeed: {response}"
        );
        assert_eq!(response["state"], "settled");
        assert_eq!(
            response["receipt"],
            serde_json::to_value(&fixture.receipt).expect("receipt")
        );
        assert!(response.get("download_token").is_none(), "token withheld");
        assert!(response.get("token_expires_at").is_none());
    }
    assert_eq!(
        fixture.counts().await,
        before,
        "no new receipt, settlement or token"
    );
}

#[tokio::test]
async fn split_recovery_after_listing_removal_withholds_the_token() {
    assert_recovered_without_token(|_, relay| {
        *relay.current_listing.lock().expect("listing") = Some(RelayLookup::ConfirmedAbsent {
            successful_relays: MIN_SUCCESSFUL_RELAYS,
        });
    })
    .await;
}

#[tokio::test]
async fn split_recovery_after_distribution_removal_withholds_the_token() {
    assert_recovered_without_token(|fixture, relay| {
        *relay.current_listing.lock().expect("listing") =
            Some(fixture.current_listing(&["https://other.example"]));
    })
    .await;
}

#[tokio::test]
async fn split_recovery_after_a_refund_withholds_the_token() {
    assert_recovered_without_token(|fixture, relay| {
        *relay.receipts.lock().expect("receipts") = Some(RelayLookup::Found(vec![
            fixture.receipt.clone(),
            fixture.refunded(),
        ]));
    })
    .await;
}

#[tokio::test]
async fn split_recovery_with_unavailable_evidence_withholds_the_token() {
    assert_recovered_without_token(|_, relay| {
        *relay.current_listing.lock().expect("listing") = Some(unavailable());
    })
    .await;
    assert_recovered_without_token(|_, relay| {
        *relay.receipts.lock().expect("receipts") = Some(unavailable());
    })
    .await;
}

async fn confirm_with_quote(
    app: axum::Router,
    buyer: &Keys,
    listing: &Event,
    coordinate: &str,
) -> (StatusCode, Value) {
    let event = EventBuilder::new(Kind::Custom(27235), "")
        .tags([
            Tag::parse(["u", "http://localhost:9099/purchase/confirm"]).expect("u"),
            Tag::parse(["method", "POST"]).expect("method"),
        ])
        .sign_with_keys(buyer)
        .expect("sign auth");
    let auth = format!(
        "Nostr {}",
        base64::engine::general_purpose::STANDARD
            .encode(serde_json::to_vec(&event).expect("auth JSON"))
    );
    let body = json!({
        "game_coordinate": coordinate,
        "listing_event": listing,
        "quote_id": "retry",
    });
    let response = app
        .oneshot(
            Request::post("/purchase/confirm")
                .header("Authorization", auth)
                .header("Content-Type", "application/json")
                .body(Body::from(body.to_string()))
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
    (status, serde_json::from_slice(&bytes).expect("JSON"))
}

#[tokio::test]
async fn quote_confirm_recovers_the_receipt_after_the_listing_changes() {
    let fixture = settled_quote().await;
    let relay = fixture.entitled_relay();
    // The publisher revised the listing (dropping this server) after the
    // quote: the quote binds its own listing snapshot, not the current one.
    *relay.current_listing.lock().expect("listing") =
        Some(fixture.current_listing(&["https://other.example"]));
    let before = fixture.counts().await;
    let app = || app(fixture.storage.clone(), &fixture.operator, relay.clone());
    let (status, response) =
        confirm_with_quote(app(), &fixture.buyer, &fixture.listing, &fixture.coordinate).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(
        response["receipt"],
        serde_json::to_value(&fixture.receipt).expect("receipt")
    );
    assert!(response.get("download_token").is_none());
    assert_eq!(fixture.counts().await, before);

    // Still distributed and entitled: the same recovery carries a token.
    *relay.current_listing.lock().expect("listing") =
        Some(fixture.current_listing(&["http://localhost:9099"]));
    let (status, response) =
        confirm_with_quote(app(), &fixture.buyer, &fixture.listing, &fixture.coordinate).await;
    assert_eq!(status, StatusCode::OK);
    assert!(response["download_token"].as_str().is_some());

    // Another buyer, or a listing other than the quoted snapshot, is refused
    // without disclosing the receipt.
    let (status, response) = confirm_with_quote(
        app(),
        &Keys::generate(),
        &fixture.listing,
        &fixture.coordinate,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(response.get("receipt").is_none());
    let other_snapshot = EventBuilder::new(Kind::Custom(30402), "other snapshot")
        .tags([Tag::parse(["d", "game"]).expect("identifier")])
        .sign_with_keys(&fixture.operator)
        .expect("other snapshot");
    let (status, response) =
        confirm_with_quote(app(), &fixture.buyer, &other_snapshot, &fixture.coordinate).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(response.get("receipt").is_none());
}
