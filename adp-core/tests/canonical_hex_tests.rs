//! ADP-01 §Canonical Hex Encoding (gap S10): every ADP-defined hex field is
//! accepted only in lowercase. Uppercase is rejected, never normalized. Each
//! test pairs a lowercase control that must still parse.
use adp_core::provisioning::{parse_attestation_event, parse_authorization_event};
use adp_core::{
    parse_entitlement_event, parse_receipt_event, AdpListing, EXPERIMENTAL_ENTITLEMENT_GRANT_KIND,
};
use nostr::{Event, EventBuilder, Keys, Kind, Tag, Timestamp};

fn tag(values: &[&str]) -> Tag {
    Tag::parse(values.iter().copied()).unwrap()
}

fn sign(keys: &Keys, kind: u16, tags: Vec<Tag>) -> Event {
    EventBuilder::new(Kind::Custom(kind), "")
        .tags(tags)
        .custom_created_at(Timestamp::from_secs(1_700_000_000))
        .sign_with_keys(keys)
        .unwrap()
}

fn hex(byte: &str) -> String {
    byte.repeat(32)
}

fn listing_with(developer: &Keys, extra: Vec<Tag>) -> AdpListing {
    let mut tags = vec![tag(&["d", "game"])];
    tags.extend(extra);
    AdpListing::from_event(sign(developer, 30402, tags)).unwrap()
}

#[test]
fn listing_file_hash_must_be_lowercase() {
    let developer = Keys::generate();
    let lower = listing_with(&developer, vec![tag(&["file_hash", &hex("ab")])]);
    assert_eq!(lower.file_hash, Some(hex("ab")));
    assert!(!lower.malformed_file_hash);

    let upper = listing_with(&developer, vec![tag(&["file_hash", &hex("AB")])]);
    assert_eq!(upper.file_hash, None, "uppercase must not be normalized");
    assert!(upper.malformed_file_hash);
}

#[test]
fn listing_fulfillment_authorization_must_be_lowercase() {
    let developer = Keys::generate();
    let key = Keys::generate().public_key().to_hex();
    let lower = listing_with(
        &developer,
        vec![tag(&["fulfillment_authorization", &hex("ab"), &key])],
    );
    assert_eq!(lower.fulfillment_authorizations.len(), 1);

    for (root, pubkey) in [(hex("AB"), key.clone()), (hex("ab"), key.to_uppercase())] {
        let upper = listing_with(
            &developer,
            vec![tag(&["fulfillment_authorization", &root, &pubkey])],
        );
        assert!(upper.fulfillment_authorizations.is_empty());
        assert_eq!(upper.malformed_fulfillment_authorizations, 1);
    }
}

#[test]
fn listing_campaign_pointer_and_terms_must_be_lowercase() {
    let developer = Keys::generate();
    let operator = Keys::generate().public_key().to_hex();
    let lower = listing_with(
        &developer,
        vec![
            tag(&["campaign", &hex("cd")]),
            tag(&["adp_terms", &operator, &hex("ef")]),
        ],
    );
    assert_eq!(lower.campaign_pointers.len(), 1);
    assert_eq!(lower.accepted_terms.len(), 1);

    let upper = listing_with(
        &developer,
        vec![
            tag(&["campaign", &hex("CD")]),
            tag(&["adp_terms", &operator.to_uppercase(), &hex("ef")]),
            tag(&["adp_terms", &operator, &hex("EF")]),
        ],
    );
    assert!(upper.campaign_pointers.is_empty());
    assert!(upper.accepted_terms.is_empty());
    assert_eq!(upper.malformed_accepted_terms, 2);
}

fn grant(developer: &Keys, source: Option<&str>, authorization: Option<&str>) -> Event {
    let coordinate = format!("30402:{}:game", developer.public_key().to_hex());
    let mut tags = vec![
        tag(&["d", "9c2f1e4a-3b7d-4f21-9a6e-2d8c1f0a5b3e"]),
        tag(&["p", &Keys::generate().public_key().to_hex()]),
        tag(&["a", &coordinate]),
        tag(&["status", "granted"]),
    ];
    if let Some(source) = source {
        tags.push(tag(&["source_event", source]));
    }
    if let Some(authorization) = authorization {
        tags.push(tag(&["authorization", authorization]));
    }
    sign(developer, EXPERIMENTAL_ENTITLEMENT_GRANT_KIND, tags)
}

#[test]
fn grant_source_and_authorization_must_be_lowercase() {
    let developer = Keys::generate();
    assert!(parse_entitlement_event(&grant(&developer, Some(&hex("ab")), None)).is_ok());
    assert!(parse_entitlement_event(&grant(&developer, None, Some(&hex("ab")))).is_ok());
    assert!(parse_entitlement_event(&grant(&developer, Some(&hex("AB")), None)).is_err());
    assert!(parse_entitlement_event(&grant(&developer, None, Some(&hex("AB")))).is_err());
}

