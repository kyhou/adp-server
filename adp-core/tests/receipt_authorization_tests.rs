use adp_core::{
    build_authorization_revocation_event_builder, build_authorization_root_event_builder,
    build_receipt_event_builder, parse_authorization_event, parse_receipt_event,
    resolve_authorization, resolve_receipt_chain, AdpListing, AuthorizationTerms,
    BuildReceiptParams, ResolvedAuthorization,
};
use nostr::{Event, EventBuilder, EventId, Keys, Kind, PublicKey, Tag, Timestamp};

fn tag(values: Vec<&str>) -> Tag {
    Tag::parse(values).expect("valid tag")
}

fn game_coordinate(developer: &Keys, resource: &str) -> String {
    format!("30402:{}:{resource}", developer.public_key())
}

#[allow(clippy::too_many_arguments)]
fn signed_receipt(
    signer: &Keys,
    buyer: &Keys,
    coordinate: &str,
    status: &str,
    predecessor: Option<EventId>,
    authorization_event: Option<EventId>,
    created_at: u64,
    extra_tags: Vec<Tag>,
) -> Event {
    let mut tags = vec![
        tag(vec!["order", "order-1"]),
        tag(vec!["p", &buyer.public_key().to_hex()]),
        tag(vec!["a", coordinate]),
        tag(vec!["status", status]),
    ];
    if let Some(predecessor) = predecessor {
        tags.push(tag(vec!["e", &predecessor.to_hex()]));
    }
    if let Some(authorization_event) = authorization_event {
        tags.push(tag(vec![
            "authorization_event",
            &authorization_event.to_hex(),
        ]));
    }
    tags.extend(extra_tags);

    EventBuilder::new(Kind::Custom(1020), "encrypted")
        .tags(tags)
        .custom_created_at(Timestamp::from_secs(created_at))
        .sign_with_keys(signer)
        .expect("sign receipt")
}

fn authorization(
    developer: &Keys,
    fulfillment: &Keys,
    coordinate: &str,
    valid_from: u64,
    valid_until: Option<u64>,
    revoked_at: Option<u64>,
) -> ResolvedAuthorization {
    let terms = AuthorizationTerms {
        authorization_id: "authorization-1".into(),
        coordinate: coordinate.into(),
        fulfillment_pubkey: fulfillment.public_key(),
        valid_from,
        valid_until,
    };
    let root = build_authorization_root_event_builder(&terms, "authorization-root")
        .unwrap()
        .allow_self_tagging()
        .custom_created_at(Timestamp::from_secs(80))
        .sign_with_keys(developer)
        .unwrap();
    let mut events = vec![parse_authorization_event(&root).unwrap()];
    if let Some(revoked_at) = revoked_at {
        let revoke =
            build_authorization_revocation_event_builder(&terms, "authorization-revoke", root.id)
                .unwrap()
                .allow_self_tagging()
                .custom_created_at(Timestamp::from_secs(revoked_at))
                .sign_with_keys(developer)
                .unwrap();
        events.push(parse_authorization_event(&revoke).unwrap());
    }
    resolve_authorization(root.id, &events).unwrap()
}

fn assert_rejected(
    events: &[Event],
    developer: PublicKey,
    authorization: Option<&ResolvedAuthorization>,
) {
    assert!(resolve_receipt_chain(events, developer, authorization).is_err());
}

#[test]
fn direct_receipt_without_anchor_is_valid_and_coordinate_d_may_contain_colons() {
    let developer = Keys::generate();
    let buyer = Keys::generate();
    let coordinate = game_coordinate(&developer, "game:linux:x86_64");
    let receipt = signed_receipt(
        &developer,
        &buyer,
        &coordinate,
        "paid",
        None,
        None,
        100,
        vec![],
    );

    let events = [receipt.clone()];
    let tip = resolve_receipt_chain(&events, developer.public_key(), None).unwrap();
    assert_eq!(tip.id, receipt.id);
    assert_eq!(
        parse_receipt_event(&receipt)
            .unwrap()
            .authorization_event_id,
        None
    );
}

