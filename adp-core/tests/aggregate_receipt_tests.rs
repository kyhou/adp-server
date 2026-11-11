use adp_core::{
    build_aggregate_receipt_event_builder, parse_receipt_event, resolve_receipt_chain, AdpListing,
    AggregateProofLeg, BuildAggregateReceiptParams, PaymentRole,
};
use nostr::{EventBuilder, EventId, Keys, Kind, Tag};

fn tag(values: &[&str]) -> Tag {
    Tag::parse(
        values
            .iter()
            .map(|value| (*value).to_string())
            .collect::<Vec<_>>(),
    )
    .expect("tag")
}

#[test]
fn aggregate_paid_root_resolves_without_legacy_singular_fields() {
    let developer = Keys::generate();
    let buyer = Keys::generate();
    let listing = listing(&developer);
    let legs = [AggregateProofLeg {
        leg_id: "developer".into(),
        role: PaymentRole::Developer,
        recipient_pubkey: developer.public_key().to_hex(),
        amount_msat: 1000,
        payment_hash: "11".repeat(32),
        proof_type: "bolt11-preimage".into(),
        proof_reference: "11".repeat(32),
        accepted_terms_id: None,
    }];
    let event = build_aggregate_receipt_event_builder(
        "portable-order",
        &BuildAggregateReceiptParams {
            listing: &listing,
            buyer_pubkey: buyer.public_key(),
            authorization: None,
            quote_event_id: EventId::from_hex(&"22".repeat(32)).expect("quote"),
            processor_pubkey: developer.public_key(),
            processor_url: None,
            gross_msat: 1000,
            settled_at: 123,
            legs: &legs,
        },
        "encrypted fixture".into(),
    )
    .expect("receipt")
    .sign_with_keys(&developer)
    .expect("signed receipt");
    parse_receipt_event(&event).expect("valid aggregate profile");
    let events = [event];
    assert_eq!(
        resolve_receipt_chain(&events, developer.public_key(), None)
            .expect("portable aggregate ownership")
            .id,
        events[0].id
    );
}

fn listing(developer: &Keys) -> AdpListing {
    let event = EventBuilder::new(Kind::Custom(30402), "")
        .tags([tag(&["d", "game"]), tag(&["price", "1", "SATS"])])
        .sign_with_keys(developer)
        .expect("listing");
    AdpListing::from_event(event).expect("parse listing")
}

fn portable_root(
    developer: &Keys,
    buyer: &Keys,
    issuer: &Keys,
    authorization: Option<EventId>,
    at: u64,
) -> nostr::Event {
    let listing = listing(developer);
    let legs = [AggregateProofLeg {
        leg_id: "developer".into(),
        role: PaymentRole::Developer,
        recipient_pubkey: developer.public_key().to_hex(),
        amount_msat: 1000,
        payment_hash: "11".repeat(32),
        proof_type: "bolt11-preimage".into(),
        proof_reference: "11".repeat(32),
        accepted_terms_id: None,
    }];
    build_aggregate_receipt_event_builder(
        "portable",
        &BuildAggregateReceiptParams {
            listing: &listing,
            buyer_pubkey: buyer.public_key(),
            authorization,
            quote_event_id: EventId::from_hex(&"22".repeat(32)).expect("quote"),
            processor_pubkey: developer.public_key(),
            processor_url: None,
            gross_msat: 1000,
            settled_at: at,
            legs: &legs,
        },
        "ciphertext".into(),
    )
    .expect("builder")
    .custom_created_at(nostr::Timestamp::from_secs(at))
    .sign_with_keys(issuer)
    .expect("root")
}

