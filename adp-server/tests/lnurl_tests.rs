use adp_server::lnurl::resolve_lud16_lsp_pubkey_with_base;
use wiremock::matchers::{method, path};
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