#[test]
fn relay_hinted_coordinate_is_accepted_by_parser_and_chain() {
    let developer = Keys::generate();
    let buyer = Keys::generate();
    let coordinate = game_coordinate(&developer, "game-1");
    let receipt = EventBuilder::new(Kind::Custom(1020), "encrypted")
        .tags(vec![
            tag(vec!["order", "order-1"]),
            tag(vec!["p", &buyer.public_key().to_hex()]),
            tag(vec!["a", &coordinate, "wss://relay.example"]),
            tag(vec!["status", "paid"]),
        ])
        .custom_created_at(Timestamp::from_secs(100))
        .sign_with_keys(&developer)
        .unwrap();

    assert_eq!(
        parse_receipt_event(&receipt).unwrap().coordinate,
        coordinate
    );
    assert!(resolve_receipt_chain(&[receipt], developer.public_key(), None).is_ok());
}

#[test]
fn omitted_status_defaults_to_paid_for_parser_and_chain() {
    let developer = Keys::generate();
    let buyer = Keys::generate();
    let coordinate = game_coordinate(&developer, "game-1");
    let receipt = EventBuilder::new(Kind::Custom(1020), "encrypted")
        .tags(vec![
            tag(vec!["order", "order-1"]),
            tag(vec!["p", &buyer.public_key().to_hex()]),
            tag(vec!["a", &coordinate]),
        ])
        .custom_created_at(Timestamp::from_secs(100))
        .sign_with_keys(&developer)
        .unwrap();

    assert_eq!(parse_receipt_event(&receipt).unwrap().status, "paid");
    assert!(resolve_receipt_chain(&[receipt], developer.public_key(), None).is_ok());
}

#[test]
fn direct_receipt_with_valid_self_authorization_anchor_is_valid() {
    let developer = Keys::generate();
    let buyer = Keys::generate();
    let coordinate = game_coordinate(&developer, "game-1");
    let authorization = authorization(&developer, &developer, &coordinate, 90, Some(120), None);
    let receipt = signed_receipt(
        &developer,
        &buyer,
        &coordinate,
        "paid",
        None,
        Some(authorization.root_event_id),
        100,
        vec![],
    );

    assert!(
        resolve_receipt_chain(&[receipt], developer.public_key(), Some(&authorization)).is_ok()
    );
}

#[test]
fn direct_receipt_anchor_must_authorize_the_developer_at_issuance() {
    let developer = Keys::generate();
    let fulfillment = Keys::generate();
    let buyer = Keys::generate();
    let coordinate = game_coordinate(&developer, "game-1");

    for authorization in [
        authorization(&developer, &fulfillment, &coordinate, 90, None, None),
        authorization(&developer, &developer, &coordinate, 101, None, None),
        authorization(&developer, &developer, &coordinate, 90, Some(100), None),
        authorization(&developer, &developer, &coordinate, 90, None, Some(100)),
    ] {
        let receipt = signed_receipt(
            &developer,
            &buyer,
            &coordinate,
            "paid",
            None,
            Some(authorization.root_event_id),
            100,
            vec![],
        );
        assert_rejected(&[receipt], developer.public_key(), Some(&authorization));
    }
}

#[test]
fn parser_rejects_wrong_kind_invalid_signature_and_bad_required_cardinality() {
    let developer = Keys::generate();
    let buyer = Keys::generate();
    let coordinate = game_coordinate(&developer, "game-1");
    let wrong_kind = EventBuilder::new(Kind::Custom(1021), "")
        .tags(vec![])
        .sign_with_keys(&developer)
        .unwrap();
    assert!(parse_receipt_event(&wrong_kind).is_err());

    for duplicate in ["order", "p", "a", "status"] {
        let value = match duplicate {
            "order" => "order-2".to_string(),
            "p" => buyer.public_key().to_hex(),
            "a" => coordinate.clone(),
            _ => "paid".to_string(),
        };
        let receipt = signed_receipt(
            &developer,
            &buyer,
            &coordinate,
            "paid",
            None,
            None,
            100,
            vec![tag(vec![duplicate, &value])],
        );
        assert!(
            parse_receipt_event(&receipt).is_err(),
            "accepted duplicate {duplicate}"
        );
    }

    let mut invalid_signature = signed_receipt(
        &developer,
        &buyer,
        &coordinate,
        "paid",
        None,
        None,
        100,
        vec![],
    );
    invalid_signature.content = "tampered".into();
    assert!(parse_receipt_event(&invalid_signature).is_err());
}

