//! Gate 2 source coverage ONLY: synthetic identities, mock LNURL invoices,
//! authenticated production routes and private SQLite. No Lightning payments.
use adp_core::{
    build_payment_legs, build_terms_event_builder, CommercialTerms, FeePolicy, QuoteState,
    TermsPayload,
};
use adp_server::{
    config::Config,
    keystore::KeyStore,
    relay::{Relay, RelayLookup, RelayResult},
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
use bitcoin::{
    hashes::{sha256, Hash},
    secp256k1::{Secp256k1, SecretKey},
};
use http_body_util::BodyExt;
use lightning_invoice::{Currency, InvoiceBuilder, PaymentSecret};
use nostr_sdk::{Event, EventBuilder, Keys, Kind, PublicKey, Tag, Timestamp};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};
use tower::ServiceExt;
use wiremock::{
    matchers::{method, path},
    Mock, MockServer, ResponseTemplate,
};

struct FixtureRelay {
    listing: Mutex<Event>,
}
#[async_trait]
impl Relay for FixtureRelay {
    fn server_url(&self) -> &str {
        "http://localhost:19099"
    }
    async fn publish(&self, _: &Event) -> RelayResult<()> {
        Ok(())
    }
    async fn lookup_listing(&self, _: PublicKey, _: &str) -> RelayLookup<Event> {
        RelayLookup::Found(self.listing.lock().expect("listing").clone())
    }
}
struct Fixture {
    _dir: tempfile::TempDir,
    _lnurl: MockServer,
    db_path: std::path::PathBuf,
    store: Storage,
    buyer: Keys,
    developer: Keys,
    operators: [Keys; 2],
    terms: Vec<Event>,
    listing: Event,
    relay: Arc<FixtureRelay>,
    clock: Arc<AtomicU64>,
    destinations: [String; 3],
}
fn signed_terms(keys: &Keys, destination: &str, fee: u64) -> Event {
    build_terms_event_builder(&TermsPayload::Offer {
        terms: CommercialTerms {
            version: 1,
            operator_pubkey: keys.public_key().to_hex(),
            payment_destination: destination.into(),
            fee: FeePolicy::Fixed { amount_msat: fee },
            limitations: vec![],
        },
    })
    .expect("terms")
    .sign_with_keys(keys)
    .expect("sign terms")
}
impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().expect("private DB");
        let db_path = dir.path().join("processor-a.db");
        let store = Storage::connect(db_path.to_str().expect("path"))
            .await
            .expect("store");
        let buyer = Keys::generate();
        let developer = Keys::generate();
        let operators = [Keys::generate(), Keys::generate()];
        let lnurl = MockServer::start().await;
        let destinations = [
            format!("{}/a", lnurl.uri()),
            format!("{}/b", lnurl.uri()),
            format!("{}/developer", lnurl.uri()),
        ];
        for (index, (name, amount)) in [("a", 100_000), ("b", 150_000), ("developer", 750_000)]
            .into_iter()
            .enumerate()
        {
            let metadata = json!({"callback":format!("{}/{name}/callback",lnurl.uri()),"minSendable":1,"maxSendable":1_000_000,"nostrPubkey":operators[0].public_key().to_hex(),"allowsNostr":false});
            Mock::given(method("GET"))
                .and(path(format!("/{name}")))
                .respond_with(ResponseTemplate::new(200).set_body_json(metadata))
                .mount(&lnurl)
                .await;
            let invoice = InvoiceBuilder::new(Currency::Regtest)
                .description("synthetic Gate 2 source invoice".into())
                .payment_hash(sha256::Hash::hash(&[index as u8 + 1; 32]))
                .payment_secret(PaymentSecret([4; 32]))
                .amount_milli_satoshis(amount)
                .duration_since_epoch(std::time::Duration::from_secs(Timestamp::now().as_secs()))
                .expiry_time(std::time::Duration::from_secs(600))
                .min_final_cltv_expiry_delta(40)
                .build_signed(|h| {
                    Secp256k1::new().sign_ecdsa_recoverable(
                        h,
                        &SecretKey::from_slice(&[42; 32]).expect("synthetic invoice key"),
                    )
                })
                .expect("invoice")
                .to_string();
            Mock::given(method("GET"))
                .and(path(format!("/{name}/callback")))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({"pr":invoice})))
                .mount(&lnurl)
                .await;
        }
        let terms = vec![
            signed_terms(&operators[0], &destinations[0], 100_000),
            signed_terms(&operators[1], &destinations[1], 150_000),
        ];
        let listing = Self::listing(&developer, &terms, &destinations[2]);
        let relay = Arc::new(FixtureRelay {
            listing: Mutex::new(listing.clone()),
        });
        Self {
            _dir: dir,
            _lnurl: lnurl,
            db_path,
            store,
            buyer,
            developer,
            operators,
            terms,
            listing,
            relay,
            clock: Arc::new(AtomicU64::new(Timestamp::now().as_secs())),
            destinations,
        }
    }
    fn listing(developer: &Keys, terms: &[Event], destination: &str) -> Event {
        let mut tags = vec![
            Tag::parse(["d", "gate2-source"]).expect("d"),
            Tag::parse(["title", "synthetic multi-operator test"]).expect("title"),
            Tag::parse(["price", "1000", "sat"]).expect("price"),
            Tag::parse(["lud16", destination]).expect("destination"),
            Tag::parse(["server", "http://localhost:19099"]).expect("server A"),
            Tag::parse(["server", "http://localhost:19100"]).expect("server B"),
        ];
        for event in terms {
            tags.push(
                Tag::parse(["adp_terms", &event.pubkey.to_hex(), &event.id.to_hex()])
                    .expect("terms reference"),
            );
        }
        EventBuilder::new(Kind::Custom(30402), "test")
            .tags(tags)
            .sign_with_keys(developer)
            .expect("listing")
    }
    fn app(&self) -> axum::Router {
        let config = Config {
            server_url: "http://localhost:19099".into(),
            bind_addr: "127.0.0.1:0".into(),
            db_path: self.db_path.to_string_lossy().into(),
            files_dir: self._dir.path().join("files").to_string_lossy().into(),
            max_upload_bytes: 1024,
            relays: vec![],
            operator_nsec: self.operators[0].secret_key().to_secret_hex(),
            master_key: [7; 32],
            allow_insecure_lnurl: true,
            download_token_ttl_secs: 900,
            file_reverify_interval_secs: 3600,
        };
        let clock = self.clock.clone();
        routes::router_with_settlement_clock(
            AppState {
                config: Arc::new(config),
                storage: self.store.clone(),
                keystore: Arc::new(KeyStore::new(self.store.pool(), [7; 32])),
                relay: self.relay.clone(),
                operator_keys: self.operators[0].clone(),
            },
            SettlementClock::new(move || Some(clock.load(Ordering::SeqCst))),
        )
    }
    async fn call(&self, path: &str, body: Value) -> (StatusCode, Value) {
        let auth = EventBuilder::new(Kind::Custom(27235), "")
            .tags([
                Tag::parse(["u", &format!("http://localhost:19099{path}")]).expect("URL"),
                Tag::parse(["method", "POST"]).expect("method"),
            ])
            .sign_with_keys(&self.buyer)
            .expect("auth");
        let response = self
            .app()
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
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }
    fn request(&self) -> Value {
        json!({"purchase_id":"gate2-source","listing_event":self.listing,"terms_events":self.terms,"refund_destination":{"endpoint":"buyer@example.com","min_sendable_msat":1,"max_sendable_msat":1_000_000},"zap_requests":{}})
    }
    async fn active_quote(&self) -> adp_core::SignedQuote {
        let (status, response) = self.call("/settlement/quote", self.request()).await;
        assert_eq!(status, StatusCode::OK);
        let event: Event = serde_json::from_value(response["quote"].clone()).expect("quote event");
        let plaintext = nostr_sdk::nips::nip44::decrypt(
            self.buyer.secret_key(),
            &self.operators[0].public_key(),
            &event.content,
        )
        .expect("quote decrypt");
        adp_core::verify_quote_event(&event, &plaintext).expect("verified quote")
    }
    async fn no_capabilities(&self) {
        for table in ["purchases", "download_tokens"] {
            let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
                .fetch_one(&self.store.pool())
                .await
                .expect("count");
            assert_eq!(count, 0);
        }
    }
}

