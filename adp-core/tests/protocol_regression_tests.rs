use adp_core::{latest_status_in_chain, parse_receipt_event, verify_zap_receipt, AdpListing};
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

    assert!(verify_zap_receipt(&zap, &buyer.public_key().to_hex(), coordinate).is_ok());
}