#[test]
fn parser_rejects_malformed_or_duplicate_authorization_anchor() {
    let developer = Keys::generate();
    let buyer = Keys::generate();
    let coordinate = game_coordinate(&developer, "game-1");
    let malformed = signed_receipt(
        &developer,
        &buyer,
        &coordinate,
        "paid",
        None,
        None,
        100,
        vec![tag(vec!["authorization_event", "not-an-event-id"])],
    );
    assert!(parse_receipt_event(&malformed).is_err());

    let anchor = EventId::all_zeros().to_hex();
    let duplicate = signed_receipt(
        &developer,
        &buyer,
        &coordinate,
        "paid",
        None,
        None,
        100,
        vec![
            tag(vec!["authorization_event", &anchor]),
            tag(vec!["authorization_event", &anchor]),
        ],
    );
    assert!(parse_receipt_event(&duplicate).is_err());
}

#[test]
fn delegated_receipt_requires_exactly_one_anchor() {
    let developer = Keys::generate();
    let fulfillment = Keys::generate();
    let buyer = Keys::generate();
    let coordinate = game_coordinate(&developer, "game-1");
    let missing = signed_receipt(
        &fulfillment,
        &buyer,
        &coordinate,
        "paid",
        None,
        None,
        100,
        vec![],
    );
    assert_rejected(&[missing], developer.public_key(), None);

    let anchor = EventId::all_zeros().to_hex();
    let duplicate = signed_receipt(
        &fulfillment,
        &buyer,
        &coordinate,
        "paid",
        None,
        None,
        100,
        vec![
            tag(vec!["authorization_event", &anchor]),
            tag(vec!["authorization_event", &anchor]),
        ],
    );
    assert_rejected(&[duplicate], developer.public_key(), None);
}

#[test]
fn delegated_receipt_accepts_active_matching_authorization() {
    let developer = Keys::generate();
    let fulfillment = Keys::generate();
    let buyer = Keys::generate();
    let coordinate = game_coordinate(&developer, "game-1");
    let authorization = authorization(&developer, &fulfillment, &coordinate, 90, Some(120), None);
    let receipt = signed_receipt(
        &fulfillment,
        &buyer,
        &coordinate,
        "paid",
        None,
        Some(authorization.root_event_id),
        100,
        vec![],
    );

    assert!(
        resolve_receipt_chain(&[receipt], developer.public_key(), Some(&authorization)).is_ok()
    );
}

#[test]
fn authorization_must_match_root_developer_key_coordinate_and_signed_state() {
    let developer = Keys::generate();
    let other_developer = Keys::generate();
    let fulfillment = Keys::generate();
    let other_fulfillment = Keys::generate();
    let buyer = Keys::generate();
    let coordinate = game_coordinate(&developer, "game-1");
    let matching = authorization(&developer, &fulfillment, &coordinate, 90, Some(120), None);
    let receipt = signed_receipt(
        &fulfillment,
        &buyer,
        &coordinate,
        "paid",
        None,
        Some(matching.root_event_id),
        100,
        vec![],
    );

    let wrong_developer_coordinate = game_coordinate(&other_developer, "game-1");
    let wrong_developer = authorization(
        &other_developer,
        &fulfillment,
        &wrong_developer_coordinate,
        90,
        Some(120),
        None,
    );
    assert_rejected(
        std::slice::from_ref(&receipt),
        developer.public_key(),
        Some(&wrong_developer),
    );

    let wrong_key = authorization(
        &developer,
        &other_fulfillment,
        &coordinate,
        90,
        Some(120),
        None,
    );
    assert_rejected(
        std::slice::from_ref(&receipt),
        developer.public_key(),
        Some(&wrong_key),
    );

    let wrong_coordinate = authorization(
        &developer,
        &fulfillment,
        &game_coordinate(&developer, "game-2"),
        90,
        Some(120),
        None,
    );
    assert_rejected(
        std::slice::from_ref(&receipt),
        developer.public_key(),
        Some(&wrong_coordinate),
    );

    let wrong_root = authorization(&developer, &fulfillment, &coordinate, 90, Some(121), None);
    assert_rejected(
        std::slice::from_ref(&receipt),
        developer.public_key(),
        Some(&wrong_root),
    );

    let mut detached = matching.clone();
    detached.terms.valid_from = 0;
    assert_rejected(&[receipt], developer.public_key(), Some(&detached));
}

