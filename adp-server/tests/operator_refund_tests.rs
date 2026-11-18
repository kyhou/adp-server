//! Operator refund tooling against an actual router over loopback HTTP.
//! Synthetic obligation and offline invoice; no wallet, LNURL or payment.
use adp_server::{
    config::Config,
    crypto::MasterCipher,
    keystore::KeyStore,
    operator_refunds::{render_obligations, OperatorRefundClient},
    relay::{Relay, RelayResult},
    routes,
    state::AppState,
    storage::{NewSettlementAttempt, Storage},
};
use async_trait::async_trait;
use bitcoin::{
    hashes::{sha256, Hash},
    secp256k1::{Secp256k1, SecretKey},
};
use lightning_invoice::{Currency, InvoiceBuilder, PaymentSecret};
use nostr_sdk::{Event, Keys};
use serde_json::json;
use std::sync::Arc;

struct QuietRelay(String);
#[async_trait]
impl Relay for QuietRelay {
    fn server_url(&self) -> &str {
        &self.0
    }
    async fn publish(&self, _: &Event) -> RelayResult<()> {
        Ok(())
    }
}

fn invoice(byte: u8) -> String {
    InvoiceBuilder::new(Currency::Regtest)
        .description("synthetic operator refund".into())
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

fn encrypt(bytes: &[u8]) -> Vec<u8> {
    serde_json::to_vec(&MasterCipher::new(&[7; 32]).expect("cipher").encrypt(bytes))
        .expect("cipher JSON")
}

async fn server(operator: &Keys) -> (tempfile::TempDir, Storage, String) {
    let dir = tempfile::tempdir().expect("tempdir");
    let storage = Storage::connect(dir.path().join("adp.db").to_str().expect("path"))
        .await
        .expect("storage");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let url = format!("http://{}", listener.local_addr().expect("addr"));
    let config = Config {
        server_url: url.clone(),
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
        relay: Arc::new(QuietRelay(url.clone())),
        operator_keys: operator.clone(),
    });
    tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    (dir, storage, url)
}

/// A failed quote whose operator leg settled: the operator owes its principal back.
async fn seed_obligation(storage: &Storage, operator: &Keys, buyer: &Keys) {
    sqlx::query("INSERT INTO purchase_quotes (quote_id,quote_event_id,quote_event_json,buyer_pubkey,listing_coordinate,listing_event_id,processor_pubkey,gross_msat,state,issued_at,payment_expires_at,confirmation_expires_at,failure_at) VALUES ('q','quote-q','{}',?,'30402:dev:game','listing','processor',2000,'failed',100,1900,86500,1900)")
        .bind(buyer.public_key().to_hex()).execute(&storage.pool()).await.expect("quote");
    sqlx::query("INSERT INTO payment_legs (quote_id,leg_id,leg_order,role,recipient_pubkey,destination_encrypted,amount_msat,disclosed_at,settled_at) VALUES ('q','operator:op',0,'operator',?,X'01',1000,100,200)")
        .bind(operator.public_key().to_hex()).execute(&storage.pool()).await.expect("leg");
    let destination = json!({"endpoint":"http://127.0.0.1:1/.well-known/lnurlp/buyer","min_sendable_msat":1,"max_sendable_msat":1000});
    sqlx::query("INSERT INTO refund_destinations (destination_id,quote_id,commitment,destination_encrypted,min_sendable_msat,max_sendable_msat,authorized_at) VALUES ('dest','q','commitment',?,1,1000,100)")
        .bind(encrypt(destination.to_string().as_bytes())).execute(&storage.pool()).await.expect("destination");
    storage
        .process_settlement_deadlines("q", 1900)
        .await
        .expect("obligation");
}

