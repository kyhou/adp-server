use std::collections::BTreeSet;

use adp_core::{
    build_authorization_cancellation_event_builder, build_authorization_root_event_builder,
    parse_authorization_event, resolve_authorization_at, resolve_receipt_chain, AuthorizationTerms,
    CAPABILITY_ISSUE_GRANT, CAPABILITY_ISSUE_RECEIPT,
};
use nostr::{Event, EventBuilder, EventId, Keys, Kind, Tag, Timestamp};

fn tag(values: &[&str]) -> Tag {
    Tag::parse(values.iter().copied()).unwrap()
}

fn coordinate(developer: &Keys) -> String {
    format!("30402:{}:game", developer.public_key())
}

fn authorization(
    developer: &Keys,
    fulfillment: &Keys,
    capability: &str,
    cancelled_at: Option<u64>,
) -> adp_core::ResolvedAuthorization {
    let terms = AuthorizationTerms {
        authorization_id: "auth".into(),
        coordinate: coordinate(developer),
        operator_pubkey: Keys::generate().public_key(),
        fulfillment_pubkey: fulfillment.public_key(),
        capabilities: BTreeSet::from([capability.into()]),
        valid_from: 90,
    };
    let root = build_authorization_root_event_builder(&terms)
        .unwrap()
        .custom_created_at(Timestamp::from_secs(90))
        .sign_with_keys(developer)
        .unwrap();
    let mut events = vec![root.clone()];
    if let Some(at) = cancelled_at {
        events.push(
            build_authorization_cancellation_event_builder(&terms, root.id)
                .unwrap()
                .custom_created_at(Timestamp::from_secs(at))
                .sign_with_keys(developer)
                .unwrap(),
        );
    }
    resolve_authorization_at(root.id, &events, u64::MAX).unwrap()
}

fn receipt(
    signer: &Keys,
    buyer: &Keys,
    coordinate: &str,
    authorization: Option<EventId>,
    predecessor: Option<EventId>,
    status: &str,
    at: u64,
) -> Event {
    let mut tags = vec![
        tag(&["order", "order-1"]),
        tag(&["p", &buyer.public_key().to_hex()]),
        tag(&["a", coordinate]),
        tag(&["status", status]),
    ];
    if let Some(root) = authorization {
        tags.push(tag(&["authorization", &root.to_hex()]));
    }
    if let Some(previous) = predecessor {
        tags.push(tag(&["e", &previous.to_hex()]));
    } else {
        tags.extend([
            tag(&["payment_hash", &"11".repeat(32)]),
            tag(&["amount_msat", "1000"]),
            tag(&["settled_at", &at.to_string()]),
            tag(&["proof", "bolt11-preimage"]),
        ]);
    }
    EventBuilder::new(Kind::Custom(1020), "encrypted")
        .tags(tags)
        .custom_created_at(Timestamp::from_secs(at))
        .sign_with_keys(signer)
        .unwrap()
}

#[test]
fn direct_developer_receipt_requires_no_authorization() {
    let developer = Keys::generate();
    let buyer = Keys::generate();
    let root = receipt(
        &developer,
        &buyer,
        &coordinate(&developer),
        None,
        None,
        "paid",
        100,
    );
    assert!(resolve_receipt_chain(&[root], developer.public_key(), None).is_ok());
}

#[test]
fn adp_rejects_redundant_direct_authorization() {
    let developer = Keys::generate();
    let buyer = Keys::generate();
    let anchor = EventId::all_zeros();
    let root = receipt(
        &developer,
        &buyer,
        &coordinate(&developer),
        Some(anchor),
        None,
        "paid",
        100,
    );
    assert!(resolve_receipt_chain(&[root], developer.public_key(), None).is_err());
}

