use adp_core::ADP_VERSION;
use adp_server::announcement::build_server_announcement_event;

#[test]
fn server_announcement_is_kind_30403_signed_by_operator() {
    let operator = nostr_sdk::Keys::generate();
    let event = build_server_announcement_event(
        &operator,
        "adp-reference-server",
        "ADP Reference Server",
        "https://dist.example.com",
        "ops@example.com",
    )
    .expect("build announcement");

    assert_eq!(event.kind.as_u16(), 30403);
    assert_eq!(event.pubkey, operator.public_key());
    assert!(event.verify().is_ok());
    let tags: Vec<Vec<String>> = event.tags.iter().map(|tag| tag.clone().to_vec()).collect();
    assert!(tags.contains(&vec!["d".into(), "adp-reference-server".into()]));
    assert!(tags.contains(&vec!["url".into(), "https://dist.example.com".into()]));
    assert!(tags.contains(&vec!["supported_adp".into(), ADP_VERSION.into()]));
}