/// The original invoice, as the server would obtain it from the buyer's LNURL.
async fn original_invoice(storage: &Storage, bolt11: &str) {
    let context = storage
        .refund_context("q", "operator:op")
        .await
        .expect("context")
        .expect("owed");
    storage
        .save_refund_attempt(
            "q",
            &context,
            &NewSettlementAttempt {
                attempt_id: "original".into(),
                leg_id: "operator:op".into(),
                encrypted_bolt11: encrypt(bolt11.as_bytes()),
                lnurl_origin: "http://127.0.0.1:1".into(),
                callback: "http://127.0.0.1:1/callback".into(),
                lsp_pubkey: "synthetic".into(),
                payment_hash: bolt11
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
        .expect("attempt");
}

#[tokio::test]
async fn operator_lists_prepares_and_confirms_its_own_refund_exactly_once() {
    let operator = Keys::generate();
    let buyer = Keys::generate();
    let (_dir, storage, url) = server(&operator).await;
    seed_obligation(&storage, &operator, &buyer).await;
    let client = OperatorRefundClient::new(&url, operator.clone()).expect("client");

    let owed = client.obligations().await.expect("list");
    assert_eq!(owed.len(), 1);
    // Deadlines run on the real clock: this long-past failure is already overdue,
    // which remains payable.
    assert_eq!(owed[0]["state"], "overdue");
    assert_eq!(owed[0]["principal_msat"], 1000);
    assert!(render_obligations(&owed).contains("overdue"));

    let bolt11 = invoice(3);
    original_invoice(&storage, &bolt11).await;
    // Repeated preparation returns the one original invoice, never a second payable one.
    for _ in 0..2 {
        let prepared = client.prepare("q", "operator:op").await.expect("prepare");
        assert_eq!(prepared["attempt_id"], "original");
        assert_eq!(prepared["bolt11"], bolt11.as_str());
    }
    // A preimage for a different payment never discharges the obligation.
    assert!(client
        .confirm("q", "operator:op", "original", &hex::encode([9u8; 32]))
        .await
        .is_err());
    let state = client.obligations().await.expect("list")[0]["state"].clone();
    assert!(
        state == "initiated" || state == "overdue",
        "still owed: {state}"
    );
    // The actual payment proof is verified against the original invoice.
    let preimage = hex::encode([3u8; 32]);
    let done = client
        .confirm("q", "operator:op", "original", &preimage)
        .await
        .expect("confirm");
    assert_eq!(done["state"], "verified");
    // Idempotent after restart-like repetition; still verified, nothing new to pay.
    assert_eq!(
        client
            .confirm("q", "operator:op", "original", &preimage)
            .await
            .expect("again")["state"],
        "verified"
    );
    assert_eq!(
        client.obligations().await.expect("list")[0]["state"],
        "verified"
    );
    assert!(
        client.prepare("q", "operator:op").await.is_err(),
        "verified refunds are not re-prepared"
    );
}

#[tokio::test]
async fn other_identities_cannot_see_prepare_or_confirm_the_operators_refund() {
    let operator = Keys::generate();
    let buyer = Keys::generate();
    let (_dir, storage, url) = server(&operator).await;
    seed_obligation(&storage, &operator, &buyer).await;
    original_invoice(&storage, &invoice(3)).await;
    for outsider in [Keys::generate(), buyer] {
        let client = OperatorRefundClient::new(&url, outsider).expect("client");
        assert!(client.obligations().await.expect("list").is_empty());
        assert!(client.prepare("q", "operator:op").await.is_err());
        assert!(client
            .confirm("q", "operator:op", "original", &hex::encode([3u8; 32]))
            .await
            .is_err());
    }
    // Malformed inputs are rejected locally, before any request.
    let client = OperatorRefundClient::new(&url, operator).expect("client");
    assert!(client.prepare("q/../x", "operator:op").await.is_err());
    assert!(client
        .confirm("q", "operator:op", "original", "zz")
        .await
        .is_err());
    assert!(OperatorRefundClient::new("ftp://x", Keys::generate()).is_err());
}