#[test]
fn delegated_receipt_requires_exact_issue_receipt_root() {
    let developer = Keys::generate();
    let fulfillment = Keys::generate();
    let buyer = Keys::generate();
    let auth = authorization(&developer, &fulfillment, CAPABILITY_ISSUE_RECEIPT, None);
    let root = receipt(
        &fulfillment,
        &buyer,
        &coordinate(&developer),
        Some(auth.root_event_id),
        None,
        "paid",
        100,
    );
    assert!(resolve_receipt_chain(&[root], developer.public_key(), Some(&auth)).is_ok());

    let grant_only = authorization(&developer, &fulfillment, CAPABILITY_ISSUE_GRANT, None);
    let wrong = receipt(
        &fulfillment,
        &buyer,
        &coordinate(&developer),
        Some(grant_only.root_event_id),
        None,
        "paid",
        100,
    );
    assert!(resolve_receipt_chain(&[wrong], developer.public_key(), Some(&grant_only)).is_err());
}

#[test]
fn receipt_before_cancellation_remains_valid_with_later_status_update() {
    let developer = Keys::generate();
    let fulfillment = Keys::generate();
    let buyer = Keys::generate();
    let auth = authorization(
        &developer,
        &fulfillment,
        CAPABILITY_ISSUE_RECEIPT,
        Some(110),
    );
    let root = receipt(
        &fulfillment,
        &buyer,
        &coordinate(&developer),
        Some(auth.root_event_id),
        None,
        "paid",
        100,
    );
    let update = receipt(
        &fulfillment,
        &buyer,
        &coordinate(&developer),
        Some(auth.root_event_id),
        Some(root.id),
        "fulfilled",
        120,
    );
    assert_eq!(
        resolve_receipt_chain(&[root, update.clone()], developer.public_key(), Some(&auth))
            .unwrap()
            .id,
        update.id
    );
}

#[test]
fn receipt_at_cancellation_boundary_is_invalid() {
    let developer = Keys::generate();
    let fulfillment = Keys::generate();
    let buyer = Keys::generate();
    let terms = AuthorizationTerms {
        authorization_id: "auth".into(),
        coordinate: coordinate(&developer),
        operator_pubkey: Keys::generate().public_key(),
        fulfillment_pubkey: fulfillment.public_key(),
        capabilities: BTreeSet::from([CAPABILITY_ISSUE_RECEIPT.into()]),
        valid_from: 90,
    };
    let auth_root = build_authorization_root_event_builder(&terms)
        .unwrap()
        .custom_created_at(Timestamp::from_secs(90))
        .sign_with_keys(&developer)
        .unwrap();
    let cancel = build_authorization_cancellation_event_builder(&terms, auth_root.id)
        .unwrap()
        .custom_created_at(Timestamp::from_secs(100))
        .sign_with_keys(&developer)
        .unwrap();
    let auth = resolve_authorization_at(auth_root.id, &[auth_root, cancel], 100).unwrap();
    let root = receipt(
        &fulfillment,
        &buyer,
        &terms.coordinate,
        Some(auth.root_event_id),
        None,
        "paid",
        100,
    );
    assert!(resolve_receipt_chain(&[root], developer.public_key(), Some(&auth)).is_err());
}

#[test]
fn malformed_later_receipt_does_not_hide_valid_root() {
    let developer = Keys::generate();
    let buyer = Keys::generate();
    let root = receipt(
        &developer,
        &buyer,
        &coordinate(&developer),
        None,
        None,
        "paid",
        100,
    );
    let malformed = EventBuilder::new(Kind::Custom(1020), "bad")
        .tags(vec![tag(&["e", &root.id.to_hex()])])
        .custom_created_at(Timestamp::from_secs(110))
        .sign_with_keys(&developer)
        .unwrap();
    assert_eq!(
        resolve_receipt_chain(&[root.clone(), malformed], developer.public_key(), None)
            .unwrap()
            .id,
        root.id
    );
}

#[test]
fn duplicate_relay_delivery_is_deduplicated() {
    let developer = Keys::generate();
    let buyer = Keys::generate();
    let root = receipt(
        &developer,
        &buyer,
        &coordinate(&developer),
        None,
        None,
        "paid",
        100,
    );
    assert_eq!(
        resolve_receipt_chain(&[root.clone(), root.clone()], developer.public_key(), None)
            .unwrap()
            .id,
        root.id
    );
}