#[test]
fn a_uuid_grant_identifier_is_not_a_hex_field() {
    // The grant `d` is an opaque identifier: uppercase or hyphens are allowed.
    let developer = Keys::generate();
    let coordinate = format!("30402:{}:game", developer.public_key().to_hex());
    let event = sign(
        &developer,
        EXPERIMENTAL_ENTITLEMENT_GRANT_KIND,
        vec![
            tag(&["d", "Review-Copy-9C2F1E4A"]),
            tag(&["p", &Keys::generate().public_key().to_hex()]),
            tag(&["a", &coordinate]),
            tag(&["status", "granted"]),
        ],
    );
    assert!(parse_entitlement_event(&event).is_ok());
}

fn authorization(developer: &Keys, fulfillment: &str) -> Event {
    sign(
        developer,
        30406,
        vec![
            tag(&["d", "AUTH-ID-1"]),
            tag(&[
                "a",
                &format!("30402:{}:game", developer.public_key().to_hex()),
            ]),
            tag(&["p", &Keys::generate().public_key().to_hex()]),
            tag(&["fulfillment_pubkey", fulfillment]),
            tag(&["capability", "issue_receipt"]),
            tag(&["valid_from", "1700000000"]),
            tag(&["status", "active"]),
        ],
    )
}

#[test]
fn authorization_fulfillment_pubkey_must_be_lowercase() {
    let developer = Keys::generate();
    let key = Keys::generate().public_key().to_hex();
    assert!(parse_authorization_event(&authorization(&developer, &key)).is_ok());
    assert!(parse_authorization_event(&authorization(&developer, &key.to_uppercase())).is_err());
}

#[test]
fn attestation_fulfillment_pubkey_must_be_lowercase() {
    let operator = Keys::generate();
    let developer = Keys::generate().public_key().to_hex();
    let key = Keys::generate().public_key().to_hex();
    let attestation = |fulfillment: &str| {
        sign(
            &operator,
            30404,
            vec![
                tag(&["d", &format!("{developer}:{key}")]),
                tag(&["p", &developer]),
                tag(&["fulfillment_pubkey", fulfillment, "1700000000", ""]),
            ],
        )
    };
    assert!(parse_attestation_event(&attestation(&key)).is_ok());
    assert!(parse_attestation_event(&attestation(&key.to_uppercase())).is_err());
}

fn receipt(signer: &Keys, extra: Vec<Tag>) -> Event {
    let coordinate = format!("30402:{}:game", signer.public_key().to_hex());
    let mut tags = vec![
        tag(&["order", "ORDER-1"]),
        tag(&["p", &Keys::generate().public_key().to_hex()]),
        tag(&["a", &coordinate]),
        tag(&["payment_hash", &hex("11")]),
        tag(&["amount_msat", "1000"]),
        tag(&["settled_at", "1700000000"]),
        tag(&["status", "paid"]),
    ];
    tags.extend(extra);
    sign(signer, 1020, tags)
}

#[test]
fn receipt_authorization_and_zap_proof_must_be_lowercase() {
    let signer = Keys::generate();
    let preimage_proof = tag(&["proof", "bolt11-preimage"]);
    assert!(parse_receipt_event(&receipt(
        &signer,
        vec![preimage_proof.clone(), tag(&["authorization", &hex("ab")])]
    ))
    .is_ok());
    assert!(parse_receipt_event(&receipt(
        &signer,
        vec![preimage_proof, tag(&["authorization", &hex("AB")])]
    ))
    .is_err());
    assert!(
        parse_receipt_event(&receipt(&signer, vec![tag(&["proof", "zap", &hex("ab")])])).is_ok()
    );
    assert!(
        parse_receipt_event(&receipt(&signer, vec![tag(&["proof", "zap", &hex("AB")])])).is_err()
    );
}

#[test]
fn receipt_payment_hash_must_be_lowercase() {
    let signer = Keys::generate();
    let coordinate = format!("30402:{}:game", signer.public_key().to_hex());
    let event = sign(
        &signer,
        1020,
        vec![
            tag(&["order", "ORDER-1"]),
            tag(&["p", &Keys::generate().public_key().to_hex()]),
            tag(&["a", &coordinate]),
            tag(&["payment_hash", &hex("AA")]),
            tag(&["amount_msat", "1000"]),
            tag(&["settled_at", "1700000000"]),
            tag(&["proof", "bolt11-preimage"]),
            tag(&["status", "paid"]),
        ],
    );
    assert!(parse_receipt_event(&event).is_err());
}