#[test]
fn authorization_interval_and_revocation_boundaries_are_half_open() {
    let developer = Keys::generate();
    let fulfillment = Keys::generate();
    let buyer = Keys::generate();
    let coordinate = game_coordinate(&developer, "game-1");

    for (at, valid_from, valid_until, revoked_at, valid) in [
        (99, 100, Some(120), None, false),
        (100, 100, Some(120), None, true),
        (119, 100, Some(120), None, true),
        (120, 100, Some(120), None, false),
        (109, 100, None, Some(110), true),
        (110, 100, None, Some(110), false),
    ] {
        let authorization = authorization(
            &developer,
            &fulfillment,
            &coordinate,
            valid_from,
            valid_until,
            revoked_at,
        );
        let receipt = signed_receipt(
            &fulfillment,
            &buyer,
            &coordinate,
            "paid",
            None,
            Some(authorization.root_event_id),
            at,
            vec![],
        );
        assert_eq!(
            resolve_receipt_chain(&[receipt], developer.public_key(), Some(&authorization)).is_ok(),
            valid,
            "unexpected result at {at}"
        );
    }
}

#[test]
fn valid_receipt_before_later_revocation_remains_valid() {
    let developer = Keys::generate();
    let fulfillment = Keys::generate();
    let buyer = Keys::generate();
    let coordinate = game_coordinate(&developer, "game-1");
    let authorization = authorization(&developer, &fulfillment, &coordinate, 90, None, Some(110));
    let receipt = signed_receipt(
        &fulfillment,
        &buyer,
        &coordinate,
        "paid",
        None,
        Some(authorization.root_event_id),
        100,
        vec![],
    );

    assert!(
        resolve_receipt_chain(&[receipt], developer.public_key(), Some(&authorization)).is_ok()
    );
}

#[test]
fn chain_preserves_invariants_and_requires_strict_timestamp_order() {
    let developer = Keys::generate();
    let buyer = Keys::generate();
    let coordinate = game_coordinate(&developer, "game-1");
    let root = signed_receipt(
        &developer,
        &buyer,
        &coordinate,
        "paid",
        None,
        None,
        100,
        vec![],
    );

    let mutations = [
        ("order", "order-2".to_string()),
        ("p", Keys::generate().public_key().to_hex()),
        ("a", game_coordinate(&developer, "game-2")),
        ("authorization_event", EventId::all_zeros().to_hex()),
    ];
    for (name, value) in mutations {
        let update = signed_receipt(
            &developer,
            &buyer,
            &coordinate,
            "fulfilled",
            Some(root.id),
            None,
            110,
            vec![tag(vec![name, &value])],
        );
        assert_rejected(&[root.clone(), update], developer.public_key(), None);
    }

    for at in [99, 100] {
        let update = signed_receipt(
            &developer,
            &buyer,
            &coordinate,
            "fulfilled",
            Some(root.id),
            None,
            at,
            vec![],
        );
        assert_rejected(&[root.clone(), update], developer.public_key(), None);
    }
}

#[test]
fn chain_rejects_fork_missing_predecessor_disconnected_roots_and_tampered_cycle() {
    let developer = Keys::generate();
    let buyer = Keys::generate();
    let coordinate = game_coordinate(&developer, "game-1");
    let root = signed_receipt(
        &developer,
        &buyer,
        &coordinate,
        "paid",
        None,
        None,
        100,
        vec![],
    );
    let first = signed_receipt(
        &developer,
        &buyer,
        &coordinate,
        "fulfilled",
        Some(root.id),
        None,
        110,
        vec![],
    );
    let fork = signed_receipt(
        &developer,
        &buyer,
        &coordinate,
        "disputed",
        Some(root.id),
        None,
        120,
        vec![],
    );
    assert_rejected(&[root.clone(), first, fork], developer.public_key(), None);

    let missing = signed_receipt(
        &developer,
        &buyer,
        &coordinate,
        "fulfilled",
        Some(EventId::all_zeros()),
        None,
        110,
        vec![],
    );
    assert_rejected(&[root.clone(), missing], developer.public_key(), None);

    let other_root = signed_receipt(
        &developer,
        &buyer,
        &coordinate,
        "paid",
        None,
        None,
        101,
        vec![],
    );
    assert_rejected(&[root.clone(), other_root], developer.public_key(), None);

    let mut tampered_cycle = root.clone();
    tampered_cycle.tags.push(tag(vec!["e", &root.id.to_hex()]));
    assert_rejected(&[tampered_cycle], developer.public_key(), None);
}

