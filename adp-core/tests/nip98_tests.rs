use adp_core::verify_nip98;
use nostr::{EventBuilder, Keys, Kind, Tag, Timestamp};

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
fn nip98_shared_url_comparison_retains_first_value_and_slash_tolerance() {
    let keys = Keys::generate();
    let url = "http://localhost:8080/game/test?x=value";
    for (first, second, valid) in [
        (format!("{url}/"), "http://wrong.example".to_string(), true),
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
