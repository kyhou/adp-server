use adp_server::lnurl::{request_exact_invoice, resolve_lud16_lsp_pubkey_with_base};
use bitcoin::hashes::{sha256, Hash};
use lightning_invoice::{Currency, InvoiceBuilder, PaymentSecret};
use secp256k1::{Secp256k1, SecretKey};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn resolves_lud16_to_nostr_pubkey() {
    let server = MockServer::start().await;
    let lsp = nostr_sdk::Keys::generate();
    Mock::given(method("GET"))
        .and(path("/.well-known/lnurlp/studio"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "nostrPubkey": lsp.public_key().to_hex()
        })))
        .mount(&server)
        .await;

    let resolved = resolve_lud16_lsp_pubkey_with_base("studio@example.com", &server.uri())
        .await
        .expect("resolve lud16");

    assert_eq!(resolved, lsp.public_key());
}

#[tokio::test]
async fn rejects_malformed_lud16() {
    let server = MockServer::start().await;

    assert!(
        resolve_lud16_lsp_pubkey_with_base("not-an-address", &server.uri())
            .await
            .is_err()
    );
}

fn invoice(amount_msat: u64, issued_at: u64) -> String {
    let secp = Secp256k1::new();
    let private_key = SecretKey::from_slice(&[3; 32]).expect("secret key");
    InvoiceBuilder::new(Currency::Bitcoin)
        .amount_milli_satoshis(amount_msat)
        .description("ADP settlement leg".into())
        .payment_hash(sha256::Hash::hash(&[4; 32]))
        .payment_secret(PaymentSecret([5; 32]))
        .duration_since_epoch(std::time::Duration::from_secs(issued_at))
        .expiry_time(std::time::Duration::from_secs(600))
        .min_final_cltv_expiry_delta(144)
        .build_signed(|hash| secp.sign_ecdsa_recoverable(hash, &private_key))
        .expect("invoice")
        .to_string()
}

#[tokio::test]
async fn requests_and_validates_exact_invoice() {
    let server = MockServer::start().await;
    let lsp = nostr_sdk::Keys::generate();
    let buyer = nostr_sdk::Keys::generate();
    let recipient = nostr_sdk::Keys::generate();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs();
    Mock::given(method("GET"))
        .and(path("/pay"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "callback": format!("{}/invoice", server.uri()),
            "minSendable": 1000,
            "maxSendable": 5000,
            "nostrPubkey": lsp.public_key().to_hex(),
            "allowsNostr": false
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/invoice"))
        .and(query_param("amount", "2000"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "pr": invoice(2000, now)
        })))
        .mount(&server)
        .await;

    let exact = request_exact_invoice(
        &format!("{}/pay", server.uri()),
        2000,
        buyer.public_key(),
        recipient.public_key(),
        None,
        None,
        now + 3600,
        true,
    )
    .await
    .expect("exact invoice");

    assert_eq!(exact.provenance.lsp_pubkey, lsp.public_key().to_hex());
    assert_eq!(exact.expires_at, now + 600);
}

#[tokio::test]
async fn rejects_out_of_bounds_before_requesting_invoice() {
    let server = MockServer::start().await;
    let lsp = nostr_sdk::Keys::generate();
    Mock::given(method("GET"))
        .and(path("/pay"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "callback": format!("{}/invoice", server.uri()),
            "minSendable": 2000,
            "maxSendable": 5000,
            "nostrPubkey": lsp.public_key().to_hex()
        })))
        .mount(&server)
        .await;

    let keys = nostr_sdk::Keys::generate();
    let result = request_exact_invoice(
        &format!("{}/pay", server.uri()),
        1000,
        keys.public_key(),
        keys.public_key(),
        None,
        None,
        u64::MAX,
        true,
    )
    .await;

    assert!(result.is_err());
    assert_eq!(server.received_requests().await.expect("requests").len(), 1);
}
