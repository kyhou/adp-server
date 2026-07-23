use adp_core::{
    build_attestation_event_builder, parse_attestation_event, AcquisitionPolicy, AdpListing,
};
use nostr::{EventBuilder, Keys, Kind, Tag, Timestamp};

fn tag(values: &[&str]) -> Tag {
    Tag::parse(values.iter().copied()).unwrap()
}

fn listing(developer: &Keys, extra: Vec<Tag>) -> AdpListing {
    let mut tags = vec![tag(&["d", "game"]), tag(&["price", "0", "SATS"])];
    tags.extend(extra);
    let event = EventBuilder::new(Kind::Custom(30402), "")
        .tags(tags)
        .sign_with_keys(developer)
        .unwrap();
    AdpListing::from_event(event).unwrap()
}

#[test]
fn zero_price_without_explicit_acquisition_remains_gated() {
    assert_eq!(
        listing(&Keys::generate(), vec![]).acquisition_policy,
        AcquisitionPolicy::Gated
    );
}

#[test]
fn public_and_timed_access_are_explicit_and_half_open() {
    let developer = Keys::generate();
    assert_eq!(
        listing(&developer, vec![tag(&["acquisition", "public"])]).acquisition_policy,
        AcquisitionPolicy::Public
    );
    let timed = listing(
        &developer,
        vec![tag(&["acquisition", "timed-access", "10", "20"])],
    );
    assert!(!timed.acquisition_policy.allows_at(9));
    assert!(timed.acquisition_policy.allows_at(10));
    assert!(!timed.acquisition_policy.allows_at(20));
}

#[test]
fn attestation_remains_operational_and_round_trips_revocation() {
    let operator = Keys::generate();
    let developer = Keys::generate();
    let fulfillment = Keys::generate();
    let event = build_attestation_event_builder(
        &developer.public_key(),
        &fulfillment.public_key(),
        100,
        Some(200),
        Some("game"),
    )
    .unwrap()
    .custom_created_at(Timestamp::from_secs(200))
    .sign_with_keys(&operator)
    .unwrap();
    let parsed = parse_attestation_event(&event).unwrap();
    assert_eq!(parsed.developer_pubkey, developer.public_key());
    assert_eq!(parsed.fulfillment_pubkey, fulfillment.public_key());
    assert_eq!(parsed.revoked_at, Some(200));
}
