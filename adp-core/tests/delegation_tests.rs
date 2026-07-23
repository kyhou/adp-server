use adp_core::AdpListing;
use nostr::{EventBuilder, EventId, Keys, Kind, Tag};

fn tag(values: &[&str]) -> Tag {
    Tag::parse(values.iter().copied()).unwrap()
}

#[test]
fn listing_parses_multiple_authorizations_and_classifies_malformed_entries() {
    let developer = Keys::generate();
    let first = Keys::generate();
    let second = Keys::generate();
    let first_root = EventId::all_zeros();
    let second_root = EventId::from_hex(&"11".repeat(32)).unwrap();
    let event = EventBuilder::new(Kind::Custom(30402), "")
        .tags(vec![
            tag(&["d", "game"]),
            tag(&[
                "fulfillment_authorization",
                &first_root.to_hex(),
                &first.public_key().to_hex(),
            ]),
            tag(&[
                "fulfillment_authorization",
                &second_root.to_hex(),
                &second.public_key().to_hex(),
                "wss://relay.example.com",
            ]),
            tag(&["fulfillment_authorization", "bad", "bad"]),
        ])
        .sign_with_keys(&developer)
        .unwrap();
    let listing = AdpListing::from_event(event).unwrap();
    assert_eq!(listing.fulfillment_authorizations.len(), 2);
    assert_eq!(listing.malformed_fulfillment_authorizations, 1);
    assert_eq!(
        listing.malformed_fulfillment_authorization_tags,
        vec![vec![
            "fulfillment_authorization".to_string(),
            "bad".to_string(),
            "bad".to_string(),
        ]]
    );
    assert_eq!(
        listing.fulfillment_authorizations[0].root_event_id,
        first_root
    );
    assert_eq!(
        listing.fulfillment_authorizations[1].root_event_id,
        second_root
    );
}

#[test]
fn legacy_listing_delegation_is_not_reinterpreted() {
    let developer = Keys::generate();
    let fulfillment = Keys::generate();
    let event = EventBuilder::new(Kind::Custom(30402), "")
        .tags(vec![
            tag(&["d", "game"]),
            tag(&[
                "fulfillment_pubkey",
                &fulfillment.public_key().to_hex(),
                "1",
                "",
            ]),
        ])
        .sign_with_keys(&developer)
        .unwrap();
    let listing = AdpListing::from_event(event).unwrap();
    assert!(listing.fulfillment_authorizations.is_empty());
}
