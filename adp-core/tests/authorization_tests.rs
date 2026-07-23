use std::collections::BTreeSet;

use adp_core::{
    build_authorization_cancellation_event_builder, build_authorization_root_event_builder,
    parse_authorization_event, resolve_authorization_at, AuthorizationTerms, ChainResolutionError,
    CAPABILITY_ISSUE_GRANT, CAPABILITY_ISSUE_RECEIPT,
};
use nostr::{Event, EventBuilder, Keys, Kind, Tag, Timestamp};

fn terms(
    developer: &Keys,
    operator: &Keys,
    fulfillment: &Keys,
    capabilities: &[&str],
) -> AuthorizationTerms {
    AuthorizationTerms {
        authorization_id: "authorization-1".into(),
        coordinate: format!("30402:{}:game", developer.public_key()),
        operator_pubkey: operator.public_key(),
        fulfillment_pubkey: fulfillment.public_key(),
        capabilities: capabilities.iter().map(|value| (*value).into()).collect(),
        valid_from: 100,
    }
}

fn root(developer: &Keys, terms: &AuthorizationTerms) -> Event {
    build_authorization_root_event_builder(terms)
        .unwrap()
        .custom_created_at(Timestamp::from_secs(100))
        .sign_with_keys(developer)
        .unwrap()
}

#[test]
fn authorization_root_has_exact_current_shape_and_capabilities() {
    let developer = Keys::generate();
    let operator = Keys::generate();
    let fulfillment = Keys::generate();
    let terms = terms(
        &developer,
        &operator,
        &fulfillment,
        &[CAPABILITY_ISSUE_RECEIPT, CAPABILITY_ISSUE_GRANT],
    );
    let root = root(&developer, &terms);
    let parsed = parse_authorization_event(&root).unwrap();
    assert_eq!(parsed.terms, terms);
    let resolved = resolve_authorization_at(root.id, &[root], 100).unwrap();
    assert!(resolved.authorizes(
        &fulfillment.public_key(),
        &resolved.terms.coordinate,
        CAPABILITY_ISSUE_RECEIPT,
        100,
    ));
}

#[test]
fn unknown_capability_grants_no_authority() {
    let developer = Keys::generate();
    let operator = Keys::generate();
    let fulfillment = Keys::generate();
    let terms = terms(&developer, &operator, &fulfillment, &["future-capability"]);
    let root = root(&developer, &terms);
    let resolved = resolve_authorization_at(root.id, &[root], 100).unwrap();
    assert!(!resolved.authorizes(
        &fulfillment.public_key(),
        &resolved.terms.coordinate,
        CAPABILITY_ISSUE_RECEIPT,
        100,
    ));
}

#[test]
fn multiple_authorizations_do_not_share_capabilities() {
    let developer = Keys::generate();
    let operator = Keys::generate();
    let receipt_key = Keys::generate();
    let grant_key = Keys::generate();
    let receipt_terms = terms(
        &developer,
        &operator,
        &receipt_key,
        &[CAPABILITY_ISSUE_RECEIPT],
    );
    let grant_terms = AuthorizationTerms {
        authorization_id: "authorization-2".into(),
        fulfillment_pubkey: grant_key.public_key(),
        capabilities: BTreeSet::from([CAPABILITY_ISSUE_GRANT.into()]),
        ..receipt_terms.clone()
    };
    let receipt_root = root(&developer, &receipt_terms);
    let grant_root = root(&developer, &grant_terms);
    let receipt = resolve_authorization_at(receipt_root.id, &[receipt_root], 100).unwrap();
    let grant = resolve_authorization_at(grant_root.id, &[grant_root], 100).unwrap();
    assert!(receipt.has_capability(CAPABILITY_ISSUE_RECEIPT));
    assert!(!receipt.has_capability(CAPABILITY_ISSUE_GRANT));
    assert!(grant.has_capability(CAPABILITY_ISSUE_GRANT));
}

#[test]
fn cancellation_is_half_open_at_credential_root_time() {
    let developer = Keys::generate();
    let operator = Keys::generate();
    let fulfillment = Keys::generate();
    let terms = terms(
        &developer,
        &operator,
        &fulfillment,
        &[CAPABILITY_ISSUE_RECEIPT],
    );
    let root = root(&developer, &terms);
    let cancel = build_authorization_cancellation_event_builder(&terms, root.id)
        .unwrap()
        .custom_created_at(Timestamp::from_secs(150))
        .sign_with_keys(&developer)
        .unwrap();
    let events = [root.clone(), cancel];
    let before = resolve_authorization_at(root.id, &events, 149).unwrap();
    let boundary = resolve_authorization_at(root.id, &events, 150).unwrap();
    assert!(before.authorizes(
        &fulfillment.public_key(),
        &terms.coordinate,
        CAPABILITY_ISSUE_RECEIPT,
        149
    ));
    assert!(!boundary.authorizes(
        &fulfillment.public_key(),
        &terms.coordinate,
        CAPABILITY_ISSUE_RECEIPT,
        150
    ));
}

