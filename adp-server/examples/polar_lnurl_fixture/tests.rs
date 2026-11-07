use super::*;
use axum::{
    body::{to_bytes, Body},
    http::Request,
};
use bitcoin::hashes::{sha256, Hash};
use lightning_invoice::{InvoiceBuilder, PaymentSecret};
use secp256k1::{Secp256k1, SecretKey};
use serde_json::Value;
use tower::ServiceExt;
use wiremock::{
    matchers::{body_json, header, method, path},
    Mock, MockServer, ResponseTemplate,
};

#[tokio::test]
async fn polar_ca_certificate_is_accepted_but_untrusted_and_wrong_host_are_rejected() {
    use std::io::{Read, Write};
    // Ephemeral synthetic credentials only; never read Polar files in automated tests.
    let directory = tempfile::tempdir().expect("TLS directory");
    let cert_path = directory.path().join("tls.cert");
    let key_path = directory.path().join("tls.key");
    let output = std::process::Command::new("openssl")
        .args([
            "req",
            "-x509",
            "-newkey",
            "ec",
            "-pkeyopt",
            "ec_paramgen_curve:P-256",
            "-nodes",
        ])
        .arg("-keyout")
        .arg(&key_path)
        .arg("-out")
        .arg(&cert_path)
        .args([
            "-days",
            "1",
            "-subj",
            "/CN=localhost",
            "-addext",
            "subjectAltName=DNS:localhost",
            "-addext",
            "basicConstraints=critical,CA:TRUE",
        ])
        .output()
        .expect("openssl is required for the synthetic TLS test");
    assert!(output.status.success(), "generate synthetic TLS identity");
    let certificate = std::fs::read(cert_path).expect("certificate");
    let identity =
        native_tls::Identity::from_pkcs8(&certificate, &std::fs::read(key_path).expect("key"))
            .expect("identity");
    for (trusted, hostname) in [
        (true, "localhost"),
        (false, "localhost"),
        (true, "127.0.0.1"),
    ] {
        let acceptor = native_tls::TlsAcceptor::new(identity.clone()).expect("acceptor");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listener");
        let port = listener.local_addr().expect("address").port();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("connection");
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            if let Ok(mut tls) = acceptor.accept(stream) {
                let mut request = [0; 4096];
                if tls.read(&mut request).unwrap_or(0) > 0 {
                    let _ = tls.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK",
                    );
                }
            }
        });
        let client = if trusted {
            lnd_client(&certificate).expect("client")
        } else {
            // Same backend, but without trusting the synthetic certificate.
            reqwest::Client::builder()
                .use_native_tls()
                .no_proxy()
                .timeout(Duration::from_secs(5))
                .build()
                .expect("client")
        };
        let response = client
            .get(format!("https://{hostname}:{port}/v1/invoices"))
            .send()
            .await;
        server.join().expect("server");
        if trusted && hostname == "localhost" {
            assert_eq!(
                response
                    .expect("Polar-style CA certificate accepted")
                    .status(),
                StatusCode::OK
            );
        } else {
            assert!(
                response.is_err(),
                "untrusted certificate or wrong hostname must fail"
            );
        }
    }
}

fn key(index: u8) -> SecretKey {
    SecretKey::from_slice(&[index; 32]).expect("test key")
}
fn node_key(index: u8) -> String {
    secp256k1::PublicKey::from_secret_key(&Secp256k1::new(), &key(index)).to_string()
}
fn invoice(index: u8, amount: u64, timestamp: u64, expiry: u64, metadata: &str) -> String {
    invoice_on_network(
        index,
        amount,
        timestamp,
        expiry,
        metadata,
        Currency::Regtest,
    )
}
fn invoice_on_network(
    index: u8,
    amount: u64,
    timestamp: u64,
    expiry: u64,
    metadata: &str,
    currency: Currency,
) -> String {
    InvoiceBuilder::new(currency)
        .amount_milli_satoshis(amount)
        .description_hash(sha256::Hash::hash(metadata.as_bytes()))
        .payment_hash(sha256::Hash::hash(&[index; 32]))
        .payment_secret(PaymentSecret([index; 32]))
        .duration_since_epoch(Duration::from_secs(timestamp))
        .expiry_time(Duration::from_secs(expiry))
        .min_final_cltv_expiry_delta(40)
        .build_signed(|message| Secp256k1::new().sign_ecdsa_recoverable(message, &key(index)))
        .expect("test invoice")
        .to_string()
}
fn recipient(url: &str, index: u8) -> Recipient {
    Recipient {
        client: reqwest::Client::builder()
            .timeout(Duration::from_millis(100))
            .build()
            .expect("client"),
        invoice_url: reqwest::Url::parse(url)
            .expect("url")
            .join("/v1/invoices")
            .expect("path"),
        macaroon: reqwest::header::HeaderValue::from_static("000102"),
        node_pubkey: node_key(index),
        nostr_pubkey: nostr_sdk::Keys::generate().public_key().to_hex(),
        min: 1000,
        max: 100_000,
        expiry: 600,
    }
}
async fn request(app: Router, uri: &str) -> (StatusCode, Value) {
    let response = app
        .oneshot(
            Request::builder()
                .uri(uri)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 100_000).await.expect("body");
    (status, serde_json::from_slice(&bytes).expect("JSON"))
}
fn fixture(recipients: BTreeMap<String, Recipient>) -> Fixture {
    Fixture {
        origin: "http://127.0.0.1:18080".into(),
        recipients,
    }
}

