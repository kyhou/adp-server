use adp_core::provisioning::build_acceptance_event_builder;
use adp_core::{
    build_authorization_revocation_event_builder, build_authorization_root_event_builder,
    parse_authorization_event, resolve_authorization, AdpError, AuthorizationEvent,
    AuthorizationTerms, AuthorizationTransition, ChainResolutionError, ResolvedAuthorization,
    ACCEPTANCE_KIND,
};
use nostr::{Event, EventBuilder, EventId, Keys, Kind, PublicKey, Tag, Timestamp};

fn tag(values: Vec<&str>) -> Tag {
    Tag::parse(values).expect("valid tag")
}

fn coordinate(developer: &Keys, game_id: &str) -> String {
    format!("30402:{}:{game_id}", developer.public_key())
}

fn terms(developer: &Keys, fulfillment: &Keys) -> AuthorizationTerms {
    AuthorizationTerms {
        authorization_id: "authorization-1".into(),
        coordinate: coordinate(developer, "game-1"),
        fulfillment_pubkey: fulfillment.public_key(),
        valid_from: 100,
        valid_until: Some(200),
    }
}

fn authorization_tags(
    transition_d: &str,
    terms: &AuthorizationTerms,
    status: &str,
    predecessor: Option<EventId>,
) -> Vec<Tag> {
    let mut tags = vec![
        tag(vec!["d", transition_d]),
        tag(vec!["authorization_id", &terms.authorization_id]),
        tag(vec!["a", &terms.coordinate]),
        tag(vec!["p", &terms.fulfillment_pubkey.to_hex()]),
        tag(vec!["status", status]),
    ];
    if status == "active" {
        tags.push(tag(vec!["valid_from", &terms.valid_from.to_string()]));
        if let Some(valid_until) = terms.valid_until {
            tags.push(tag(vec!["valid_until", &valid_until.to_string()]));
        }
    }
    if let Some(predecessor) = predecessor {
        tags.push(tag(vec!["e", &predecessor.to_hex()]));
    }
    tags
}

fn signed_authorization(
    signer: &Keys,
    transition_d: &str,
    terms: &AuthorizationTerms,
    status: &str,
    predecessor: Option<EventId>,
    created_at: u64,
    extra_tags: Vec<Tag>,
) -> Event {
    let mut tags = authorization_tags(transition_d, terms, status, predecessor);
    tags.extend(extra_tags);
    EventBuilder::new(Kind::Custom(ACCEPTANCE_KIND), "")
        .tags(tags)
        .custom_created_at(Timestamp::from_secs(created_at))
        .sign_with_keys(signer)
        .expect("sign authorization")
}

fn root_event(developer: &Keys, terms: &AuthorizationTerms, d: &str, at: u64) -> Event {
    signed_authorization(developer, d, terms, "active", None, at, vec![])
}

fn parsed_root(developer: &Keys, terms: &AuthorizationTerms) -> AuthorizationEvent {
    parse_authorization_event(&root_event(developer, terms, "transition-root", 90)).unwrap()
}

#[test]
fn valid_active_root_round_trips_and_resolves() {
    let developer = Keys::generate();
    let fulfillment = Keys::generate();
    let terms = terms(&developer, &fulfillment);
    let event = build_authorization_root_event_builder(&terms, "transition-root")
        .unwrap()
        .custom_created_at(Timestamp::from_secs(90))
        .sign_with_keys(&developer)
        .unwrap();
    let parsed = parse_authorization_event(&event).unwrap();

    assert_eq!(parsed.transition_d, "transition-root");
    assert_eq!(parsed.authorization_id, terms.authorization_id);
    assert_eq!(parsed.coordinate, terms.coordinate);
    assert_eq!(parsed.fulfillment_pubkey, terms.fulfillment_pubkey);
    assert_eq!(parsed.predecessor, None);
    assert_eq!(
        parsed.transition,
        AuthorizationTransition::ActiveRoot(terms.clone())
    );

    let resolved = resolve_authorization(event.id, &[parsed]).unwrap();
    assert_eq!(resolved.root_event_id, event.id);
    assert_eq!(resolved.developer_pubkey, developer.public_key());
    assert_eq!(resolved.terms, terms);
    assert_eq!(resolved.revoked_at, None);
}

#[test]
fn transition_d_is_separate_from_stable_authorization_id() {
    let developer = Keys::generate();
    let fulfillment = Keys::generate();
    let terms = terms(&developer, &fulfillment);
    let root = build_authorization_root_event_builder(&terms, "root-d")
        .unwrap()
        .custom_created_at(Timestamp::from_secs(90))
        .sign_with_keys(&developer)
        .unwrap();
    let revoke = build_authorization_revocation_event_builder(&terms, "revoke-d", root.id)
        .unwrap()
        .custom_created_at(Timestamp::from_secs(150))
        .sign_with_keys(&developer)
        .unwrap();

    let root = parse_authorization_event(&root).unwrap();
    let revoke = parse_authorization_event(&revoke).unwrap();
    assert_eq!(root.authorization_id, revoke.authorization_id);
    assert_ne!(root.transition_d, revoke.transition_d);
}