#[tokio::test]
async fn three_recipient_quote_uses_independent_signed_terms_exact_sum_and_order() {
    let f = Fixture::new().await;
    assert_ne!(f.operators[0].public_key(), f.operators[1].public_key());
    for term in &f.terms {
        term.verify().expect("signature");
        adp_core::parse_terms_event(term).expect("signed role binding");
    }
    let quote = f.active_quote().await;
    assert_eq!(
        quote.legs.iter().map(|l| l.amount_msat).collect::<Vec<_>>(),
        vec![100_000, 150_000, 750_000]
    );
    assert_eq!(
        quote
            .legs
            .iter()
            .map(|l| l.recipient_pubkey.clone())
            .collect::<Vec<_>>(),
        vec![
            f.operators[0].public_key().to_hex(),
            f.operators[1].public_key().to_hex(),
            f.developer.public_key().to_hex()
        ]
    );
    assert_eq!(
        quote.legs.iter().map(|l| l.amount_msat).sum::<u64>(),
        1_000_000
    );
    assert_eq!(
        quote.legs[0].accepted_terms_id,
        Some(f.terms[0].id.to_hex())
    );
    assert_eq!(
        quote.legs[1].accepted_terms_id,
        Some(f.terms[1].id.to_hex())
    );
    f.no_capabilities().await;
}

