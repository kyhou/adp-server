// Included in download_authorization_tests to reuse its actual-router fixture.
fn aggregate_root(
    developer: &Keys,
    buyer: &Keys,
    sale: &Event,
    proof_type: &str,
    proof: &str,
) -> Event {
    let listing = adp_core::AdpListing::from_event(sale.clone()).expect("sale listing");
    let legs = [adp_core::AggregateProofLeg {
        leg_id: "developer".into(),
        role: adp_core::PaymentRole::Developer,
        recipient_pubkey: developer.public_key().to_hex(),
        amount_msat: 100000,
        payment_hash: "11".repeat(32),
        proof_type: proof_type.into(),
        proof_reference: proof.into(),
        accepted_terms_id: None,
    }];
    adp_core::build_aggregate_receipt_event_builder(
        "portable-order",
        &adp_core::BuildAggregateReceiptParams {
            listing: &listing,
            buyer_pubkey: buyer.public_key(),
            authorization: None,
            quote_event_id: EventId::from_hex(&"22".repeat(32)).expect("quote ID"),
            processor_pubkey: developer.public_key(),
            processor_url: None,
            gross_msat: 100000,
            settled_at: now(),
            legs: &legs,
        },
        "encrypted fixture".into(),
    )
    .expect("receipt builder")
    .sign_with_keys(developer)
    .expect("receipt")
}

#[tokio::test]
async fn gate7_aggregate_receipt_download_needs_no_local_purchase_or_token() {
    let developer = Keys::generate();
    let buyer = Keys::generate();
    let coordinate = format!("30402:{}:game-1", developer.public_key());
    let hash = hex::encode(Sha256::digest(b"game"));
    let sale = listing(&developer, &hash, None, false);
    let receipt = aggregate_root(
        &developer,
        &buyer,
        &sale,
        "bolt11-preimage",
        &"11".repeat(32),
    );
    let router = app(
        &developer,
        sale.clone(),
        vec![sale],
        vec![],
        absent(),
        RelayLookup::Found(vec![receipt]),
    )
    .await;
    assert_eq!(download(router, &buyer, &coordinate).await, StatusCode::OK);
}

#[tokio::test]
async fn gate7_missing_aggregate_public_zap_cannot_grant_download() {
    let developer = Keys::generate();
    let buyer = Keys::generate();
    let coordinate = format!("30402:{}:game-1", developer.public_key());
    let hash = hex::encode(Sha256::digest(b"game"));
    let sale = listing(&developer, &hash, None, false);
    let receipt = aggregate_root(&developer, &buyer, &sale, "zap", &"33".repeat(32));
    let router = app(
        &developer,
        sale.clone(),
        vec![sale],
        vec![],
        absent(),
        RelayLookup::Found(vec![receipt]),
    )
    .await;
    assert_eq!(
        download(router, &buyer, &coordinate).await,
        StatusCode::SERVICE_UNAVAILABLE,
        "missing public proof is unavailable, never ownership"
    );
}

fn raw_auth(keys: &Keys, method: &str, url: &str) -> String {
    let event = EventBuilder::new(Kind::Custom(27235), "")
        .tags([tag(vec!["u", url]), tag(vec!["method", method])])
        .sign_with_keys(keys)
        .expect("auth event");
    format!(
        "Nostr {}",
        base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(&event).expect("JSON"))
    )
}

fn signed_tags(event: &Event, signer: &Keys, changed: Vec<Tag>) -> Event {
    EventBuilder::new(event.kind, event.content.clone())
        .tags(changed)
        .custom_created_at(event.created_at)
        .sign_with_keys(signer)
        .expect("signed fixture revision")
}

