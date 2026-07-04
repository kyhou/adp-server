//! Pure logic tests for `verify_signer_authorization` — no network, no relays.
//! These exercise the validity-interval semantics described in ADP-01.md
//! "Fulfillment Key Delegation".

use adp_core::{verify_signer_authorization, AdpListing, SignerAuthorization};
use nostr::{EventBuilder, Keys, Kind, Tag, Timestamp};

fn tag(values: Vec<&str>) -> Tag {
    Tag::parse(values).expect("valid tag")
}

fn build_listing(
    developer: &Keys,
    d_tag: &str,
    delegations: &[(String, u64, Option<u64>)],
) -> nostr::Event {
    let mut tags = vec![tag(vec!["d", d_tag])];
    for (pk, valid_from, revoked_at) in delegations {
        let valid_from_s = valid_from.to_string();
        let revoked_s = revoked_at.map(|r| r.to_string()).unwrap_or_default();
        tags.push(tag(vec![
            "fulfillment_pubkey",
            pk,
            &valid_from_s,
            &revoked_s,
        ]));
    }
    EventBuilder::new(Kind::Custom(30402), "")
        .tags(tags)
        .sign_with_keys(developer)
        .expect("sign listing event")
}

fn build_receipt_at(signer: &Keys, created_at: u64) -> nostr::Event {
    EventBuilder::new(Kind::Custom(1020), "")
        .custom_created_at(Timestamp::from(created_at))
        .sign_with_keys(signer)
        .expect("sign receipt event")
}

#[test]
fn direct_developer_signature_is_authorized() {
    let developer = Keys::generate();
    let listing_event = build_listing(&developer, "game-1", &[]);
    let listing = AdpListing::from_event(listing_event).expect("parse listing");
    let receipt = build_receipt_at(&developer, 1_700_000_100);

    let result = verify_signer_authorization(&receipt, &listing).expect("should authorize");
    assert_eq!(result, SignerAuthorization::DirectDeveloper);
}

#[test]
fn active_delegated_key_is_authorized() {
    let developer = Keys::generate();
    let fulfillment = Keys::generate();
    let listing_event = build_listing(
        &developer,
        "game-1",
        &[(fulfillment.public_key().to_hex(), 1_700_000_000, None)],
    );
    let listing = AdpListing::from_event(listing_event).expect("parse listing");
    let receipt = build_receipt_at(&fulfillment, 1_700_000_100);

    let result = verify_signer_authorization(&receipt, &listing).expect("should authorize");
    assert!(matches!(result, SignerAuthorization::Delegated { .. }));
}

#[test]
fn receipt_before_valid_from_is_rejected() {
    let developer = Keys::generate();
    let fulfillment = Keys::generate();
    let listing_event = build_listing(
        &developer,
        "game-1",
        &[(fulfillment.public_key().to_hex(), 1_700_000_000, None)],
    );
    let listing = AdpListing::from_event(listing_event).expect("parse listing");
    // Signed before the delegation's valid_from.
    let receipt = build_receipt_at(&fulfillment, 1_699_999_999);

    assert!(verify_signer_authorization(&receipt, &listing).is_err());
}

#[test]
fn revoked_key_is_rejected_for_receipts_after_revocation() {
    let developer = Keys::generate();
    let fulfillment = Keys::generate();
    let listing_event = build_listing(
        &developer,
        "game-1",
        &[(
            fulfillment.public_key().to_hex(),
            1_700_000_000,
            Some(1_735_000_000),
        )],
    );
    let listing = AdpListing::from_event(listing_event).expect("parse listing");
    // Signed at the exact revocation instant — interval is open-ended, so this must fail.
    let receipt = build_receipt_at(&fulfillment, 1_735_000_000);

    let result = verify_signer_authorization(&receipt, &listing);
    assert!(result.is_err());
}

#[test]
fn revoked_key_still_authorizes_receipts_signed_before_revocation() {
    let developer = Keys::generate();
    let fulfillment = Keys::generate();
    let listing_event = build_listing(
        &developer,
        "game-1",
        &[(
            fulfillment.public_key().to_hex(),
            1_700_000_000,
            Some(1_735_000_000),
        )],
    );
    let listing = AdpListing::from_event(listing_event).expect("parse listing");
    let receipt = build_receipt_at(&fulfillment, 1_710_000_000);

    assert!(verify_signer_authorization(&receipt, &listing).is_ok());
}

#[test]
fn rotation_to_new_key_authorizes_new_key_after_handoff() {
    let developer = Keys::generate();
    let old_key = Keys::generate();
    let new_key = Keys::generate();
    let listing_event = build_listing(
        &developer,
        "game-1",
        &[
            (
                old_key.public_key().to_hex(),
                1_700_000_000,
                Some(1_735_000_000),
            ),
            (new_key.public_key().to_hex(), 1_735_000_000, None),
        ],
    );
    let listing = AdpListing::from_event(listing_event).expect("parse listing");

    let receipt_old = build_receipt_at(&old_key, 1_720_000_000);
    let receipt_new = build_receipt_at(&new_key, 1_740_000_000);

    assert!(verify_signer_authorization(&receipt_old, &listing).is_ok());
    assert!(verify_signer_authorization(&receipt_new, &listing).is_ok());
}

#[test]
fn unrelated_key_is_rejected() {
    let developer = Keys::generate();
    let stranger = Keys::generate();
    let listing_event = build_listing(&developer, "game-1", &[]);
    let listing = AdpListing::from_event(listing_event).expect("parse listing");
    let receipt = build_receipt_at(&stranger, 1_700_000_100);

    assert!(verify_signer_authorization(&receipt, &listing).is_err());
}