#[tokio::test]
async fn missing_or_forged_second_terms_are_rejected_before_invoice_preflight() {
    let f = Fixture::new().await;
    let mut missing = f.request();
    missing["terms_events"] = json!([f.terms[0]]);
    assert_eq!(
        f.call("/settlement/quote", missing).await.0,
        StatusCode::BAD_REQUEST
    );
    let mut forged = f.request();
    forged["terms_events"][1]["sig"] = json!("00".repeat(64));
    assert_eq!(
        f.call("/settlement/quote", forged).await.0,
        StatusCode::BAD_REQUEST
    );
    assert!(f
        ._lnurl
        .received_requests()
        .await
        .expect("requests")
        .is_empty());
    f.no_capabilities().await;
}

#[tokio::test]
async fn duplicated_operator_and_conflicting_fee_terms_are_rejected() {
    let f = Fixture::new().await;
    let mut duplicate = f.request();
    duplicate["terms_events"] = json!([f.terms[0], f.terms[0]]);
    assert_eq!(
        f.call("/settlement/quote", duplicate).await.0,
        StatusCode::BAD_REQUEST
    );
    let alternate = signed_terms(&f.operators[1], &f.destinations[1], 200_000);
    let mut conflict = f.request();
    conflict["terms_events"] = json!([f.terms[0], alternate]);
    assert_eq!(
        f.call("/settlement/quote", conflict).await.0,
        StatusCode::BAD_REQUEST
    );
    let duplicate_listing = Fixture::listing(
        &f.developer,
        &[f.terms[0].clone(), f.terms[0].clone()],
        &f.destinations[2],
    );
    *f.relay.listing.lock().expect("listing") = duplicate_listing.clone();
    let mut duplicate = f.request();
    duplicate["listing_event"] = json!(duplicate_listing);
    assert_eq!(
        f.call("/settlement/quote", duplicate).await.0,
        StatusCode::BAD_REQUEST
    );
    assert!(f
        ._lnurl
        .received_requests()
        .await
        .expect("requests")
        .is_empty());
}