#[test]
fn malformed_successor_is_excluded_and_valid_sibling_recovers() {
    let developer = Keys::generate();
    let operator = Keys::generate();
    let fulfillment = Keys::generate();
    let terms = terms(
        &developer,
        &operator,
        &fulfillment,
        &[CAPABILITY_ISSUE_RECEIPT],
    );
    let root = root(&developer, &terms);
    let malformed = EventBuilder::new(Kind::Custom(30406), "")
        .tags(vec![Tag::parse(["e", &root.id.to_hex()]).unwrap()])
        .custom_created_at(Timestamp::from_secs(140))
        .sign_with_keys(&developer)
        .unwrap();
    let valid = build_authorization_cancellation_event_builder(&terms, root.id)
        .unwrap()
        .custom_created_at(Timestamp::from_secs(150))
        .sign_with_keys(&developer)
        .unwrap();
    let resolved = resolve_authorization_at(root.id, &[root, malformed, valid], 150).unwrap();
    assert_eq!(resolved.cancelled_at, Some(150));
}

#[test]
fn two_valid_successors_fail_closed() {
    let developer = Keys::generate();
    let operator = Keys::generate();
    let fulfillment = Keys::generate();
    let terms = terms(
        &developer,
        &operator,
        &fulfillment,
        &[CAPABILITY_ISSUE_RECEIPT],
    );
    let root = root(&developer, &terms);
    let first = build_authorization_cancellation_event_builder(&terms, root.id)
        .unwrap()
        .custom_created_at(Timestamp::from_secs(150))
        .sign_with_keys(&developer)
        .unwrap();
    let second = build_authorization_cancellation_event_builder(&terms, root.id)
        .unwrap()
        .custom_created_at(Timestamp::from_secs(151))
        .sign_with_keys(&developer)
        .unwrap();
    assert!(matches!(
        resolve_authorization_at(root.id, &[root, first, second], 200),
        Err(ChainResolutionError::Fork(_))
    ));
}

#[test]
fn valid_from_cannot_precede_root_timestamp() {
    let developer = Keys::generate();
    let operator = Keys::generate();
    let fulfillment = Keys::generate();
    let mut terms = terms(
        &developer,
        &operator,
        &fulfillment,
        &[CAPABILITY_ISSUE_RECEIPT],
    );
    terms.valid_from = 99;
    let event = build_authorization_root_event_builder(&terms)
        .unwrap()
        .custom_created_at(Timestamp::from_secs(100))
        .sign_with_keys(&developer)
        .unwrap();
    assert!(parse_authorization_event(&event).is_err());
}

#[test]
fn authorization_root_requires_coordinate_developer_signature() {
    let developer = Keys::generate();
    let stranger = Keys::generate();
    let operator = Keys::generate();
    let fulfillment = Keys::generate();
    let terms = terms(
        &developer,
        &operator,
        &fulfillment,
        &[CAPABILITY_ISSUE_RECEIPT],
    );
    let event = build_authorization_root_event_builder(&terms)
        .unwrap()
        .custom_created_at(Timestamp::from_secs(100))
        .sign_with_keys(&stranger)
        .unwrap();
    assert!(parse_authorization_event(&event).is_err());
}

#[test]
fn authorization_root_rejects_duplicate_and_forbidden_tags() {
    let developer = Keys::generate();
    let operator = Keys::generate();
    let fulfillment = Keys::generate();
    let terms = terms(
        &developer,
        &operator,
        &fulfillment,
        &[CAPABILITY_ISSUE_RECEIPT],
    );
    let duplicate_d = build_authorization_root_event_builder(&terms)
        .unwrap()
        .tag(Tag::parse(["d", "duplicate"]).unwrap())
        .custom_created_at(Timestamp::from_secs(100))
        .sign_with_keys(&developer)
        .unwrap();
    assert!(parse_authorization_event(&duplicate_d).is_err());

    let forbidden = build_authorization_root_event_builder(&terms)
        .unwrap()
        .tag(Tag::parse(["revoked_at", "100"]).unwrap())
        .custom_created_at(Timestamp::from_secs(100))
        .sign_with_keys(&developer)
        .unwrap();
    assert!(parse_authorization_event(&forbidden).is_err());
}

#[test]
fn cancellation_must_preserve_complete_root_terms() {
    let developer = Keys::generate();
    let operator = Keys::generate();
    let fulfillment = Keys::generate();
    let terms = terms(
        &developer,
        &operator,
        &fulfillment,
        &[CAPABILITY_ISSUE_RECEIPT],
    );
    let root = root(&developer, &terms);
    let mut changed = terms.clone();
    changed.capabilities.insert(CAPABILITY_ISSUE_GRANT.into());
    let invalid = build_authorization_cancellation_event_builder(&changed, root.id)
        .unwrap()
        .custom_created_at(Timestamp::from_secs(150))
        .sign_with_keys(&developer)
        .unwrap();
    let resolved = resolve_authorization_at(root.id, &[root, invalid], 200).unwrap();
    assert_eq!(resolved.cancelled_at, None);
}

fn _set_contract(_: BTreeSet<String>) {}
