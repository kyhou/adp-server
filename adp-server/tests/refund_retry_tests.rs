//! Isolated refund replay/retry regressions, no wallet or external HTTP access.
use adp_server::{
    config::Config,
    crypto::MasterCipher,
    keystore::KeyStore,
    relay::{Relay, RelayResult},
    routes,
    state::AppState,
    storage::{NewSettlementAttempt, Storage},
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
use nostr_sdk::{Event, EventBuilder, Keys, Kind, Tag};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

struct QuietRelay;
#[async_trait]
impl Relay for QuietRelay {
    fn server_url(&self) -> &str {
        "http://localhost:9099"
    }
    async fn publish(&self, _: &Event) -> RelayResult<()> {
        Ok(())
    }
}

async fn fixture() -> (tempfile::TempDir, Storage, axum::Router, Keys, Keys, String) {
    let dir = tempfile::tempdir().expect("tempdir");
    let storage = Storage::connect(dir.path().join("adp.db").to_str().expect("path"))
        .await
        .expect("storage");
    let recipient = Keys::generate();
    let buyer = Keys::generate();
    let operator = Keys::generate();
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
    let app = routes::router(AppState {
        config: Arc::new(config),
        storage: storage.clone(),
        keystore: Arc::new(KeyStore::new(storage.pool(), [7; 32])),
        relay: Arc::new(QuietRelay),
        operator_keys: operator,
    });
    seed_quote(&storage, &recipient, &buyer, "q").await;
    (dir, storage, app, recipient, buyer, invoice_for(3))
}

fn invoice_for(byte: u8) -> String {
    InvoiceBuilder::new(Currency::Regtest)
        .description("synthetic refund".into())
        .payment_hash(sha256::Hash::hash(&[byte; 32]))
        .payment_secret(PaymentSecret([4; 32]))
        .amount_milli_satoshis(1000)
        .duration_since_epoch(std::time::Duration::from_secs(
            nostr_sdk::Timestamp::now().as_secs(),
        ))
        .min_final_cltv_expiry_delta(144)
        .build_signed(|h| {
            Secp256k1::new()
                .sign_ecdsa_recoverable(h, &SecretKey::from_slice(&[42; 32]).expect("key"))
        })
        .expect("invoice")
        .to_string()
}

async fn seed_quote(storage: &Storage, recipient: &Keys, buyer: &Keys, id: &str) {
    sqlx::query("INSERT INTO purchase_quotes (quote_id,quote_event_id,quote_event_json,buyer_pubkey,listing_coordinate,listing_event_id,processor_pubkey,gross_msat,state,issued_at,payment_expires_at,confirmation_expires_at,failure_at) VALUES (?,?,'{}',?,'30402:dev:game','listing','processor',1000,'failed',100,1900,86500,1900)")
        .bind(id).bind(format!("quote-{id}")).bind(buyer.public_key().to_hex()).execute(&storage.pool()).await.expect("quote");
    sqlx::query("INSERT INTO payment_legs (quote_id,leg_id,leg_order,role,recipient_pubkey,destination_encrypted,amount_msat,disclosed_at,settled_at) VALUES (?,'developer',0,'developer',?,X'01',1000,100,200)")
        .bind(id).bind(recipient.public_key().to_hex()).execute(&storage.pool()).await.expect("paid leg");
    let destination = json!({"endpoint":"http://127.0.0.1:1/.well-known/lnurlp/buyer","min_sendable_msat":1,"max_sendable_msat":1000});
    let encrypted = serde_json::to_vec(
        &MasterCipher::new(&[7; 32])
            .expect("cipher")
            .encrypt(destination.to_string().as_bytes()),
    )
    .expect("cipher JSON");
    sqlx::query("INSERT INTO refund_destinations (destination_id,quote_id,commitment,destination_encrypted,min_sendable_msat,max_sendable_msat,authorized_at) VALUES (?,?,'commitment',?,1,1000,100)")
        .bind(format!("dest-{id}")).bind(id).bind(encrypted).execute(&storage.pool()).await.expect("destination");
    storage
        .process_settlement_deadlines(id, 1900)
        .await
        .expect("obligation");
}

async fn save(
    storage: &Storage,
    quote: &str,
    id: &str,
    invoice: &str,
) -> Result<(), adp_server::storage::PersistenceError> {
    let context = storage
        .refund_context(quote, "developer")
        .await
        .expect("context")
        .expect("owed");
    let encrypted = serde_json::to_vec(
        &MasterCipher::new(&[7; 32])
            .expect("cipher")
            .encrypt(invoice.as_bytes()),
    )
    .expect("cipher JSON");
    storage
        .save_refund_attempt(
            quote,
            &context,
            &NewSettlementAttempt {
                attempt_id: id.into(),
                leg_id: "developer".into(),
                encrypted_bolt11: encrypted,
                lnurl_origin: "http://127.0.0.1:1".into(),
                callback: "http://127.0.0.1:1/callback".into(),
                lsp_pubkey: "synthetic".into(),
                payment_hash: invoice
                    .parse::<lightning_invoice::Bolt11Invoice>()
                    .expect("invoice")
                    .payment_hash()
                    .to_string(),
                amount_msat: 1000,
                expires_at: u32::MAX as u64,
            },
            100,
        )
        .await
        .map(|_| ())
}

async fn call(app: axum::Router, signer: &Keys, path: &str, body: Value) -> (StatusCode, Value) {
    let event = EventBuilder::new(Kind::Custom(27235), "")
        .tags([
            Tag::parse(["u", &format!("http://localhost:9099{path}")]).expect("u"),
            Tag::parse(["method", "POST"]).expect("method"),
        ])
        .sign_with_keys(signer)
        .expect("auth");
    let auth = format!(
        "Nostr {}",
        base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(&event).expect("JSON"))
    );
    let response = app
        .oneshot(
            Request::post(path)
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
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn preparation_reuses_existing_invoice_without_contacting_lnurl() {
    let (_dir, storage, app, recipient, _buyer, invoice) = fixture().await;
    save(&storage, "q", "original", &invoice)
        .await
        .expect("attempt");
    for _ in 0..3 {
        let (status, result) = call(
            app.clone(),
            &recipient,
            "/settlement/q/refund/developer/prepare",
            json!({}),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "existing invoice must be reused even when LNURL is unavailable"
        );
        assert_eq!(result["attempt_id"], "original");
        assert_eq!(result["bolt11"], invoice);
    }
}

#[tokio::test]
async fn refund_obligation_reads_are_scoped_to_recipient_or_authorized_buyer() {
    let (_dir, storage, app, recipient, buyer, _invoice) = fixture().await;
    let event = EventBuilder::new(Kind::Custom(1033), "synthetic snapshot")
        .sign_with_keys(&recipient)
        .expect("event");
    let listing = EventBuilder::new(Kind::Custom(30402), "synthetic listing")
        .sign_with_keys(&recipient)
        .expect("listing");
    sqlx::query("UPDATE purchase_quotes SET quote_event_json=?,listing_event_json=?,quote_plaintext_encrypted=X'01' WHERE quote_id='q'")
        .bind(serde_json::to_string(&event).expect("JSON")).bind(serde_json::to_string(&listing).expect("JSON")).execute(&storage.pool()).await.expect("read-route snapshots");
    let (status, rows) = call(app.clone(), &recipient, "/settlement/refunds", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rows["obligations"].as_array().expect("rows").len(), 1);
    let (status, rows) = call(app.clone(), &buyer, "/settlement/refunds", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert!(rows["obligations"].as_array().expect("rows").is_empty());
    assert_eq!(
        call(app.clone(), &recipient, "/settlement/q/refunds", json!({}))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(app, &buyer, "/settlement/q/refunds", json!({}))
            .await
            .0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn concurrent_attempt_creation_is_one_per_obligation_and_confirmation_is_idempotent() {
    let (_dir, storage, app, recipient, buyer, invoice) = fixture().await;
    let second = invoice_for(5);
    let (a, b) = tokio::join!(
        save(&storage, "q", "first", &invoice),
        save(&storage, "q", "second", &second)
    );
    a.expect("first prepare");
    b.expect("concurrent prepare");
    let attempts: Vec<String> =
        sqlx::query_scalar("SELECT attempt_id FROM refund_attempts WHERE quote_id='q'")
            .fetch_all(&storage.pool())
            .await
            .expect("attempts");
    assert_eq!(
        attempts.len(),
        1,
        "concurrent prepares must return the same persisted refund attempt"
    );
    let byte = if attempts[0] == "first" { 3 } else { 5 };
    let body = json!({"attempt_id":attempts[0],"preimage":hex::encode([byte;32])});
    assert_eq!(
        call(
            app.clone(),
            &buyer,
            "/settlement/q/refund/developer/confirm",
            body.clone()
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    for _ in 0..3 {
        assert_eq!(
            call(
                app.clone(),
                &recipient,
                "/settlement/q/refund/developer/confirm",
                body.clone()
            )
            .await
            .0,
            StatusCode::OK
        );
    }
    assert_eq!(
        call(
            app.clone(),
            &recipient,
            "/settlement/q/refund/developer/confirm",
            json!({"attempt_id":attempts[0],"preimage":hex::encode([8;32])})
        )
        .await
        .0,
        StatusCode::PAYMENT_REQUIRED
    );
    assert_eq!(
        call(
            app,
            &recipient,
            "/settlement/q/refund/developer/prepare",
            json!({})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn refund_payment_proof_cannot_discharge_two_obligations() {
    let (_dir, storage, _app, recipient, buyer, invoice) = fixture().await;
    seed_quote(&storage, &recipient, &buyer, "q2").await;
    save(&storage, "q", "first", &invoice)
        .await
        .expect("first attempt");
    assert!(
        save(&storage, "q2", "second", &invoice).await.is_err(),
        "refund invoice hashes are globally unique"
    );
    let hash = sha256::Hash::hash(&[3; 32]).to_string();
    assert!(storage
        .verify_refund_attempt("q", "first", &hash, b"encrypted proof", 201)
        .await
        .expect("first proof"));
    assert!(!storage
        .verify_refund_attempt("q2", "second", &hash, b"same encrypted proof", 202)
        .await
        .expect("reject replay"));
}

#[tokio::test]
async fn recovery_identity_is_stable_per_evidence_not_per_request() {
    let (_dir, storage, _app, recipient, _buyer, _invoice) = fixture().await;
    let signer = recipient.public_key().to_hex();
    let original = EventBuilder::new(Kind::Custom(1034), "first encrypted evidence")
        .sign_with_keys(&recipient)
        .expect("event");
    let retry = EventBuilder::new(Kind::Custom(1034), "new encryption of same evidence")
        .sign_with_keys(&recipient)
        .expect("retry event");
    let first = storage
        .save_recovery_claim("q", &signer, "processor", &original, "evidence-a")
        .await
        .expect("first");
    let again = storage
        .save_recovery_claim("q", &signer, "processor", &retry, "evidence-a")
        .await
        .expect("retry");
    assert_eq!(first.id, again.id);
    let late = storage
        .save_recovery_claim("q", &signer, "processor", &retry, "evidence-b")
        .await
        .expect("late evidence");
    assert_ne!(first.id, late.id);
    first.verify().expect("original signature");
    late.verify().expect("new signature");
}

#[tokio::test]
async fn exact_legacy_recovery_replay_retains_its_signed_event() {
    let (_dir, storage, _app, recipient, _buyer, _invoice) = fixture().await;
    let signer = recipient.public_key().to_hex();
    let event = EventBuilder::new(Kind::Custom(1034), "legacy encrypted evidence")
        .sign_with_keys(&recipient)
        .expect("legacy event");
    sqlx::query("INSERT INTO recovery_claims (claim_event_id,quote_id,claimant_pubkey,claimant_role,event_json,created_at) VALUES (?,'q',?,'processor',?,?)")
        .bind(event.id.to_hex()).bind(&signer).bind(serde_json::to_string(&event).expect("JSON"))
        .bind(event.created_at.as_secs() as i64).execute(&storage.pool()).await.expect("legacy row");
    let adopted = storage
        .save_recovery_claim("q", &signer, "processor", &event, "legacy-evidence")
        .await
        .expect("adopt");
    assert_eq!(event, adopted);
    adopted.verify().expect("unchanged signature");
}