#[test]
fn root_rejects_missing_duplicate_malformed_and_forbidden_tags() {
    let developer = Keys::generate();
    let fulfillment = Keys::generate();
    let terms = terms(&developer, &fulfillment);
    let base = authorization_tags("root", &terms, "active", None);

    for required in ["d", "authorization_id", "a", "p", "valid_from", "status"] {
        let tags: Vec<Tag> = base
            .iter()
            .filter(|candidate| (*candidate).clone().to_vec()[0] != required)
            .cloned()
            .collect();
        let event = EventBuilder::new(Kind::Custom(ACCEPTANCE_KIND), "")
            .tags(tags)
            .sign_with_keys(&developer)
            .unwrap();
        assert!(
            parse_authorization_event(&event).is_err(),
            "missing {required}"
        );
    }

    for duplicate in [
        "d",
        "authorization_id",
        "a",
        "p",
        "valid_from",
        "valid_until",
        "status",
    ] {
        let duplicate_tag = base
            .iter()
            .find(|candidate| (*candidate).clone().to_vec()[0] == duplicate)
            .unwrap()
            .clone();
        let event = signed_authorization(
            &developer,
            "root",
            &terms,
            "active",
            None,
            90,
            vec![duplicate_tag],
        );
        assert!(matches!(
            parse_authorization_event(&event),
            Err(AdpError::DuplicateTag(_))
        ));
    }

    let empty_id = AuthorizationTerms {
        authorization_id: String::new(),
        ..terms.clone()
    };
    assert!(parse_authorization_event(&root_event(&developer, &empty_id, "root", 90)).is_err());
    assert!(parse_authorization_event(&signed_authorization(
        &developer,
        "root",
        &terms,
        "active",
        None,
        90,
        vec![tag(vec!["e", &EventId::all_zeros().to_hex()])],
    ))
    .is_err());
    assert!(parse_authorization_event(&signed_authorization(
        &developer,
        "root",
        &terms,
        "active",
        None,
        90,
        vec![tag(vec!["unexpected", "value"])],
    ))
    .is_err());
}

#[test]
fn root_requires_coordinate_developer_to_sign() {
    let developer = Keys::generate();
    let other = Keys::generate();
    let fulfillment = Keys::generate();
    let terms = terms(&developer, &fulfillment);

    assert!(matches!(
        parse_authorization_event(&root_event(&other, &terms, "root", 90)),
        Err(AdpError::CoordinateMismatch { .. })
    ));
}

#[test]
fn root_rejects_wrong_coordinate_shape_and_invalid_signature() {
    let developer = Keys::generate();
    let fulfillment = Keys::generate();
    let mut invalid_terms = terms(&developer, &fulfillment);
    invalid_terms.coordinate = format!("30403:{}:game-1", developer.public_key());
    assert!(
        parse_authorization_event(&root_event(&developer, &invalid_terms, "root", 90)).is_err()
    );

    let event = root_event(&developer, &terms(&developer, &fulfillment), "root", 90);
    let mut json = serde_json::to_value(event).unwrap();
    json["content"] = serde_json::Value::String("tampered".into());
    let event: Event = serde_json::from_value(json).unwrap();
    assert!(matches!(
        parse_authorization_event(&event),
        Err(AdpError::InvalidSignature)
    ));
}

#[test]
fn root_rejects_malformed_fulfillment_key_and_validity_interval() {
    let developer = Keys::generate();
    let fulfillment = Keys::generate();
    let terms = terms(&developer, &fulfillment);
    let mut malformed_p = authorization_tags("root", &terms, "active", None);
    *malformed_p
        .iter_mut()
        .find(|candidate| (**candidate).clone().to_vec()[0] == "p")
        .unwrap() = tag(vec!["p", "not-a-key"]);
    let event = EventBuilder::new(Kind::Custom(ACCEPTANCE_KIND), "")
        .tags(malformed_p)
        .sign_with_keys(&developer)
        .unwrap();
    assert!(parse_authorization_event(&event).is_err());

    for valid_until in [Some(100), Some(99)] {
        let invalid = AuthorizationTerms {
            valid_until,
            ..terms.clone()
        };
        assert!(parse_authorization_event(&root_event(&developer, &invalid, "root", 90)).is_err());
    }
}

