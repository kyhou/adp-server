use adp_core::{
    build_aggregate_receipt_event_builder, parse_receipt_event, AdpListing, AggregateProofLeg,
    BuildAggregateReceiptParams, PaymentRole,
};
use nostr::{EventBuilder, EventId, Keys, Kind, Tag};

fn tag(values: &[&str]) -> Tag {
    Tag::parse(
        values
            .iter()
            .map(|value| (*value).to_string())
            .collect::<Vec<_>>(),
    )
    .expect("tag")
}

fn listing(developer: &Keys) -> AdpListing {
    let event = EventBuilder::new(Kind::Custom(30402), "")
        .tags([tag(&["d", "game"]), tag(&["price", "1", "SATS"])])
        .sign_with_keys(developer)
        .expect("listing");
    AdpListing::from_event(event).expect("parse listing")
}

#[test]
fn aggregate_receipt_round_trips_and_rejects_singular_tags() {
    let developer = Keys::generate();
    let buyer = Keys::generate();
    let processor = Keys::generate();
    let signer = Keys::generate();
    let listing = listing(&developer);
    let legs = [AggregateProofLeg {
        leg_id: "developer".into(),
        role: PaymentRole::Developer,
        recipient_pubkey: developer.public_key().to_hex(),
        amount_msat: 1_000,
        payment_hash: "11".repeat(32),
        proof_type: "bolt11-preimage".into(),
        proof_reference: "proof-commitment".into(),
        accepted_terms_id: None,
    }];
    let params = BuildAggregateReceiptParams {
        listing: &listing,
        buyer_pubkey: buyer.public_key(),
        authorization: None,
        quote_event_id: EventId::from_hex(&"22".repeat(32)).expect("event id"),
        processor_pubkey: processor.public_key(),
        processor_url: Some("https://processor.example"),
        gross_msat: 1_000,
        settled_at: 123,
        legs: &legs,
    };
    let event = build_aggregate_receipt_event_builder("order", &params, "ciphertext".into())
        .expect("builder")
        .sign_with_keys(&signer)
        .expect("receipt");
    let parsed = parse_receipt_event(&event).expect("aggregate receipt");
    assert_eq!(parsed.aggregate.expect("aggregate").legs, legs);

    let mut tags = event.tags.to_vec();
    tags.push(tag(&["amount_msat", "1000"]));
    let mixed = EventBuilder::new(Kind::Custom(1020), "ciphertext")
        .tags(tags)
        .sign_with_keys(&signer)
        .expect("mixed receipt");
    assert!(parse_receipt_event(&mixed).is_err());
}