#[test]
fn duplicate_root_from_multiple_relays_is_deduplicated() {
    let developer = Keys::generate();
    let buyer = Keys::generate();
    let coordinate = game_coordinate(&developer, "game-1");
    let root = signed_receipt(
        &developer,
        &buyer,
        &coordinate,
        "paid",
        None,
        None,
        100,
        vec![],
    );

    let events = [root.clone(), root.clone()];
    let tip = resolve_receipt_chain(&events, developer.public_key(), None).unwrap();
    assert_eq!(tip.id, root.id);
}

#[test]
fn duplicate_update_from_multiple_relays_is_deduplicated() {
    let developer = Keys::generate();
    let buyer = Keys::generate();
    let coordinate = game_coordinate(&developer, "game-1");
    let root = signed_receipt(
        &developer,
        &buyer,
        &coordinate,
        "paid",
        None,
        None,
        100,
        vec![],
    );
    let update = signed_receipt(
        &developer,
        &buyer,
        &coordinate,
        "fulfilled",
        Some(root.id),
        None,
        110,
        vec![],
    );

    let events = [root, update.clone(), update.clone()];
    let tip = resolve_receipt_chain(&events, developer.public_key(), None).unwrap();
    assert_eq!(tip.id, update.id);
}

#[test]
fn canonical_authorization_is_reused_for_multi_event_chain_and_detached_state_is_rejected() {
    let developer = Keys::generate();
    let fulfillment = Keys::generate();
    let buyer = Keys::generate();
    let coordinate = game_coordinate(&developer, "game-1");
    let authorization = authorization(&developer, &fulfillment, &coordinate, 90, Some(130), None);
    let root = signed_receipt(
        &fulfillment,
        &buyer,
        &coordinate,
        "paid",
        None,
        Some(authorization.root_event_id),
        100,
        vec![],
    );
    let update = signed_receipt(
        &fulfillment,
        &buyer,
        &coordinate,
        "fulfilled",
        Some(root.id),
        Some(authorization.root_event_id),
        110,
        vec![],
    );
    let events = [root, update];

    assert!(resolve_receipt_chain(&events, developer.public_key(), Some(&authorization)).is_ok());

    let mut detached = authorization;
    detached.terms.valid_until = Some(105);
    assert_rejected(&events, developer.public_key(), Some(&detached));
}

#[test]
fn receipt_resolver_resolves_authorization_once_then_uses_trusted_checks() {
    let source = include_str!("../src/receipt.rs");
    let resolver = source
        .split_once("pub fn resolve_receipt_chain")
        .unwrap()
        .1
        .split_once("pub fn latest_status_in_chain")
        .unwrap()
        .0;

    assert_eq!(resolver.matches("resolve_authorization(").count(), 1);
    assert!(resolver.contains(".authorizes_trusted("));
    assert!(!resolver.contains(".authorizes("));
}

#[test]
fn every_chain_event_is_authorized_independently() {
    let developer = Keys::generate();
    let fulfillment = Keys::generate();
    let second_fulfillment = Keys::generate();
    let buyer = Keys::generate();
    let coordinate = game_coordinate(&developer, "game-1");
    let authorization = authorization(&developer, &fulfillment, &coordinate, 90, Some(130), None);
    let root = signed_receipt(
        &fulfillment,
        &buyer,
        &coordinate,
        "paid",
        None,
        Some(authorization.root_event_id),
        100,
        vec![],
    );

    let delegated_update = signed_receipt(
        &fulfillment,
        &buyer,
        &coordinate,
        "fulfilled",
        Some(root.id),
        Some(authorization.root_event_id),
        110,
        vec![],
    );
    assert!(resolve_receipt_chain(
        &[root.clone(), delegated_update],
        developer.public_key(),
        Some(&authorization)
    )
    .is_ok());

    let developer_update = signed_receipt(
        &developer,
        &buyer,
        &coordinate,
        "refunded",
        Some(root.id),
        Some(authorization.root_event_id),
        110,
        vec![],
    );
    assert!(resolve_receipt_chain(
        &[root.clone(), developer_update],
        developer.public_key(),
        Some(&authorization)
    )
    .is_ok());

    let wrong_update = signed_receipt(
        &second_fulfillment,
        &buyer,
        &coordinate,
        "fulfilled",
        Some(root.id),
        Some(authorization.root_event_id),
        110,
        vec![],
    );
    assert_rejected(
        &[root, wrong_update],
        developer.public_key(),
        Some(&authorization),
    );
}