fn status(
    root: &nostr::Event,
    previous: EventId,
    signer: &Keys,
    state: &str,
    at: u64,
) -> nostr::Event {
    let mut tags = root
        .tags
        .iter()
        .filter(|tag| {
            matches!(
                tag.as_slice().first().map(String::as_str),
                Some("order" | "p" | "a" | "authorization")
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    tags.extend([tag(&["e", &previous.to_hex()]), tag(&["status", state])]);
    EventBuilder::new(Kind::Custom(1020), "ciphertext")
        .tags(tags)
        .custom_created_at(nostr::Timestamp::from_secs(at))
        .sign_with_keys(signer)
        .expect("status")
}

#[test]
fn aggregate_delegation_retains_capabilities_coordinate_and_historical_cancellation() {
    let developer = Keys::generate();
    let buyer = Keys::generate();
    let issuer = Keys::generate();
    for capability in [
        adp_core::CAPABILITY_ISSUE_RECEIPT,
        adp_core::CAPABILITY_ISSUE_GRANT,
    ] {
        let terms = adp_core::AuthorizationTerms {
            authorization_id: "portable-authority".into(),
            coordinate: listing(&developer).coordinate(),
            operator_pubkey: Keys::generate().public_key(),
            fulfillment_pubkey: issuer.public_key(),
            capabilities: std::collections::BTreeSet::from([capability.into()]),
            valid_from: 90,
        };
        let authority = adp_core::build_authorization_root_event_builder(&terms)
            .expect("authority")
            .custom_created_at(nostr::Timestamp::from_secs(90))
            .sign_with_keys(&developer)
            .expect("root");
        let cancelled =
            adp_core::build_authorization_cancellation_event_builder(&terms, authority.id)
                .expect("cancel")
                .custom_created_at(nostr::Timestamp::from_secs(110))
                .sign_with_keys(&developer)
                .expect("cancellation");
        let resolved =
            adp_core::resolve_authorization_at(authority.id, &[authority.clone(), cancelled], 200)
                .expect("chain");
        for at in [89, 100, 110] {
            let root = portable_root(&developer, &buyer, &issuer, Some(authority.id), at);
            if at == 110 {
                let mut tags = root.tags.clone().to_vec();
                tags.retain(|tag| tag.as_slice()[0] != "settled_at");
                tags.push(tag(&["settled_at", "100"]));
                let late = EventBuilder::new(Kind::Custom(1020), root.content.clone())
                    .tags(tags)
                    .custom_created_at(root.created_at)
                    .sign_with_keys(&issuer)
                    .expect("late issuance");
                assert!(
                    resolve_receipt_chain(&[late], developer.public_key(), Some(&resolved))
                        .is_err(),
                    "backdated settlement does not authorize late issuance"
                );
            }
            assert_eq!(
                resolve_receipt_chain(&[root.clone()], developer.public_key(), Some(&resolved))
                    .is_ok(),
                at == 100 && capability == adp_core::CAPABILITY_ISSUE_RECEIPT
            );
            assert!(resolve_receipt_chain(&[root], developer.public_key(), None).is_err());
        }
        let root = portable_root(&developer, &buyer, &developer, Some(authority.id), 100);
        assert!(
            resolve_receipt_chain(&[root], developer.public_key(), Some(&resolved)).is_err(),
            "direct issuer must omit delegated anchor"
        );
        let root = portable_root(&developer, &buyer, &issuer, Some(EventId::all_zeros()), 100);
        assert!(
            resolve_receipt_chain(&[root], developer.public_key(), Some(&resolved)).is_err(),
            "exact immutable anchor"
        );
    }
}

#[test]
fn aggregate_status_chains_reject_forks_mutations_and_unreachable_members() {
    let developer = Keys::generate();
    let buyer = Keys::generate();
    let root = portable_root(&developer, &buyer, &developer, None, 100);
    let fulfilled = status(&root, root.id, &developer, "fulfilled", 110);
    let refunded = status(&root, fulfilled.id, &developer, "refunded", 120);
    let valid = [
        root.clone(),
        fulfilled.clone(),
        refunded.clone(),
        refunded.clone(),
    ];
    assert_eq!(
        resolve_receipt_chain(&valid, developer.public_key(), None)
            .expect("refund tip")
            .id,
        refunded.id
    );
    for bad in [
        status(&root, root.id, &developer, "disputed", 111),
        status(&root, EventId::all_zeros(), &developer, "refunded", 130),
        status(&root, refunded.id, &developer, "fulfilled", 130),
        status(&root, fulfilled.id, &Keys::generate(), "refunded", 130),
    ] {
        assert!(resolve_receipt_chain(
            &[root.clone(), fulfilled.clone(), refunded.clone(), bad],
            developer.public_key(),
            None
        )
        .is_err());
    }
    for name in ["order", "p", "a", "authorization"] {
        let mut tags = fulfilled.tags.clone().to_vec();
        tags.retain(|tag| tag.as_slice().first().map(String::as_str) != Some(name));
        tags.push(tag(&[
            name,
            if name == "a" {
                "30402:invalid:other"
            } else {
                "changed"
            },
        ]));
        let altered = EventBuilder::new(Kind::Custom(1020), "ciphertext")
            .tags(tags)
            .custom_created_at(nostr::Timestamp::from_secs(110))
            .sign_with_keys(&developer)
            .expect("altered");
        assert!(
            resolve_receipt_chain(&[root.clone(), altered], developer.public_key(), None).is_err(),
            "changed {name}"
        );
    }
    let mut bad_signature = fulfilled;
    bad_signature.content = "tampered".into();
    assert!(
        resolve_receipt_chain(&[root.clone(), bad_signature], developer.public_key(), None)
            .is_err()
    );
    let aggregate_successor = portable_root(&developer, &buyer, &developer, None, 110);
    assert!(
        resolve_receipt_chain(&[root, aggregate_successor], developer.public_key(), None).is_err(),
        "two aggregate roots"
    );
}

#[test]
fn aggregate_public_reference_and_commitment_mutations_are_rejected() {
    let developer = Keys::generate();
    let buyer = Keys::generate();
    let root = portable_root(&developer, &buyer, &developer, None, 100);
    for (field, value) in [
        (6, "unknown".into()),
        (7, "not-an-event-id".into()),
        (7, "AA".repeat(32)),
        (7, "22".repeat(32)),
        (4, "0".into()),
        (5, "FF".repeat(32)),
    ] {
        let tags = root
            .tags
            .iter()
            .map(|existing| {
                let mut values = existing.as_slice().to_vec();
                if values[0] == "payment_leg" {
                    values[field] = value.clone();
                }
                Tag::parse(values).expect("tag")
            })
            .collect::<Vec<_>>();
        let bad = EventBuilder::new(Kind::Custom(1020), "ciphertext")
            .tags(tags)
            .sign_with_keys(&developer)
            .expect("mutated");
        assert!(
            parse_receipt_event(&bad).is_err(),
            "invalid aggregate leg field {field}"
        );
    }
}

#[test]
fn aggregate_receipt_round_trips_and_rejects_singular_tags() {
    let developer = Keys::generate();
    let buyer = Keys::generate();
    let processor = Keys::generate();
    let signer = Keys::generate();
    let listing = listing(&developer);
    let legs = [AggregateProofLeg {
        leg_id: "developer".into(),
        role: PaymentRole::Developer,
        recipient_pubkey: developer.public_key().to_hex(),
        amount_msat: 1_000,
        payment_hash: "11".repeat(32),
        proof_type: "bolt11-preimage".into(),
        proof_reference: "11".repeat(32),
        accepted_terms_id: None,
    }];
    let params = BuildAggregateReceiptParams {
        listing: &listing,
        buyer_pubkey: buyer.public_key(),
        authorization: None,
        quote_event_id: EventId::from_hex(&"22".repeat(32)).expect("event id"),
        processor_pubkey: processor.public_key(),
        processor_url: Some("https://processor.example"),
        gross_msat: 1_000,
        settled_at: 123,
        legs: &legs,
    };
    let event = build_aggregate_receipt_event_builder("order", &params, "ciphertext".into())
        .expect("builder")
        .sign_with_keys(&signer)
        .expect("receipt");
    let parsed = parse_receipt_event(&event).expect("aggregate receipt");
    assert_eq!(parsed.aggregate.expect("aggregate").legs, legs);

    let mut tags = event.tags.to_vec();
    tags.push(tag(&["amount_msat", "1000"]));
    let mixed = EventBuilder::new(Kind::Custom(1020), "ciphertext")
        .tags(tags)
        .sign_with_keys(&signer)
        .expect("mixed receipt");
    assert!(parse_receipt_event(&mixed).is_err());
}