#[tokio::test]
async fn metadata_and_all_four_callbacks_use_correct_nodes() {
    let mut recipients = BTreeMap::new();
    let mut servers = Vec::new();
    for (index, name) in ["alice", "bob", "carol", "dave"].into_iter().enumerate() {
        let index = index as u8 + 1;
        let server = MockServer::start().await;
        let metadata = metadata_string(name);
        use base64::Engine;
        use sha2::Digest;
        let hash = base64::engine::general_purpose::STANDARD
            .encode(sha2::Sha256::digest(metadata.as_bytes()));
        Mock::given(method("POST"))
            .and(path("/v1/invoices"))
            .and(header("Grpc-Metadata-macaroon", "000102"))
            .and(body_json(
                json!({"value_msat":"12001", "expiry":"600", "description_hash":hash}),
            ))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(
                    json!({"payment_request":invoice(index,12001,now(),600,&metadata)}),
                ),
            )
            .expect(1)
            .mount(&server)
            .await;
        recipients.insert(name.into(), recipient(&server.uri(), index));
        servers.push(server);
    }
    let app = router(fixture(recipients));
    for name in ["alice", "bob", "carol", "dave"] {
        let (status, body) = request(app.clone(), &format!("/.well-known/lnurlp/{name}")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["tag"], "payRequest");
        assert_eq!(body["minSendable"], 1000);
        assert_eq!(body["maxSendable"], 100_000);
        assert_eq!(body["allowsNostr"], false);
        nostr_sdk::PublicKey::from_hex(body["nostrPubkey"].as_str().expect("pubkey"))
            .expect("valid key");
        assert_eq!(body["metadata"], metadata_string(name));
        assert_eq!(
            body["callback"],
            format!("http://127.0.0.1:18080/lnurlp/{name}/callback")
        );
        let uri = format!("/lnurlp/{name}/callback?amount=12001");
        let (status, body) = request(app.clone(), &uri).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["routes"], json!([]));
        let decoded =
            Bolt11Invoice::from_str(body["pr"].as_str().expect("invoice")).expect("valid BOLT11");
        assert_eq!(decoded.amount_milli_satoshis(), Some(12001));
        assert_eq!(decoded.expiry_time().as_secs(), 600);
        let index = ["alice", "bob", "carol", "dave"]
            .iter()
            .position(|n| *n == name)
            .expect("recipient") as u8
            + 1;
        assert_eq!(decoded.recover_payee_pub_key().to_string(), node_key(index));
    }
}

#[test]
fn invalid_amounts_and_bounds() {
    for query in [
        None,
        Some(""),
        Some("amount=0"),
        Some("amount=-1"),
        Some("amount=1.2"),
        Some("amount=+1000"),
        Some("amount=999"),
        Some("amount=100001"),
        Some("amount=18446744073709551616"),
        Some("amount=1000&amount=1000"),
        Some("amount=1000&nostr=x"),
        Some("amount=%GG"),
        Some("other=1000"),
    ] {
        assert_eq!(parse_amount(query, 1000, 100_000), None, "{query:?}");
    }
    assert_eq!(parse_amount(Some("amount=1000"), 1000, 100_000), Some(1000));
    assert_eq!(
        parse_amount(Some("amount=100000"), 1000, 100_000),
        Some(100_000)
    );
}

#[tokio::test]
async fn malformed_requests_and_upstream_errors_are_sanitized() {
    let server = MockServer::start().await;
    let app = router(fixture(BTreeMap::from([(
        "alice".into(),
        recipient(&server.uri(), 1),
    )])));
    for uri in [
        "/lnurlp/alice/callback",
        "/lnurlp/alice/callback?amount=nope",
    ] {
        assert_eq!(request(app.clone(), uri).await.0, StatusCode::BAD_REQUEST);
    }
    assert!(server
        .received_requests()
        .await
        .expect("requests")
        .is_empty());
    assert_eq!(
        request(app.clone(), "/.well-known/lnurlp/unknown").await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(app.clone(), "/lnurlp/unknown/callback?amount=1000")
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(503).set_body_string("secret upstream details"))
        .mount(&server)
        .await;
    let (status, body) = request(app, "/lnurlp/alice/callback?amount=1000").await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(
        body,
        json!({"status":"ERROR","reason":"LND rejected invoice request"})
    );
}