#[test]
fn delegated_update_must_still_be_active_at_its_own_timestamp() {
    let developer = Keys::generate();
    let fulfillment = Keys::generate();
    let buyer = Keys::generate();
    let coordinate = game_coordinate(&developer, "game-1");
    let authorization = authorization(&developer, &fulfillment, &coordinate, 90, Some(110), None);
    let root = signed_receipt(
        &fulfillment,
        &buyer,
        &coordinate,
        "paid",
        None,
        Some(authorization.root_event_id),
        100,
        vec![],
    );
    let update = signed_receipt(
        &fulfillment,
        &buyer,
        &coordinate,
        "fulfilled",
        Some(root.id),
        Some(authorization.root_event_id),
        110,
        vec![],
    );

    assert_rejected(
        &[root, update],
        developer.public_key(),
        Some(&authorization),
    );
}

#[test]
fn root_must_be_paid_known_statuses_are_enforced_and_refund_is_terminal() {
    let developer = Keys::generate();
    let buyer = Keys::generate();
    let coordinate = game_coordinate(&developer, "game-1");
    for status in ["fulfilled", "refunded", "disputed", "free", "shipped"] {
        let root = signed_receipt(
            &developer,
            &buyer,
            &coordinate,
            status,
            None,
            None,
            100,
            vec![],
        );
        assert_rejected(&[root], developer.public_key(), None);
    }

    let root = signed_receipt(
        &developer,
        &buyer,
        &coordinate,
        "paid",
        None,
        None,
        100,
        vec![],
    );
    let refunded = signed_receipt(
        &developer,
        &buyer,
        &coordinate,
        "refunded",
        Some(root.id),
        None,
        110,
        vec![],
    );
    let after_refund = signed_receipt(
        &developer,
        &buyer,
        &coordinate,
        "disputed",
        Some(refunded.id),
        None,
        120,
        vec![],
    );
    assert_rejected(
        &[root, refunded, after_refund],
        developer.public_key(),
        None,
    );
}

#[test]
fn payment_proof_e_tag_and_self_contained_tags_are_unchanged() {
    let developer = Keys::generate();
    let buyer = Keys::generate();
    let coordinate = game_coordinate(&developer, "game-1");
    let zap = EventId::all_zeros();
    let receipt = signed_receipt(
        &developer,
        &buyer,
        &coordinate,
        "paid",
        Some(zap),
        None,
        100,
        vec![
            tag(vec!["bolt11", "lnbc1invoice"]),
            tag(vec!["preimage", "00"]),
        ],
    );

    let parsed = parse_receipt_event(&receipt).unwrap();
    assert_eq!(parsed.e_tag, Some(zap.to_hex()));
    assert!(resolve_receipt_chain(&[receipt], developer.public_key(), None).is_ok());
}

#[test]
fn builder_emits_one_optional_anchor_without_changing_payment_tags() {
    let developer = Keys::generate();
    let buyer = Keys::generate();
    let listing_event = EventBuilder::new(Kind::Custom(30402), "")
        .tags(vec![tag(vec!["d", "game-1"])])
        .sign_with_keys(&developer)
        .unwrap();
    let listing = AdpListing::from_event(listing_event).unwrap();
    let anchor = EventId::all_zeros();
    let zap = EventId::all_zeros().to_hex();
    let params = BuildReceiptParams {
        listing: &listing,
        buyer_pubkey: buyer.public_key(),
        payment_proof_event_id: Some(zap.clone()),
        bolt11: Some("lnbc1invoice".into()),
        preimage: Some("00".into()),
        authorization_event_id: Some(anchor),
        amount: "1000".into(),
        currency: "SATS".into(),
    };
    let event = build_receipt_event_builder("order-1", &params, "encrypted".into())
        .unwrap()
        .sign_with_keys(&developer)
        .unwrap();

    let parsed = parse_receipt_event(&event).unwrap();
    assert_eq!(parsed.authorization_event_id, Some(anchor));
    assert_eq!(parsed.e_tag, Some(zap));
    for expected in ["bolt11", "preimage"] {
        assert_eq!(
            event
                .tags
                .iter()
                .filter(|tag| {
                    (*tag)
                        .clone()
                        .to_vec()
                        .first()
                        .is_some_and(|name| name == expected)
                })
                .count(),
            1
        );
    }
}