fn receipt_status(root: &Event, signer: &Keys, status: &str, at: u64) -> Event {
    let mut tags = root
        .tags
        .iter()
        .filter(|t| {
            matches!(
                t.as_slice()[0].as_str(),
                "order" | "p" | "a" | "authorization"
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    tags.extend([
        tag(vec!["e", &root.id.to_hex()]),
        tag(vec!["status", status]),
    ]);
    EventBuilder::new(Kind::Custom(1020), root.content.clone())
        .tags(tags)
        .custom_created_at(Timestamp::from_secs(at))
        .sign_with_keys(signer)
        .expect("status update")
}

#[tokio::test]
async fn gate7_delegated_portable_receipts_preserve_history_authority_and_status() {
    let developer = Keys::generate();
    let issuer = Keys::generate();
    let buyer = Keys::generate();
    let coordinate = format!("30402:{}:game-1", developer.public_key());
    let sale = listing(
        &developer,
        &hex::encode(Sha256::digest(b"game")),
        None,
        false,
    );
    for case in [
        "historical",
        "at-cancellation",
        "missing-authority",
        "wrong-capability",
        "wrong-coordinate",
        "wrong-key",
        "fulfilled",
        "refunded",
        "disputed",
        "fork",
    ] {
        let terms = adp_core::AuthorizationTerms {
            authorization_id: "portable-auth".into(),
            coordinate: if case == "wrong-coordinate" {
                format!("30402:{}:other", developer.public_key())
            } else {
                coordinate.clone()
            },
            operator_pubkey: Keys::generate().public_key(),
            fulfillment_pubkey: issuer.public_key(),
            capabilities: std::collections::BTreeSet::from([if case == "wrong-capability" {
                adp_core::CAPABILITY_ISSUE_GRANT
            } else {
                adp_core::CAPABILITY_ISSUE_RECEIPT
            }
            .into()]),
            valid_from: now() - 60,
        };
        let authority = adp_core::build_authorization_root_event_builder(&terms)
            .expect("root")
            .custom_created_at(Timestamp::from_secs(now() - 60))
            .sign_with_keys(&developer)
            .expect("authority");
        let cancelled =
            adp_core::build_authorization_cancellation_event_builder(&terms, authority.id)
                .expect("cancel")
                .custom_created_at(Timestamp::from_secs(now() - 10))
                .sign_with_keys(&developer)
                .expect("cancellation");
        let at = if case == "at-cancellation" {
            cancelled.created_at.as_secs()
        } else {
            now() - 20
        };
        let original = aggregate_root(
            &developer,
            &buyer,
            &sale,
            "bolt11-preimage",
            &"11".repeat(32),
        );
        let mut tags = original.tags.to_vec();
        tags.retain(|t| t.as_slice()[0] != "settled_at");
        tags.extend([
            tag(vec!["settled_at", &at.to_string()]),
            tag(vec!["authorization", &authority.id.to_hex()]),
        ]);
        let actual_issuer = if case == "wrong-key" {
            Keys::generate()
        } else {
            issuer.clone()
        };
        let root = EventBuilder::new(
            Kind::Custom(1020),
            adp_core::encrypt_receipt_content(&actual_issuer, &buyer.public_key(), "{}")
                .expect("NIP44"),
        )
        .tags(tags)
        .custom_created_at(Timestamp::from_secs(at))
        .sign_with_keys(&actual_issuer)
        .expect("root receipt");
        let mut chain = vec![root.clone()];
        if matches!(case, "fulfilled" | "refunded" | "disputed" | "fork") {
            chain.push(receipt_status(
                &root,
                &developer,
                if case == "fork" { "fulfilled" } else { case },
                at + 1,
            ));
        }
        if case == "fork" {
            chain.push(receipt_status(&root, &issuer, "refunded", at + 2));
        }
        let mut evidence = vec![sale.clone()];
        if case != "missing-authority" {
            evidence.extend([authority, cancelled]);
        }
        let router = app(
            &developer,
            sale.clone(),
            evidence,
            vec![],
            absent(),
            RelayLookup::Found(chain),
        )
        .await;
        let expected = match case {
            "historical" | "fulfilled" => StatusCode::OK,
            "missing-authority" => StatusCode::SERVICE_UNAVAILABLE,
            _ => StatusCode::FORBIDDEN,
        };
        assert_eq!(
            download(router, &buyer, &coordinate).await,
            expected,
            "{case}"
        );
    }
}

#[tokio::test]
async fn gate7_same_portable_receipt_works_on_two_independent_authorized_servers() {
    use http_body_util::BodyExt;
    let developer = Keys::generate();
    let buyer = Keys::generate();
    let coordinate = format!("30402:{}:game-1", developer.public_key());
    let base = listing(
        &developer,
        &hex::encode(Sha256::digest(b"game")),
        None,
        false,
    );
    let mut tags = base.tags.clone().to_vec();
    tags.push(tag(vec!["server", "http://localhost:8081"]));
    let sale = signed_tags(&base, &developer, tags);
    let receipt = aggregate_root(
        &developer,
        &buyer,
        &sale,
        "bolt11-preimage",
        &"11".repeat(32),
    );
    for server in [
        "http://localhost:8080",
        "http://localhost:8081",
        "http://localhost:8082",
    ] {
        let router = app_at(
            server,
            &developer,
            sale.clone(),
            vec![sale.clone()],
            vec![],
            absent(),
            RelayLookup::Found(vec![receipt.clone()]),
        )
        .await;
        let target = format!("/game/{}", coordinate.replace(':', "%3A"));
        let response = router
            .oneshot(
                Request::builder()
                    .uri(&target)
                    .header(
                        "authorization",
                        raw_auth(&buyer, "GET", &format!("{server}{target}")),
                    )
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        if server.ends_with("8082") {
            assert_eq!(
                response.status(),
                StatusCode::UNAVAILABLE_FOR_LEGAL_REASONS,
                "ownership does not grant distribution"
            );
        } else {
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                response
                    .into_body()
                    .collect()
                    .await
                    .expect("bytes")
                    .to_bytes()
                    .as_ref(),
                b"game"
            );
        }
    }
}

fn public_zap(
    buyer: &Keys,
    recipient: PublicKey,
    lsp: &Keys,
    coordinate: &str,
    leg: &str,
    amount: u64,
    preimage: [u8; 32],
    quote: &str,
) -> Event {
    use bitcoin::hashes::{sha256, Hash};
    use bitcoin::secp256k1::{Secp256k1, SecretKey};
    let request = EventBuilder::new(Kind::Custom(9734), "")
        .tags([
            tag(vec!["p", &recipient.to_hex()]),
            tag(vec!["amount", &amount.to_string()]),
            tag(vec!["a", coordinate]),
            tag(vec!["sale_quote", quote]),
            tag(vec!["payment_leg", leg]),
            tag(vec!["relays", "ws://127.0.0.1:1"]),
        ])
        .custom_created_at(Timestamp::from_secs(now() - 4))
        .sign_with_keys(buyer)
        .expect("buyer request");
    let description = serde_json::to_string(&request).expect("description");
    let key = SecretKey::from_slice(&[7; 32]).expect("synthetic invoice key");
    let invoice = lightning_invoice::InvoiceBuilder::new(lightning_invoice::Currency::Regtest)
        .amount_milli_satoshis(amount)
        .description_hash(sha256::Hash::hash(description.as_bytes()))
        .payment_hash(sha256::Hash::hash(&preimage))
        .payment_secret(lightning_invoice::PaymentSecret([8; 32]))
        .duration_since_epoch(std::time::Duration::from_secs(now() - 6))
        .min_final_cltv_expiry_delta(144)
        .build_signed(|hash| Secp256k1::new().sign_ecdsa_recoverable(hash, &key))
        .expect("offline invoice")
        .to_string();
    EventBuilder::new(Kind::Custom(9735), "")
        .tags([
            tag(vec!["P", &buyer.public_key().to_hex()]),
            tag(vec!["p", &recipient.to_hex()]),
            tag(vec!["a", coordinate]),
            tag(vec!["description", &description]),
            tag(vec!["bolt11", &invoice]),
        ])
        .custom_created_at(Timestamp::from_secs(now() - 2))
        .sign_with_keys(lsp)
        .expect("LSP proof")
}

#[tokio::test]
async fn gate7_public_aggregate_legs_require_pinned_provenance_and_every_valid_proof() {
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let metadata = MockServer::start().await;
    let developer = Keys::generate();
    let operator = Keys::generate();
    let buyer = Keys::generate();
    let developer_lsp = Keys::generate();
    let operator_lsp = Keys::generate();
    for (name, lsp) in [
        ("dev", &developer_lsp),
        ("op", &operator_lsp),
        ("current", &buyer),
    ] {
        Mock::given(method("GET")).and(path(format!("/.well-known/lnurlp/{name}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "callback":format!("{}/never-invoice",metadata.uri()),"minSendable":1000,"maxSendable":100000,
                "nostrPubkey":lsp.public_key().to_hex(),"allowsNostr":true,
            }))).mount(&metadata).await;
    }
    Mock::given(method("GET"))
        .and(path("/never-invoice"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&metadata)
        .await;
    let terms = adp_core::build_terms_event_builder(&adp_core::TermsPayload::Offer {
        terms: adp_core::CommercialTerms {
            version: 1,
            operator_pubkey: operator.public_key().to_hex(),
            payment_destination: format!("op@{}", metadata.address()),
            fee: adp_core::FeePolicy::Fixed { amount_msat: 1000 },
            limitations: vec![],
        },
    })
    .expect("terms")
    .sign_with_keys(&operator)
    .expect("signed offer");
    let coordinate = format!("30402:{}:game-1", developer.public_key());
    let base = listing(
        &developer,
        &hex::encode(Sha256::digest(b"game")),
        None,
        false,
    );
    let mut tags = base.tags.clone().to_vec();
    tags.retain(|tag| tag.as_slice()[0] != "price");
    tags.extend([
        tag(vec!["price", "3", "SATS"]),
        tag(vec!["lud16", &format!("dev@{}", metadata.address())]),
        tag(vec![
            "adp_terms",
            &operator.public_key().to_hex(),
            &terms.id.to_hex(),
        ]),
    ]);
    let sale = signed_tags(&base, &developer, tags);
    let dev_zap = public_zap(
        &buyer,
        developer.public_key(),
        &developer_lsp,
        &coordinate,
        "developer",
        2000,
        [42; 32],
        "portable-order",
    );
    let op_zap = public_zap(
        &buyer,
        operator.public_key(),
        &operator_lsp,
        &coordinate,
        "operator",
        1000,
        [43; 32],
        "portable-order",
    );
    let original_legs = vec![
        adp_core::AggregateProofLeg {
            leg_id: "operator".into(),
            role: adp_core::PaymentRole::Operator,
            recipient_pubkey: operator.public_key().to_hex(),
            amount_msat: 1000,
            payment_hash: hex::encode(Sha256::digest([43; 32])),
            proof_type: "zap".into(),
            proof_reference: op_zap.id.to_hex(),
            accepted_terms_id: Some(terms.id.to_hex()),
        },
        adp_core::AggregateProofLeg {
            leg_id: "developer".into(),
            role: adp_core::PaymentRole::Developer,
            recipient_pubkey: developer.public_key().to_hex(),
            amount_msat: 2000,
            payment_hash: hex::encode(Sha256::digest([42; 32])),
            proof_type: "zap".into(),
            proof_reference: dev_zap.id.to_hex(),
            accepted_terms_id: None,
        },
    ];
    for case in [
        "valid",
        "valid-public-preimage",
        "missing-proof",
        "bad-signature",
        "wrong-lsp",
        "wrong-amount",
        "wrong-hash",
        "wrong-buyer",
        "wrong-recipient",
        "wrong-order",
        "wrong-leg",
        "wrong-description",
        "missing-description",
        "wrong-description-hash",
        "bad-request-signature",
        "future-proof",
        "duplicate-recipient",
        "bad-terms-signature",
        "wrong-public-preimage",
        "missing-listing",
        "foreign-listing",
        "missing-terms",
        "unaccepted-terms",
        "metadata-unavailable",
    ] {
        let mut dev = dev_zap.clone();
        let op = op_zap.clone();
        let mut sale_ref = sale.clone();
        let mut legs = original_legs.clone();
        let mut accepted = terms.clone();
        match case {
            "bad-signature" => dev.content = "tampered".into(),
            "wrong-lsp" => dev = signed_tags(&dev, &developer, dev.tags.clone().to_vec()),
            "wrong-amount" => {
                dev = public_zap(
                    &buyer,
                    developer.public_key(),
                    &developer_lsp,
                    &coordinate,
                    "developer",
                    5000,
                    [42; 32],
                    "portable-order",
                )
            }
            "wrong-hash" => {
                dev = public_zap(
                    &buyer,
                    developer.public_key(),
                    &developer_lsp,
                    &coordinate,
                    "developer",
                    2000,
                    [44; 32],
                    "portable-order",
                )
            }
            "wrong-buyer" => {
                dev = public_zap(
                    &operator,
                    developer.public_key(),
                    &developer_lsp,
                    &coordinate,
                    "developer",
                    2000,
                    [42; 32],
                    "portable-order",
                )
            }
            "wrong-recipient" => {
                dev = public_zap(
                    &buyer,
                    operator.public_key(),
                    &developer_lsp,
                    &coordinate,
                    "developer",
                    2000,
                    [42; 32],
                    "portable-order",
                )
            }
            "wrong-order" => {
                dev = public_zap(
                    &buyer,
                    developer.public_key(),
                    &developer_lsp,
                    &coordinate,
                    "developer",
                    2000,
                    [42; 32],
                    "other-order",
                )
            }
            "wrong-leg" => {
                dev = public_zap(
                    &buyer,
                    developer.public_key(),
                    &developer_lsp,
                    &coordinate,
                    "other-leg",
                    2000,
                    [42; 32],
                    "portable-order",
                )
            }
            "wrong-description" => {
                let mut tags = dev.tags.clone().to_vec();
                tags.retain(|t| t.as_slice()[0] != "description");
                tags.push(tag(vec!["description", "{}"]));
                dev = signed_tags(&dev, &developer_lsp, tags);
            }
            "missing-description" => {
                let tags = dev
                    .tags
                    .iter()
                    .filter(|t| t.as_slice()[0] != "description")
                    .cloned()
                    .collect();
                dev = signed_tags(&dev, &developer_lsp, tags);
            }
            "wrong-description-hash" | "bad-request-signature" => {
                let description = dev
                    .tags
                    .iter()
                    .find(|t| t.as_slice()[0] == "description")
                    .expect("description")
                    .as_slice()[1]
                    .clone();
                let mut request: Event = serde_json::from_str(&description).expect("request");
                if case == "bad-request-signature" {
                    request.content = "tampered".into();
                } else {
                    request = EventBuilder::new(Kind::Custom(9734), "different signed message")
                        .tags(request.tags.clone().to_vec())
                        .custom_created_at(request.created_at)
                        .sign_with_keys(&buyer)
                        .expect("valid changed request");
                }
                let mut tags = dev.tags.clone().to_vec();
                tags.retain(|t| t.as_slice()[0] != "description");
                tags.push(tag(vec![
                    "description",
                    &serde_json::to_string(&request).expect("JSON"),
                ]));
                dev = signed_tags(&dev, &developer_lsp, tags);
            }
            "future-proof" => {
                dev = EventBuilder::new(Kind::Custom(9735), dev.content.clone())
                    .tags(dev.tags.clone().to_vec())
                    .custom_created_at(Timestamp::from_secs(now() + 30))
                    .sign_with_keys(&developer_lsp)
                    .expect("future proof");
            }
            "duplicate-recipient" => {
                let mut tags = dev.tags.clone().to_vec();
                tags.push(tag(vec!["p", &operator.public_key().to_hex()]));
                dev = signed_tags(&dev, &developer_lsp, tags);
            }
            "bad-terms-signature" => {
                accepted.content.push(' ');
            }
            "wrong-public-preimage" => {
                let mut tags = dev.tags.clone().to_vec();
                tags.push(tag(vec!["preimage", &"ff".repeat(32)]));
                dev = signed_tags(&dev, &developer_lsp, tags);
            }
            "valid-public-preimage" => {
                let mut tags = dev.tags.clone().to_vec();
                tags.push(tag(vec!["preimage", &hex::encode([42; 32])]));
                dev = signed_tags(&dev, &developer_lsp, tags);
            }
            "foreign-listing" => {
                let mut tags = sale.tags.clone().to_vec();
                tags.retain(|t| t.as_slice()[0] != "d");
                tags.push(tag(vec!["d", "other-game"]));
                sale_ref = signed_tags(&sale, &developer, tags);
            }
            "unaccepted-terms" => {
                accepted = adp_core::build_terms_event_builder(&adp_core::TermsPayload::Offer {
                    terms: adp_core::CommercialTerms {
                        version: 1,
                        operator_pubkey: operator.public_key().to_hex(),
                        payment_destination: format!("current@{}", metadata.address()),
                        fee: adp_core::FeePolicy::None,
                        limitations: vec![],
                    },
                })
                .expect("offer")
                .sign_with_keys(&operator)
                .expect("signed offer");
                legs[0].accepted_terms_id = Some(accepted.id.to_hex());
            }
            "metadata-unavailable" => {
                let mut tags = sale.tags.clone().to_vec();
                tags.retain(|t| t.as_slice()[0] != "lud16");
                tags.push(tag(vec![
                    "lud16",
                    &format!("absent@{}", metadata.address()),
                ]));
                sale_ref = signed_tags(&sale, &developer, tags);
            }
            _ => {}
        }
        legs[1].proof_reference = dev.id.to_hex();
        legs[0].proof_reference = op.id.to_hex();
        let parsed = adp_core::AdpListing::from_event(sale_ref.clone()).expect("sale snapshot");
        let mut receipt = adp_core::build_aggregate_receipt_event_builder(
            "portable-order",
            &adp_core::BuildAggregateReceiptParams {
                listing: &parsed,
                buyer_pubkey: buyer.public_key(),
                authorization: None,
                quote_event_id: EventId::from_hex(&"22".repeat(32)).expect("quote"),
                processor_pubkey: operator.public_key(),
                processor_url: None,
                gross_msat: 3000,
                settled_at: now(),
                legs: &legs,
            },
            adp_core::encrypt_receipt_content(
                &developer,
                &buyer.public_key(),
                "{\"fixture\":true}",
            )
            .expect("NIP44"),
        )
        .expect("receipt")
        .sign_with_keys(&developer)
        .expect("signed root");
        if case == "foreign-listing" {
            let mut tags = receipt.tags.clone().to_vec();
            tags.retain(|t| t.as_slice()[0] != "a");
            tags.push(tag(vec!["a", &coordinate]));
            receipt = signed_tags(&receipt, &developer, tags);
        }
        let mut evidence = vec![];
        if case != "missing-proof" {
            evidence.push(dev);
        }
        evidence.push(op);
        if case != "missing-listing" {
            evidence.push(sale_ref);
        }
        if case != "missing-terms" {
            evidence.push(accepted);
        }
        // The current listing deliberately advertises a different LSP. Historical
        // sale_listing and exact accepted terms, never current metadata, bind proofs.
        let mut current_tags = sale.tags.clone().to_vec();
        current_tags.retain(|t| t.as_slice()[0] != "lud16");
        current_tags.push(tag(vec![
            "lud16",
            &format!("current@{}", metadata.address()),
        ]));
        let current = signed_tags(&sale, &developer, current_tags);
        let router = app(
            &developer,
            current,
            evidence,
            vec![],
            absent(),
            RelayLookup::Found(vec![receipt]),
        )
        .await;
        let expected = match case {
            "valid" | "valid-public-preimage" => StatusCode::OK,
            "missing-proof" | "missing-listing" | "missing-terms" | "metadata-unavailable" => {
                StatusCode::SERVICE_UNAVAILABLE
            }
            _ => StatusCode::FORBIDDEN,
        };
        assert_eq!(
            download(router, &buyer, &coordinate).await,
            expected,
            "{case}"
        );
    }
}

#[tokio::test]
async fn gate7_authenticated_routes_bind_raw_encoding_queries_and_configured_origin() {
    use serde_json::json;
    let developer = Keys::generate();
    let buyer = Keys::generate();
    let hash = hex::encode(Sha256::digest(b"game"));
    let sale = listing(&developer, &hash, None, false);
    let router = app(&developer, sale.clone(), vec![], vec![], absent(), absent()).await;
    let destination =
        json!({"endpoint":"buyer@example.test","min_sendable_msat":1000,"max_sendable_msat":10000});
    let cases = vec![
        ("POST", "/provision", json!({"scope":"isolated"})),
        (
            "POST",
            "/provision/revoke",
            json!({"fulfillment_pubkey":buyer.public_key().to_hex()}),
        ),
        ("POST", "/upload", json!({})),
        (
            "POST",
            "/purchase/confirm",
            json!({"game_coordinate":format!("30402:{}:game-1",developer.public_key()),"listing_event":sale,"bolt11":null,"preimage":null}),
        ),
        (
            "POST",
            "/entitlement/claim",
            json!({"game_coordinate":format!("30402:{}:game-1",developer.public_key()),"campaign_event_id":"invalid"}),
        ),
        (
            "POST",
            "/settlement/quote",
            json!({"purchase_id":"","listing_event":sale,"refund_destination":destination}),
        ),
        ("GET", "/settlement/missing%3Aquote", json!({})),
        ("POST", "/settlement/missing%3Aquote/status", json!({})),
        ("POST", "/settlement/missing%3Aquote/next", json!({})),
        (
            "POST",
            "/settlement/missing%3Aquote/proof",
            json!({"attempt_id":"attempt","preimage":"11"}),
        ),
        (
            "POST",
            "/settlement/missing%3Aquote/refund-destination",
            json!({"destination":destination}),
        ),
        ("POST", "/settlement/missing%3Aquote/recovery", json!({})),
        (
            "POST",
            "/settlement/missing%3Aquote/recovery/submit",
            json!({"event":sale,"plaintext":"synthetic"}),
        ),
        (
            "POST",
            "/settlement/missing%3Aquote/refund/leg/prepare",
            json!({}),
        ),
        (
            "POST",
            "/settlement/missing%3Aquote/refund/leg/confirm",
            json!({"attempt_id":"attempt","preimage":"11"}),
        ),
        ("POST", "/settlement/refunds", json!({})),
        ("POST", "/settlement/missing%3Aquote/refunds", json!({})),
    ];
    for (method, path, body) in cases {
        let target = format!("{path}?x=%2f&x=/&value=+");
        let url = format!("http://localhost:8080{target}");
        for signed in [
            url.clone(),
            format!("{url}/"),
            url.replacen('?', "/?", 1),
            url.replace("%2f", "%2F"),
            url.replace("value=+", "value=%20"),
            url.replace("localhost:8080", "forged.example"),
        ] {
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(&target)
                        .header("authorization", raw_auth(&buyer, method, &signed))
                        .header("host", "forged.example")
                        .header("x-forwarded-host", "forged.example")
                        .header("x-forwarded-proto", "https")
                        .header("content-type", "application/json")
                        .body(Body::from(body.to_string()))
                        .expect("request"),
                )
                .await
                .expect("response");
            if signed == url {
                assert_ne!(
                    response.status(),
                    StatusCode::UNAUTHORIZED,
                    "exact raw target on {path}"
                );
            } else {
                assert_eq!(
                    response.status(),
                    StatusCode::UNAUTHORIZED,
                    "mismatched URL on {path}: {signed}"
                );
            }
        }
        if method == "GET" {
            for signed_method in ["GET", "HEAD"] {
                let response = router
                    .clone()
                    .oneshot(
                        Request::builder()
                            .method("HEAD")
                            .uri(&target)
                            .header("authorization", raw_auth(&buyer, signed_method, &url))
                            .body(Body::empty())
                            .expect("HEAD request"),
                    )
                    .await
                    .expect("HEAD response");
                assert_eq!(
                    response.status() == StatusCode::UNAUTHORIZED,
                    signed_method == "GET",
                    "actual HTTP method is authenticated"
                );
            }
        }
    }
}