#[test]
fn two_valid_receipt_successors_fail_closed() {
    let developer = Keys::generate();
    let buyer = Keys::generate();
    let coordinate = coordinate(&developer);
    let root = receipt(&developer, &buyer, &coordinate, None, None, "paid", 100);
    let fulfilled = receipt(
        &developer,
        &buyer,
        &coordinate,
        None,
        Some(root.id),
        "fulfilled",
        110,
    );
    let disputed = receipt(
        &developer,
        &buyer,
        &coordinate,
        None,
        Some(root.id),
        "disputed",
        111,
    );
    assert!(
        resolve_receipt_chain(&[root, fulfilled, disputed], developer.public_key(), None).is_err()
    );
}

#[test]
fn ordinary_bolt_receipt_validation_never_decrypts_content() {
    let developer = Keys::generate();
    let buyer = Keys::generate();
    let mut root = receipt(
        &developer,
        &buyer,
        &coordinate(&developer),
        None,
        None,
        "paid",
        100,
    );
    root.content = "opaque-and-not-decryptable".into();
    let root = EventBuilder::new(Kind::Custom(1020), root.content.clone())
        .tags(root.tags)
        .custom_created_at(Timestamp::from_secs(100))
        .sign_with_keys(&developer)
        .unwrap();
    assert!(resolve_receipt_chain(&[root], developer.public_key(), None).is_ok());
}

#[test]
fn receipt_parser_rejects_wrong_kind_and_duplicate_public_binding() {
    let developer = Keys::generate();
    let buyer = Keys::generate();
    let wrong_kind = EventBuilder::new(Kind::TextNote, "")
        .sign_with_keys(&developer)
        .unwrap();
    assert!(adp_core::parse_receipt_event(&wrong_kind).is_err());

    let coordinate = coordinate(&developer);
    let root = receipt(&developer, &buyer, &coordinate, None, None, "paid", 100);
    let duplicate = EventBuilder::new(Kind::Custom(1020), root.content.clone())
        .tags(
            root.tags
                .iter()
                .cloned()
                .chain([tag(&["payment_hash", &"22".repeat(32)])])
                .collect::<Vec<_>>(),
        )
        .custom_created_at(Timestamp::from_secs(100))
        .sign_with_keys(&developer)
        .unwrap();
    assert!(adp_core::parse_receipt_event(&duplicate).is_err());
}

#[test]
fn relay_hinted_coordinate_is_accepted() {
    let developer = Keys::generate();
    let buyer = Keys::generate();
    let coordinate = coordinate(&developer);
    let root = receipt(&developer, &buyer, &coordinate, None, None, "paid", 100);
    let hinted = EventBuilder::new(Kind::Custom(1020), root.content.clone())
        .tags(
            root.tags
                .iter()
                .map(|tag_value| {
                    let values = tag_value.clone().to_vec();
                    if values.first().is_some_and(|name| name == "a") {
                        tag(&["a", &coordinate, "wss://relay.example.com"])
                    } else {
                        tag_value.clone()
                    }
                })
                .collect::<Vec<_>>(),
        )
        .custom_created_at(Timestamp::from_secs(100))
        .sign_with_keys(&developer)
        .unwrap();
    assert!(resolve_receipt_chain(&[hinted], developer.public_key(), None).is_ok());
}

#[test]
fn duplicate_receipt_update_is_deduplicated_and_refund_is_terminal() {
    let developer = Keys::generate();
    let buyer = Keys::generate();
    let coordinate = coordinate(&developer);
    let root = receipt(&developer, &buyer, &coordinate, None, None, "paid", 100);
    let refund = receipt(
        &developer,
        &buyer,
        &coordinate,
        None,
        Some(root.id),
        "refunded",
        110,
    );
    let invalid_successor = receipt(
        &developer,
        &buyer,
        &coordinate,
        None,
        Some(refund.id),
        "fulfilled",
        120,
    );
    let events = [root, refund.clone(), refund.clone(), invalid_successor];
    let tip = resolve_receipt_chain(&events, developer.public_key(), None).unwrap();
    assert_eq!(tip.id, refund.id);
}

fn _parser_contract(event: &Event) {
    let _ = parse_authorization_event(event);
}
