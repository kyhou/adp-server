use adp_core::{
    build_acceptance_event_builder, build_attestation_event_builder, latest_status_in_chain,
    parse_acceptance_event, parse_attestation_event, parse_receipt_event, verify_zap_receipt,
    AdpListing,
};
use nostr::{EventBuilder, Keys, Kind, Tag, Timestamp};

fn tag(values: Vec<&str>) -> Tag {
    Tag::parse(values).expect("valid tag")
}

fn signed_event(keys: &Keys, kind: u16, tags: Vec<Tag>, created_at: u64) -> nostr::Event {
    EventBuilder::new(Kind::Custom(kind), "")
        .tags(tags)
        .custom_created_at(Timestamp::from(created_at))
        .sign_with_keys(keys)
        .expect("sign event")
}

#[test]
fn listing_rejects_malformed_fulfillment_valid_from() {
    let developer = Keys::generate();
    let fulfillment = Keys::generate();
    let event = signed_event(
        &developer,
        30402,
        vec![
            tag(vec!["d", "game-1"]),
            tag(vec![
                "fulfillment_pubkey",
                &fulfillment.public_key().to_hex(),
                "not-a-timestamp",
                "",
            ]),
        ],
        100,
    );

    assert!(AdpListing::from_event(event).is_err());
}

#[test]
fn listing_parses_nip99_price_tag() {
    let developer = Keys::generate();
    let event = signed_event(
        &developer,
        30402,
        vec![
            tag(vec!["d", "game-1"]),
            tag(vec!["price", "21000", "SATS"]),
        ],
        100,
    );

    let listing = AdpListing::from_event(event).expect("parse listing");

    assert_eq!(
        listing.price,
        Some(("21000".to_string(), "SATS".to_string()))
    );
}

#[test]
fn listing_parses_lud16_tag() {
    let developer = Keys::generate();
    let event = signed_event(
        &developer,
        30402,
        vec![
            tag(vec!["d", "game-1"]),
            tag(vec!["lud16", "studio@example.com"]),
        ],
        100,
    );

    let listing = AdpListing::from_event(event).expect("parse listing");

    assert_eq!(listing.lud16.as_deref(), Some("studio@example.com"));
}

#[test]
fn receipt_missing_status_is_rejected() {
    let signer = Keys::generate();
    let buyer = Keys::generate();
    let event = signed_event(
        &signer,
        1020,
        vec![
            tag(vec!["order", "order-1"]),
            tag(vec!["p", &buyer.public_key().to_hex()]),
            tag(vec!["a", "30402:developer:game-1"]),
            tag(vec!["amount", "1000"]),
            tag(vec!["currency", "SATS"]),
        ],
        100,
    );

    assert!(parse_receipt_event(&event).is_err());
}

#[test]
fn ambiguous_status_chain_returns_no_authoritative_tip() {
    let signer = Keys::generate();
    let buyer = Keys::generate();
    let root = signed_event(
        &signer,
        1020,
        vec![
            tag(vec!["order", "order-1"]),
            tag(vec!["p", &buyer.public_key().to_hex()]),
            tag(vec!["a", "30402:developer:game-1"]),
            tag(vec!["status", "paid"]),
        ],
        100,
    );
    let root_id = root.id.to_hex();
    let shipped = signed_event(
        &signer,
        1020,
        vec![
            tag(vec!["order", "order-1"]),
            tag(vec!["p", &buyer.public_key().to_hex()]),
            tag(vec!["a", "30402:developer:game-1"]),
            tag(vec!["status", "shipped"]),
            tag(vec!["e", &root_id]),
        ],
        110,
    );
    let refunded = signed_event(
        &signer,
        1020,
        vec![
            tag(vec!["order", "order-1"]),
            tag(vec!["p", &buyer.public_key().to_hex()]),
            tag(vec!["a", "30402:developer:game-1"]),
            tag(vec!["status", "refunded"]),
            tag(vec!["e", &root_id]),
        ],
        120,
    );

    assert!(latest_status_in_chain(&[root, shipped, refunded]).is_none());
}