#[tokio::test]
async fn aggregate_operator_fees_equal_to_or_above_gross_are_rejected() {
    let f = Fixture::new().await;
    for fee in [500_000, 600_000] {
        let terms = vec![
            signed_terms(&f.operators[0], &f.destinations[0], fee),
            signed_terms(&f.operators[1], &f.destinations[1], fee),
        ];
        let listing = Fixture::listing(&f.developer, &terms, &f.destinations[2]);
        *f.relay.listing.lock().expect("listing") = listing.clone();
        let mut request = f.request();
        request["listing_event"] = json!(listing);
        request["terms_events"] = json!(terms);
        assert_eq!(
            f.call("/settlement/quote", request).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    assert!(f
        ._lnurl
        .received_requests()
        .await
        .expect("requests")
        .is_empty());
}

#[tokio::test]
async fn restart_retry_and_invalid_third_leg_never_grant_ownership_or_repeat_fees() {
    let mut f = Fixture::new().await;
    let q = f.active_quote().await;
    let original = f.call("/settlement/quote", f.request()).await.1["quote"].clone();
    for byte in [1u8, 2] {
        let (status, invoice) = f.call("/settlement/gate2-source/next", json!({})).await;
        assert_eq!(status, StatusCode::OK);
        let again = f.call("/settlement/gate2-source/next", json!({})).await.1;
        assert_eq!(invoice, again);
        let proof = json!({"attempt_id":invoice["attempt_id"],"preimage":hex::encode([byte;32])});
        for _ in 0..2 {
            let (status, result) = f
                .call("/settlement/gate2-source/proof", proof.clone())
                .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(result["state"], "active");
        }
    }
    f.store.pool().close().await;
    f.store = Storage::connect(f.db_path.to_str().expect("path"))
        .await
        .expect("restart");
    let (status, replayed) = f.call("/settlement/quote", f.request()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(original, replayed["quote"]);
    let (status, third) = f.call("/settlement/gate2-source/next", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(third["amount_msat"], 750_000);
    for _ in 0..2 {
        assert_eq!(
            f.call(
                "/settlement/gate2-source/proof",
                json!({"attempt_id":third["attempt_id"],"preimage":hex::encode([9;32])})
            )
            .await
            .0,
            StatusCode::PAYMENT_REQUIRED
        );
    }
    f.no_capabilities().await;
    f.clock.store(q.confirmation_expires_at, Ordering::SeqCst);
    let (status, failed) = f.call("/settlement/gate2-source/status", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(failed["state"], "failed");
    assert_eq!(failed["settled_legs"], 2);
    assert_eq!(
        f.store
            .settlement_status("gate2-source")
            .await
            .expect("status")
            .expect("quote")
            .state,
        QuoteState::Failed
    );
    let obligations: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT principal_msat,due_at FROM refund_obligations ORDER BY principal_msat",
    )
    .fetch_all(&f.store.pool())
    .await
    .expect("refund obligations");
    assert_eq!(
        obligations,
        vec![
            (100_000, (q.confirmation_expires_at + 604800) as i64),
            (150_000, (q.confirmation_expires_at + 604800) as i64)
        ]
    );
    let attempts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM invoice_attempts")
        .fetch_one(&f.store.pool())
        .await
        .expect("count");
    assert_eq!(attempts, 3);
    f.no_capabilities().await;
}

#[test]
fn alias_urls_are_one_operator_but_conflicting_acceptance_is_invalid() {
    let operator = Keys::generate().public_key().to_hex();
    let servers = vec![
        adp_core::ServerOperator {
            url: "https://one.example".into(),
            operator_pubkey: operator.clone(),
        },
        adp_core::ServerOperator {
            url: "https://alias.example".into(),
            operator_pubkey: operator.clone(),
        },
    ];
    let accepted = vec![adp_core::AcceptedTermsRef {
        operator_pubkey: operator.clone(),
        terms_event_id: "01".repeat(32),
    }];
    let coverage =
        adp_core::validate_terms_coverage(&servers, &accepted).expect("one accepted identity");
    assert_eq!(coverage.len(), 1);
    let terms = CommercialTerms {
        version: 1,
        operator_pubkey: operator,
        payment_destination: "op@example.com".into(),
        fee: FeePolicy::Fixed {
            amount_msat: 100_000,
        },
        limitations: vec![],
    };
    let legs = build_payment_legs(
        1_000_000,
        "developer",
        "dev@example.com",
        &[(terms, coverage[0].terms_event_id.clone())],
    )
    .expect("one fee");
    assert_eq!(legs.len(), 2);
    assert_eq!(legs[1].amount_msat, 900_000);
    assert!(adp_core::validate_terms_coverage(
        &servers,
        &[accepted[0].clone(), accepted[0].clone()]
    )
    .is_err());
}