#[test]
fn authorization_uses_half_open_validity_interval() {
    let developer = Keys::generate();
    let fulfillment = Keys::generate();
    let terms = terms(&developer, &fulfillment);
    let root = parsed_root(&developer, &terms);
    let resolved = resolve_authorization(root.event.id, std::slice::from_ref(&root)).unwrap();

    assert!(!resolved.authorizes(&fulfillment.public_key(), &terms.coordinate, 99));
    assert!(resolved.authorizes(&fulfillment.public_key(), &terms.coordinate, 100));
    assert!(resolved.authorizes(&fulfillment.public_key(), &terms.coordinate, 199));
    assert!(!resolved.authorizes(&fulfillment.public_key(), &terms.coordinate, 200));
    assert!(!resolved.authorizes(&Keys::generate().public_key(), &terms.coordinate, 150));
    assert!(!resolved.authorizes(&fulfillment.public_key(), "30402:wrong:game", 150));
}

#[test]
fn authorization_without_valid_until_remains_active() {
    let developer = Keys::generate();
    let fulfillment = Keys::generate();
    let terms = AuthorizationTerms {
        valid_until: None,
        ..terms(&developer, &fulfillment)
    };
    let root = parsed_root(&developer, &terms);
    let resolved = resolve_authorization(root.event.id, std::slice::from_ref(&root)).unwrap();

    assert!(resolved.authorizes(&fulfillment.public_key(), &terms.coordinate, u64::MAX));
}

#[test]
fn terminal_revocation_is_effective_at_its_timestamp() {
    let developer = Keys::generate();
    let fulfillment = Keys::generate();
    let terms = terms(&developer, &fulfillment);
    let root_event = root_event(&developer, &terms, "root", 90);
    let revoke_event = signed_authorization(
        &developer,
        "revoke",
        &terms,
        "revoked",
        Some(root_event.id),
        150,
        vec![],
    );
    let root = parse_authorization_event(&root_event).unwrap();
    let revoke = parse_authorization_event(&revoke_event).unwrap();
    let resolved = resolve_authorization(root_event.id, &[revoke, root]).unwrap();

    assert_eq!(resolved.revoked_at, Some(150));
    assert!(resolved.authorizes(&fulfillment.public_key(), &terms.coordinate, 149));
    assert!(!resolved.authorizes(&fulfillment.public_key(), &terms.coordinate, 150));
}

#[test]
fn revocation_has_exact_shape_and_is_terminal() {
    let developer = Keys::generate();
    let fulfillment = Keys::generate();
    let terms = terms(&developer, &fulfillment);
    let root_event = root_event(&developer, &terms, "root", 90);

    let missing_predecessor =
        signed_authorization(&developer, "revoke", &terms, "revoked", None, 150, vec![]);
    assert!(parse_authorization_event(&missing_predecessor).is_err());

    let with_validity = signed_authorization(
        &developer,
        "revoke",
        &terms,
        "revoked",
        Some(root_event.id),
        150,
        vec![tag(vec!["valid_from", "100"])],
    );
    assert!(parse_authorization_event(&with_validity).is_err());

    let revoke_event = signed_authorization(
        &developer,
        "revoke",
        &terms,
        "revoked",
        Some(root_event.id),
        150,
        vec![],
    );
    let after_revoke = signed_authorization(
        &developer,
        "later",
        &terms,
        "revoked",
        Some(revoke_event.id),
        160,
        vec![],
    );
    let nodes = [&root_event, &revoke_event, &after_revoke]
        .into_iter()
        .map(|event| parse_authorization_event(event).unwrap())
        .collect::<Vec<_>>();
    assert!(matches!(
        resolve_authorization(root_event.id, &nodes),
        Err(ChainResolutionError::InvalidTransition(_))
    ));
}

#[test]
fn resolver_requires_strict_timestamps_and_unique_transition_d() {
    let developer = Keys::generate();
    let fulfillment = Keys::generate();
    let terms = terms(&developer, &fulfillment);
    let root_event = root_event(&developer, &terms, "same-d", 100);
    for created_at in [99, 100] {
        let revoke = signed_authorization(
            &developer,
            "revoke",
            &terms,
            "revoked",
            Some(root_event.id),
            created_at,
            vec![],
        );
        let nodes = vec![
            parse_authorization_event(&root_event).unwrap(),
            parse_authorization_event(&revoke).unwrap(),
        ];
        assert!(matches!(
            resolve_authorization(root_event.id, &nodes),
            Err(ChainResolutionError::TimestampRegression)
        ));
    }

    let revoke = signed_authorization(
        &developer,
        "same-d",
        &terms,
        "revoked",
        Some(root_event.id),
        150,
        vec![],
    );
    let nodes = vec![
        parse_authorization_event(&root_event).unwrap(),
        parse_authorization_event(&revoke).unwrap(),
    ];
    assert!(matches!(
        resolve_authorization(root_event.id, &nodes),
        Err(ChainResolutionError::InvalidTransition(_))
    ));
}