#[test]
fn partial_zap_validation_accepts_buyer_uppercase_p_tag() {
    let lsp = Keys::generate();
    let buyer = Keys::generate();
    let coordinate = "30402:developer:game-1";
    let zap = signed_event(
        &lsp,
        9735,
        vec![
            tag(vec!["P", &buyer.public_key().to_hex()]),
            tag(vec!["a", coordinate]),
            tag(vec!["amount", "1000"]),
        ],
        100,
    );

    assert!(verify_zap_receipt(
        &zap,
        &buyer.public_key().to_hex(),
        coordinate,
        &lsp.public_key()
    )
    .is_ok());
}

#[test]
fn zap_validation_rejects_lsp_pubkey_mismatch() {
    let actual_lsp = Keys::generate();
    let declared_lsp = Keys::generate();
    let buyer = Keys::generate();
    let coordinate = "30402:developer:game-1";
    let zap = signed_event(
        &actual_lsp,
        9735,
        vec![
            tag(vec!["P", &buyer.public_key().to_hex()]),
            tag(vec!["a", coordinate]),
            tag(vec!["amount", "1000"]),
        ],
        100,
    );

    assert!(verify_zap_receipt(
        &zap,
        &buyer.public_key().to_hex(),
        coordinate,
        &declared_lsp.public_key(),
    )
    .is_err());
}

#[test]
fn attestation_event_round_trips_scope_and_revocation() {
    let operator = Keys::generate();
    let developer = Keys::generate();
    let fulfillment = Keys::generate();
    let event = build_attestation_event_builder(
        &developer.public_key(),
        &fulfillment.public_key(),
        1_700_000_000,
        Some(1_800_000_000),
        Some("game-1"),
    )
    .expect("build attestation")
    .sign_with_keys(&operator)
    .expect("sign attestation");

    assert_eq!(event.kind.as_u16(), 30404);
    assert_eq!(event.pubkey, operator.public_key());

    let parsed = parse_attestation_event(&event).expect("parse attestation");
    assert_eq!(parsed.developer_pubkey, developer.public_key());
    assert_eq!(parsed.fulfillment_pubkey, fulfillment.public_key());
    assert_eq!(parsed.valid_from, 1_700_000_000);
    assert_eq!(parsed.revoked_at, Some(1_800_000_000));
    assert_eq!(parsed.scope.as_deref(), Some("game-1"));
}

#[test]
fn acceptance_event_round_trips_operator_and_fulfillment_key() {
    let developer = Keys::generate();
    let operator = Keys::generate();
    let fulfillment = Keys::generate();
    let event = build_acceptance_event_builder(&operator.public_key(), &fulfillment.public_key())
        .expect("build acceptance")
        .sign_with_keys(&developer)
        .expect("sign acceptance");

    assert_eq!(event.kind.as_u16(), 30406);
    assert_eq!(event.pubkey, developer.public_key());

    let parsed = parse_acceptance_event(&event).expect("parse acceptance");
    assert_eq!(parsed.developer_pubkey, developer.public_key());
    assert_eq!(parsed.operator_pubkey, operator.public_key());
    assert_eq!(parsed.fulfillment_pubkey, fulfillment.public_key());
}

#[test]
fn attestation_parse_requires_matching_d_tag() {
    let operator = Keys::generate();
    let developer = Keys::generate();
    let fulfillment = Keys::generate();
    let event = signed_event(
        &operator,
        30404,
        vec![
            tag(vec!["d", "wrong:coordinate"]),
            tag(vec!["p", &developer.public_key().to_hex()]),
            tag(vec![
                "fulfillment_pubkey",
                &fulfillment.public_key().to_hex(),
                "1700000000",
                "",
            ]),
        ],
        100,
    );

    assert!(parse_attestation_event(&event).is_err());
}
