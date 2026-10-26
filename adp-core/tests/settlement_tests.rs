use adp_core::*;
use nostr::Keys;

fn terms(operator: &str, fee: FeePolicy) -> CommercialTerms {
    CommercialTerms {
        version: 1,
        operator_pubkey: operator.into(),
        payment_destination: format!("{operator}@example.com"),
        fee,
        limitations: vec![],
    }
}

#[test]
fn fee_math_floors_clamps_and_rejects_invalid_values() {
    let fee = FeePolicy::BasisPoints {
        basis_points: 333,
        minimum_msat: Some(500),
        maximum_msat: Some(2_000),
    };
    assert_eq!(fee.amount_msat(10_001).expect("fee"), 500);
    assert_eq!(fee.amount_msat(100_000).expect("fee"), 2_000);
    assert_eq!(
        FeePolicy::Fixed { amount_msat: 0 }.amount_msat(1),
        Err(SettlementError::FixedFeeZero)
    );
    assert_eq!(
        FeePolicy::BasisPoints {
            basis_points: u64::MAX,
            minimum_msat: None,
            maximum_msat: None
        }
        .amount_msat(u64::MAX),
        Err(SettlementError::Overflow)
    );
}

#[test]
fn coverage_deduplicates_urls_by_operator_and_rejects_conflicts() {
    let servers = vec![
        ServerOperator {
            url: "https://a".into(),
            operator_pubkey: "op".into(),
        },
        ServerOperator {
            url: "https://b".into(),
            operator_pubkey: "op".into(),
        },
    ];
    let accepted = vec![AcceptedTermsRef {
        operator_pubkey: "op".into(),
        terms_event_id: "root".into(),
    }];
    assert_eq!(
        validate_terms_coverage(&servers, &accepted)
            .expect("coverage")
            .len(),
        1
    );
    assert!(validate_terms_coverage(
        &servers,
        &[
            accepted[0].clone(),
            AcceptedTermsRef {
                operator_pubkey: "op".into(),
                terms_event_id: "other".into()
            }
        ]
    )
    .is_err());
    assert!(validate_terms_coverage(&servers, &[]).is_err());
}

#[test]
fn split_preserves_gross_order_and_zero_has_no_leg() {
    let legs = build_payment_legs(
        10_000,
        "dev",
        "dev@example.com",
        &[
            (
                terms("z", FeePolicy::Fixed { amount_msat: 1_000 }),
                "tz".into(),
            ),
            (
                terms("a", FeePolicy::Fixed { amount_msat: 1_000 }),
                "ta".into(),
            ),
            (terms("free", FeePolicy::None), "tf".into()),
        ],
    )
    .expect("legs");
    assert_eq!(legs.iter().map(|leg| leg.amount_msat).sum::<u64>(), 10_000);
    assert_eq!(
        legs.iter()
            .map(|leg| leg.recipient_pubkey.as_str())
            .collect::<Vec<_>>(),
        vec!["a", "z", "dev"]
    );
    assert!(build_payment_legs(
        10_000,
        "dev",
        "dev@example.com",
        &[(
            terms(
                "op",
                FeePolicy::Fixed {
                    amount_msat: 10_000
                }
            ),
            "t".into()
        )]
    )
    .is_err());
}

#[test]
fn currency_is_exact_and_fiat_is_rejected() {
    assert_eq!(parse_gross_msat("2", "SATS").expect("sats"), 2_000);
    assert_eq!(parse_gross_msat("0.00000000001", "BTC").expect("btc"), 1);
    assert_eq!(
        parse_gross_msat("1.000000000001", "BTC"),
        Err(SettlementError::InvalidAmount)
    );
    assert_eq!(
        parse_gross_msat("5", "USD"),
        Err(SettlementError::UnsupportedCurrency)
    );
}

#[test]
fn quote_requires_every_exact_invoice_and_failure_time_is_fixed() {
    let destination = RefundDestination {
        endpoint: "buyer@example.com".into(),
        min_sendable_msat: 1,
        max_sendable_msat: 20_000,
    };
    let legs = build_payment_legs(
        10_000,
        "dev",
        "dev@example.com",
        &[(
            terms("op", FeePolicy::Fixed { amount_msat: 1_000 }),
            "terms".into(),
        )],
    )
    .expect("legs");
    let mut quote = SignedQuote::new(
        "q".into(),
        "buyer".into(),
        "listing".into(),
        "30402:dev:g".into(),
        "processor".into(),
        10_000,
        100,
        &destination,
        legs.clone(),
    )
    .expect("quote");
    assert!(quote.activate(101, &[]).is_err());
    let attempts = legs
        .iter()
        .map(|leg| InvoicePreflight {
            leg_id: leg.leg_id.clone(),
            recipient_pubkey: leg.recipient_pubkey.clone(),
            amount_msat: leg.amount_msat,
            payment_hash: "11".repeat(32),
            lnurl_origin: "https://example.com".into(),
            callback: "https://example.com/cb".into(),
            lsp_pubkey: "lsp".into(),
            invoice_expires_at: 200,
        })
        .collect::<Vec<_>>();
    quote.activate(101, &attempts).expect("activate");
    quote.fail(quote.payment_expires_at).expect("fail");
    assert_eq!(
        quote.refund_due_at().expect("deadline"),
        quote.payment_expires_at + REFUND_DUE_SECS
    );
    assert!(quote.fail(quote.payment_expires_at + 1).is_err());
}

#[test]
fn aggregate_rejects_replay_and_wrong_sum() {
    let leg = AggregateProofLeg {
        leg_id: "one".into(),
        role: PaymentRole::Developer,
        recipient_pubkey: "dev".into(),
        amount_msat: 1_000,
        payment_hash: "22".repeat(32),
        proof_type: "zap".into(),
        proof_reference: "event".into(),
        accepted_terms_id: None,
    };
    assert_eq!(
        aggregate_commitment(1_000, std::slice::from_ref(&leg))
            .expect("commitment")
            .len(),
        64
    );
    assert!(aggregate_commitment(2_000, std::slice::from_ref(&leg)).is_err());
    assert!(aggregate_commitment(
        2_000,
        &[
            leg.clone(),
            AggregateProofLeg {
                leg_id: "two".into(),
                ..leg
            }
        ]
    )
    .is_err());
}

#[test]
fn terms_and_withdrawal_are_role_bound() {
    let operator = Keys::generate();
    let stranger = Keys::generate();
    let offer = build_terms_event_builder(&TermsPayload::Offer {
        terms: terms(&operator.public_key().to_hex(), FeePolicy::None),
    })
    .expect("builder")
    .sign_with_keys(&operator)
    .expect("sign");
    assert!(matches!(
        parse_terms_event(&offer),
        Ok(TermsPayload::Offer { .. })
    ));

    let forged = build_terms_event_builder(&TermsPayload::Offer {
        terms: terms(&operator.public_key().to_hex(), FeePolicy::None),
    })
    .expect("builder")
    .sign_with_keys(&stranger)
    .expect("sign");
    assert!(parse_terms_event(&forged).is_err());

    let withdrawal = build_terms_event_builder(&TermsPayload::Withdraw {
        version: SETTLEMENT_VERSION,
        terms_event_id: offer.id.to_hex(),
    })
    .expect("builder")
    .sign_with_keys(&operator)
    .expect("sign");
    verify_terms_withdrawal(&withdrawal, &offer).expect("withdrawal");
}
