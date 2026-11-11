use adp_core::verify_nip98;
use nostr::{EventBuilder, Keys, Kind, Tag, Timestamp};

#[test]
fn nip98_rejects_trailing_slash_mismatch() {
    let keys = Keys::generate();
    let signed = EventBuilder::new(Kind::Custom(27235), "")
        .tags([
            Tag::parse(["u", "https://adp.example/provision/"]).expect("u"),
            Tag::parse(["method", "POST"]).expect("method"),
        ])
        .sign_with_keys(&keys)
        .expect("signed auth");
    assert!(
        verify_nip98(&signed, "POST", "https://adp.example/provision").is_err(),
        "a different raw target is not authorized"
    );
}

#[test]
fn nip98_retains_signature_kind_timestamp_and_method_checks() {
    let keys = Keys::generate();
    let url = "http://localhost:8080/game/test";
    for (kind, created_at, method, tamper, valid) in [
        (27235, Timestamp::now(), "GET", false, true),
        (27235, Timestamp::now(), "get", false, true),
        (27235, Timestamp::now(), "POST", false, false),
        (1, Timestamp::now(), "GET", false, false),
        (27235, Timestamp::from_secs(1), "GET", false, false),
        (27235, Timestamp::now(), "GET", true, false),
    ] {
        let mut event = EventBuilder::new(Kind::Custom(kind), "")
            .tags([
                Tag::parse(["u", url]).expect("url tag"),
                Tag::parse(["method", method]).expect("method tag"),
            ])
            .custom_created_at(created_at)
            .sign_with_keys(&keys)
            .expect("event");
        if tamper {
            event.content = "tampered".into();
        }
        assert_eq!(verify_nip98(&event, "GET", url).is_ok(), valid);
    }
}

#[test]
fn nip98_shared_url_comparison_is_exact_and_retains_first_value() {
    let keys = Keys::generate();
    let url = "http://localhost:8080/game/test?x=value";
    for (first, second, valid) in [
        (url.to_string(), "http://wrong.example".to_string(), true),
        (format!("{url}/"), "http://wrong.example".to_string(), false),
        ("http://wrong.example".to_string(), url.to_string(), false),
        (url.replace("value", "%76alue"), url.to_string(), false),
    ] {
        let event = EventBuilder::new(Kind::Custom(27235), "")
            .tags([
                Tag::parse(["u", &first]).expect("first url"),
                Tag::parse(["u", &second]).expect("second url"),
                Tag::parse(["method", "GET"]).expect("method"),
            ])
            .sign_with_keys(&keys)
            .expect("event");
        assert_eq!(verify_nip98(&event, "GET", url).is_ok(), valid);
    }
}
