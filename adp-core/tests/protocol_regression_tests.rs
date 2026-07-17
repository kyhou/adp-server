use adp_core::provisioning::parse_authorization_event;
use adp_core::{
    build_acceptance_event_builder, build_attestation_event_builder, latest_status_in_chain,
    parse_acceptance_event, parse_attestation_event, parse_receipt_event, verify_zap_receipt,
    AcquisitionPolicy, AdpListing,
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
fn acquisition_absent_is_gated() {
    let developer = Keys::generate();
    let event = signed_event(&developer, 30402, vec![tag(vec!["d", "game-1"])], 100);

    let listing = AdpListing::from_event(event).expect("parse listing");

    assert_eq!(listing.acquisition_policy, AcquisitionPolicy::Gated);
    assert!(!listing.acquisition_policy.allows_at(100));
}

#[test]
fn acquisition_public_is_explicit() {
    let developer = Keys::generate();
    let event = signed_event(
        &developer,
        30402,
        vec![tag(vec!["d", "game-1"]), tag(vec!["acquisition", "public"])],
        100,
    );

    let listing = AdpListing::from_event(event).expect("parse listing");

    assert_eq!(listing.acquisition_policy, AcquisitionPolicy::Public);
    assert!(listing.acquisition_policy.allows_at(0));
}

#[test]
fn timed_access_uses_half_open_interval() {
    let developer = Keys::generate();
    let event = signed_event(
        &developer,
        30402,
        vec![
            tag(vec!["d", "game-1"]),
            tag(vec!["acquisition", "timed-access", "100", "200"]),
        ],
        100,
    );

    let listing = AdpListing::from_event(event).expect("parse listing");

    assert_eq!(
        listing.acquisition_policy,
        AcquisitionPolicy::TimedAccess {
            starts: 100,
            ends: 200,
        }
    );
    assert!(!listing.acquisition_policy.allows_at(99));
    assert!(listing.acquisition_policy.allows_at(100));
    assert!(listing.acquisition_policy.allows_at(199));
    assert!(!listing.acquisition_policy.allows_at(200));
}

#[test]
fn duplicate_acquisition_is_invalid() {
    let developer = Keys::generate();
    let event = signed_event(
        &developer,
        30402,
        vec![
            tag(vec!["d", "game-1"]),
            tag(vec!["acquisition", "public"]),
            tag(vec!["acquisition", "public"]),
        ],
        100,
    );

    let listing = AdpListing::from_event(event).expect("parse unrelated listing fields");

    assert_eq!(listing.acquisition_policy, AcquisitionPolicy::Invalid);
    assert!(!listing.acquisition_policy.allows_at(100));
}

#[test]
fn malformed_acquisition_is_invalid() {
    let cases = [
        vec!["acquisition"],
        vec!["acquisition", "unknown"],
        vec!["acquisition", "public", "extra"],
    ];

    for acquisition in cases {
        let developer = Keys::generate();
        let event = signed_event(
            &developer,
            30402,
            vec![tag(vec!["d", "game-1"]), tag(acquisition)],
            100,
        );

        let listing = AdpListing::from_event(event).expect("parse unrelated listing fields");

        assert_eq!(listing.acquisition_policy, AcquisitionPolicy::Invalid);
        assert!(!listing.acquisition_policy.allows_at(100));
    }
}

#[test]
fn malformed_timed_access_is_invalid() {
    let cases = [
        vec!["acquisition", "timed-access", "not-a-number", "200"],
        vec!["acquisition", "timed-access", "100"],
        vec!["acquisition", "timed-access", "200", "200"],
        vec!["acquisition", "timed-access", "201", "200"],
        vec!["acquisition", "timed-access", "100", "200", "extra"],
    ];

    for acquisition in cases {
        let developer = Keys::generate();
        let event = signed_event(
            &developer,
            30402,
            vec![tag(vec!["d", "game-1"]), tag(acquisition)],
            100,
        );

        let listing = AdpListing::from_event(event).expect("parse unrelated listing fields");

        assert_eq!(listing.acquisition_policy, AcquisitionPolicy::Invalid);
        assert!(!listing.acquisition_policy.allows_at(100));
    }
}

#[test]
fn zero_price_without_acquisition_is_gated() {
    let developer = Keys::generate();
    let event = signed_event(
        &developer,
        30402,
        vec![tag(vec!["d", "game-1"]), tag(vec!["price", "0", "SATS"])],
        100,
    );

    let listing = AdpListing::from_event(event).expect("parse listing");

    assert_eq!(listing.price, Some(("0".to_string(), "SATS".to_string())));
    assert_eq!(listing.acquisition_policy, AcquisitionPolicy::Gated);
}

#[test]
fn campaign_pointer_accepts_event_id_without_hint() {
    let developer = Keys::generate();
    let campaign = signed_event(&developer, 30078, vec![], 99);
    let campaign_id = campaign.id.to_hex();
    let event = signed_event(
        &developer,
        30402,
        vec![
            tag(vec!["d", "game-1"]),
            tag(vec!["campaign", &campaign_id]),
        ],
        100,
    );

    let listing = AdpListing::from_event(event).expect("parse listing");

    assert_eq!(listing.campaign_pointers.len(), 1);
    assert_eq!(listing.campaign_pointers[0].root_event_id, campaign.id);
    assert_eq!(listing.campaign_pointers[0].relay_hint, None);
}

#[test]
fn campaign_pointer_accepts_wss_hint() {
    let developer = Keys::generate();
    let campaign = signed_event(&developer, 30078, vec![], 99);
    let campaign_id = campaign.id.to_hex();
    let event = signed_event(
        &developer,
        30402,
        vec![
            tag(vec!["d", "game-1"]),
            tag(vec!["campaign", &campaign_id, "wss://relay.example.com"]),
        ],
        100,
    );

    let listing = AdpListing::from_event(event).expect("parse listing");

    assert_eq!(listing.campaign_pointers.len(), 1);
    assert_eq!(
        listing.campaign_pointers[0]
            .relay_hint
            .as_ref()
            .map(ToString::to_string)
            .as_deref(),
        Some("wss://relay.example.com")
    );
}

#[test]
fn campaign_pointer_keeps_id_when_hint_is_malformed() {
    let developer = Keys::generate();
    let campaign = signed_event(&developer, 30078, vec![], 99);
    let campaign_id = campaign.id.to_hex();
    let event = signed_event(
        &developer,
        30402,
        vec![
            tag(vec!["d", "game-1"]),
            tag(vec!["campaign", &campaign_id, "not-a-relay"]),
        ],
        100,
    );

    let listing = AdpListing::from_event(event).expect("parse listing");

    assert_eq!(listing.campaign_pointers.len(), 1);
    assert_eq!(listing.campaign_pointers[0].root_event_id, campaign.id);
    assert_eq!(listing.campaign_pointers[0].relay_hint, None);
}

#[test]
fn campaign_pointer_discards_ws_hint() {
    let developer = Keys::generate();
    let campaign = signed_event(&developer, 30078, vec![], 99);
    let campaign_id = campaign.id.to_hex();
    let event = signed_event(
        &developer,
        30402,
        vec![
            tag(vec!["d", "game-1"]),
            tag(vec!["campaign", &campaign_id, "ws://relay.example.com"]),
        ],
        100,
    );

    let listing = AdpListing::from_event(event).expect("parse listing");

    assert_eq!(listing.campaign_pointers.len(), 1);
    assert_eq!(listing.campaign_pointers[0].root_event_id, campaign.id);
    assert_eq!(listing.campaign_pointers[0].relay_hint, None);
}

#[test]
fn campaign_pointer_discards_malformed_event_id() {
    let developer = Keys::generate();
    let event = signed_event(
        &developer,
        30402,
        vec![
            tag(vec!["d", "game-1"]),
            tag(vec!["campaign", "not-an-event-id"]),
        ],
        100,
    );

    let listing = AdpListing::from_event(event).expect("parse listing");

    assert!(listing.campaign_pointers.is_empty());
}

#[test]
fn campaign_pointer_with_extra_fields_is_ignored() {
    let developer = Keys::generate();
    let campaign = signed_event(&developer, 30078, vec![], 99);
    let campaign_id = campaign.id.to_hex();
    let event = signed_event(
        &developer,
        30402,
        vec![
            tag(vec!["d", "game-1"]),
            tag(vec![
                "campaign",
                &campaign_id,
                "wss://relay.example.com",
                "extra",
            ]),
        ],
        100,
    );

    let listing = AdpListing::from_event(event).expect("parse listing");

    assert!(listing.campaign_pointers.is_empty());
}

#[test]
fn duplicate_d_tag_is_rejected() {
    let developer = Keys::generate();
    let event = signed_event(
        &developer,
        30402,
        vec![tag(vec!["d", "game-1"]), tag(vec!["d", "game-2"])],
        100,
    );

    assert!(AdpListing::from_event(event).is_err());
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
    assert!(parse_authorization_event(&event).is_err());
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
