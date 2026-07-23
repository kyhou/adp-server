use adp_core::{
    build_attestation_event_builder, parse_attestation_event, AcquisitionPolicy, AdpListing,
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
fn d_tag_with_extra_field_is_rejected() {
    let developer = Keys::generate();
    let event = signed_event(
        &developer,
        30402,
        vec![tag(vec!["d", "game-1", "extra"])],
        100,
    );

    assert!(AdpListing::from_event(event).is_err());
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