#[test]
fn resolver_rejects_missing_predecessor_fork_and_disconnected_root() {
    let developer = Keys::generate();
    let fulfillment = Keys::generate();
    let terms = terms(&developer, &fulfillment);
    let root = root_event(&developer, &terms, "root", 90);
    let missing = signed_authorization(
        &developer,
        "missing",
        &terms,
        "revoked",
        Some(EventId::all_zeros()),
        150,
        vec![],
    );
    let nodes = [&root, &missing]
        .into_iter()
        .map(|event| parse_authorization_event(event).unwrap())
        .collect::<Vec<_>>();
    assert!(matches!(
        resolve_authorization(root.id, &nodes),
        Err(ChainResolutionError::MissingPredecessor(_))
    ));

    let first = signed_authorization(
        &developer,
        "first",
        &terms,
        "revoked",
        Some(root.id),
        150,
        vec![],
    );
    let second = signed_authorization(
        &developer,
        "second",
        &terms,
        "revoked",
        Some(root.id),
        160,
        vec![],
    );
    let nodes = [&root, &first, &second]
        .into_iter()
        .map(|event| parse_authorization_event(event).unwrap())
        .collect::<Vec<_>>();
    assert!(matches!(
        resolve_authorization(root.id, &nodes),
        Err(ChainResolutionError::Fork(_))
    ));

    let other_root = root_event(&developer, &terms, "other-root", 91);
    let nodes = [&root, &other_root]
        .into_iter()
        .map(|event| parse_authorization_event(event).unwrap())
        .collect::<Vec<_>>();
    assert!(matches!(
        resolve_authorization(root.id, &nodes),
        Err(ChainResolutionError::MultipleRoots)
    ));
}

#[test]
fn resolver_rejects_wrong_expected_root_and_invariant_mutations() {
    let developer = Keys::generate();
    let other = Keys::generate();
    let fulfillment = Keys::generate();
    let terms = terms(&developer, &fulfillment);
    let root_event = root_event(&developer, &terms, "root", 90);
    let root = parse_authorization_event(&root_event).unwrap();
    assert!(resolve_authorization(EventId::all_zeros(), std::slice::from_ref(&root)).is_err());

    for mutation in ["authorization_id", "a", "p"] {
        let mut changed = terms.clone();
        match mutation {
            "authorization_id" => changed.authorization_id = "other-id".into(),
            "a" => changed.coordinate = coordinate(&developer, "other-game"),
            "p" => changed.fulfillment_pubkey = other.public_key(),
            _ => unreachable!(),
        }
        let successor = signed_authorization(
            &developer,
            mutation,
            &changed,
            "revoked",
            Some(root_event.id),
            150,
            vec![],
        );
        let nodes = vec![root.clone(), parse_authorization_event(&successor).unwrap()];
        assert!(matches!(
            resolve_authorization(root_event.id, &nodes),
            Err(ChainResolutionError::InvariantMutation(_))
        ));
    }
}

#[test]
fn resolver_reparses_signed_events_and_effective_check_is_panic_free() {
    let developer = Keys::generate();
    let fulfillment = Keys::generate();
    let terms = terms(&developer, &fulfillment);
    let mut root = parsed_root(&developer, &terms);
    let root_id = root.event.id;
    root.authorization_id = "forged".into();
    assert!(matches!(
        resolve_authorization(root_id, &[root.clone()]),
        Err(ChainResolutionError::InvariantMutation(
            "signed event metadata"
        ))
    ));

    let malformed = ResolvedAuthorization {
        root_event_id: root_id,
        developer_pubkey: developer.public_key(),
        terms: terms.clone(),
        events: vec![root],
        revoked_at: None,
    };
    assert!(!malformed.authorizes(&fulfillment.public_key(), &terms.coordinate, 150));
}

#[test]
fn old_acceptance_shape_is_not_an_authorization_root() {
    let developer = Keys::generate();
    let operator = Keys::generate();
    let fulfillment = Keys::generate();
    let event = build_acceptance_event_builder(&operator.public_key(), &fulfillment.public_key())
        .unwrap()
        .sign_with_keys(&developer)
        .unwrap();

    assert!(parse_authorization_event(&event).is_err());
}

#[test]
fn parser_rejects_self_predecessor_encoded_in_signed_event_metadata() {
    let developer = Keys::generate();
    let fulfillment = Keys::generate();
    let terms = terms(&developer, &fulfillment);
    let root = parsed_root(&developer, &terms);
    let mut forged = root.clone();
    forged.predecessor = Some(forged.event.id);

    assert!(resolve_authorization(root.event.id, &[forged]).is_err());
}

fn _public_key_type_is_part_of_authorizes_api(_: PublicKey) {}