#[tokio::test]
async fn unavailable_or_malformed_lnd_response() {
    let server = MockServer::start().await;
    let app = router(fixture(BTreeMap::from([(
        "alice".into(),
        recipient(&server.uri(), 1),
    )])));
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"wrong":"response"})))
        .mount(&server)
        .await;
    assert_eq!(
        request(app.clone(), "/lnurlp/alice/callback?amount=1000")
            .await
            .0,
        StatusCode::BAD_GATEWAY
    );
    server.reset().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(1)))
        .mount(&server)
        .await;
    let (_, body) = request(app, "/lnurlp/alice/callback?amount=1000").await;
    assert_eq!(body["reason"], "LND invoice service unavailable");
}

#[test]
fn rejects_wrong_node_amount_expiry_network_or_metadata() {
    let recipient = recipient("http://localhost:1", 1);
    let metadata = metadata_string("alice");
    assert!(validate_invoice(
        &invoice(1, 1000, now(), 600, &metadata),
        &recipient,
        1000,
        &metadata
    )
    .is_ok());
    for invalid in [
        invoice(2, 1000, now(), 600, &metadata),
        invoice(1, 1001, now(), 600, &metadata),
        invoice(1, 1000, now(), 3601, &metadata),
        invoice(1, 1000, now() - 1000, 600, &metadata),
        invoice(1, 1000, now() + 100, 600, &metadata),
        invoice(1, 1000, now(), 600, "wrong metadata"),
        "not-an-invoice".into(),
    ] {
        assert!(validate_invoice(&invalid, &recipient, 1000, &metadata).is_err());
    }
    assert!(validate_invoice(
        &invoice_on_network(1, 1000, now(), 600, &metadata, Currency::Bitcoin),
        &recipient,
        1000,
        &metadata
    )
    .is_err());
}

#[tokio::test]
async fn invalid_upstream_invoices_return_sanitized_errors() {
    let server = MockServer::start().await;
    let metadata = metadata_string("alice");
    let app = router(fixture(BTreeMap::from([(
        "alice".into(),
        recipient(&server.uri(), 1),
    )])));
    for invalid in [
        "not-an-invoice".into(),
        invoice(2, 12001, now(), 600, &metadata),
        invoice(1, 12000, now(), 600, &metadata),
        invoice(1, 12001, now(), 601, &metadata),
        invoice(1, 12001, now() - 1000, 600, &metadata),
        invoice(1, 12001, now(), 600, "wrong metadata"),
        invoice_on_network(1, 12001, now(), 600, &metadata, Currency::Bitcoin),
    ] {
        server.reset().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"payment_request":invalid})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let (status, body) = request(app.clone(), "/lnurlp/alice/callback?amount=12001").await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(
            body,
            json!({"status":"ERROR", "reason":"LND invoice failed validation"})
        );
    }
}

#[tokio::test]
async fn compatible_with_existing_adp_exact_invoice_preflight() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/invoices"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"payment_request":invoice(1,12001,now(),600,&metadata_string("alice"))}),
        ))
        .expect(1)
        .mount(&server)
        .await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let address = listener.local_addr().expect("address");
    let app = router(Fixture {
        origin: format!("http://{address}"),
        recipients: BTreeMap::from([("alice".into(), recipient(&server.uri(), 1))]),
    });
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    let buyer = nostr_sdk::Keys::generate();
    let result = adp_server::lnurl::request_exact_invoice(
        &format!("alice@{address}"),
        12001,
        buyer.public_key(),
        buyer.public_key(),
        None,
        None,
        now() + 86400,
        true,
    )
    .await;
    task.abort();
    let exact = result.expect("existing ADP preflight accepts fixture");
    assert_eq!(exact.provenance.min_sendable_msat, 1000);
    assert_eq!(
        Bolt11Invoice::from_str(&exact.bolt11)
            .expect("invoice")
            .recover_payee_pub_key()
            .to_string(),
        node_key(1)
    );
}

#[test]
fn config_refuses_public_bind_plaintext_lnd_and_invalid_limits() {
    let text = include_str!("config.example.json");
    let mut config: Config = serde_json::from_str(text).expect("example config");
    validate_config(&config).expect("valid example");
    config.bind = "0.0.0.0:18080".parse().expect("address");
    assert!(validate_config(&config).is_err());
    config.bind = default_bind();
    config
        .recipients
        .get_mut("alice")
        .expect("alice")
        .lnd_rest_url = "http://localhost:8081".into();
    assert!(validate_config(&config).is_err());
    config
        .recipients
        .get_mut("alice")
        .expect("alice")
        .lnd_rest_url = "https://localhost:8081".into();
    config
        .recipients
        .get_mut("alice")
        .expect("alice")
        .invoice_expiry_secs = 86400;
    assert!(validate_config(&config).is_err());
}
