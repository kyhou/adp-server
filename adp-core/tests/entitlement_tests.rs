use adp_core::campaign::{parse_campaign_event, resolve_campaign};
use adp_core::{
    build_entitlement_event_builder, group_parsed_entitlements, parse_authorization_event,
    parse_entitlement_event, resolve_authorization,
    resolve_entitlement_grant as resolve_entitlement, verify_adp_entitlement, AdpError,
    AuthorizationTerms, BuildEntitlementParams, ChainResolutionError, EntitlementResolutionError,
    GrantStatus, ParsedEntitlementGrant, ResolvedAuthorization, ResolvedEntitlementGrant,
    ACCEPTANCE_KIND, EXPERIMENTAL_ADP_CAMPAIGN_KIND, EXPERIMENTAL_ENTITLEMENT_GRANT_KIND,
};
use nostr::{Event, EventBuilder, EventId, Keys, Kind, Tag, Timestamp};

fn tag(values: Vec<&str>) -> Tag {
    Tag::parse(values).expect("valid tag")
}

fn coordinate(developer: &Keys, resource: &str) -> String {
    format!("30402:{}:{resource}", developer.public_key())
}

fn params(recipient: &Keys, coordinate: &str) -> BuildEntitlementParams {
    BuildEntitlementParams {
        grant_id: "grant-1".into(),
        recipient: recipient.public_key(),
        coordinate: coordinate.into(),
        source_event: None,
        authorization_event: None,
        reason: None,
        status: GrantStatus::Granted,
        predecessor: None,
    }
}

#[allow(clippy::too_many_arguments)]
fn signed_entitlement(
    signer: &Keys,
    grant_id: &str,
    recipient: &Keys,
    coordinate: &str,
    status: &str,
    predecessor: Option<EventId>,
    source_event: Option<EventId>,
    authorization_event: Option<EventId>,
    reason: Option<&str>,
    created_at: u64,
    extra_tags: Vec<Tag>,
) -> Event {
    let mut tags = vec![
        tag(vec!["d", grant_id]),
        tag(vec!["p", &recipient.public_key().to_hex()]),
        tag(vec!["a", coordinate]),
        tag(vec!["status", status]),
    ];
    if let Some(predecessor) = predecessor {
        tags.push(tag(vec!["e", &predecessor.to_hex()]));
    }
    if let Some(source_event) = source_event {
        tags.push(tag(vec!["source_event", &source_event.to_hex()]));
    }
    if let Some(authorization_event) = authorization_event {
        tags.push(tag(vec![
            "authorization_event",
            &authorization_event.to_hex(),
        ]));
    }
    if let Some(reason) = reason {
        tags.push(tag(vec!["reason", reason]));
    }
    tags.extend(extra_tags);

    EventBuilder::new(Kind::Custom(EXPERIMENTAL_ENTITLEMENT_GRANT_KIND), "")
        .tags(tags)
        .custom_created_at(Timestamp::from_secs(created_at))
        .sign_with_keys(signer)
        .expect("sign entitlement")
}

fn root(signer: &Keys, recipient: &Keys, coordinate: &str, created_at: u64) -> Event {
    signed_entitlement(
        signer,
        "grant-1",
        recipient,
        coordinate,
        "granted",
        None,
        None,
        None,
        None,
        created_at,
        vec![],
    )
}

fn revoke(
    signer: &Keys,
    recipient: &Keys,
    coordinate: &str,
    predecessor: EventId,
    created_at: u64,
) -> Event {
    signed_entitlement(
        signer,
        "grant-1",
        recipient,
        coordinate,
        "revoked",
        Some(predecessor),
        None,
        None,
        None,
        created_at,
        vec![],
    )
}

fn resolve_chain(
    nodes: &[ParsedEntitlementGrant],
) -> Result<ResolvedEntitlementGrant, ChainResolutionError> {
    resolve_entitlement(nodes).map_err(|error| match error {
        EntitlementResolutionError::Chain(error) => error,
        EntitlementResolutionError::Parse(error) => panic!("unexpected parse error: {error}"),
    })
}

fn parse_events(events: &[Event]) -> Vec<ParsedEntitlementGrant> {
    events
        .iter()
        .map(|event| parse_entitlement_event(event).unwrap())
        .collect()
}

#[test]
fn generic_builder_and_parser_use_kind_1030() {
    let issuer = Keys::generate();
    let recipient = Keys::generate();
    let coordinate = coordinate(&issuer, "game");
    let source_event = root(&issuer, &recipient, &coordinate, 1).id;
    let authorization_event = root(&issuer, &recipient, &coordinate, 2).id;
    let mut input = params(&recipient, &coordinate);
    input.source_event = Some(source_event);
    input.authorization_event = Some(authorization_event);
    input.reason = Some("review-copy".into());

    let event = build_entitlement_event_builder(&input)
        .unwrap()
        .custom_created_at(Timestamp::from_secs(100))
        .sign_with_keys(&issuer)
        .unwrap();
    let parsed = parse_entitlement_event(&event).unwrap();

    assert_eq!(event.kind.as_u16(), EXPERIMENTAL_ENTITLEMENT_GRANT_KIND);
    assert_eq!(parsed.event, event);
    assert_eq!(parsed.grant_id, "grant-1");
    assert_eq!(parsed.recipient, recipient.public_key());
    assert_eq!(parsed.coordinate, coordinate);
    assert_eq!(parsed.source_event, Some(source_event));
    assert_eq!(parsed.authorization_event, Some(authorization_event));
    assert_eq!(parsed.reason.as_deref(), Some("review-copy"));
    assert_eq!(parsed.status, GrantStatus::Granted);
    assert_eq!(parsed.predecessor, None);
}

#[test]
fn generic_valid_revoke_update_allows_a_different_signer() {
    let issuer = Keys::generate();
    let revoker = Keys::generate();
    let recipient = Keys::generate();
    let coordinate = coordinate(&issuer, "game");
    let root = root(&issuer, &recipient, &coordinate, 100);
    let mut input = params(&recipient, &coordinate);
    input.status = GrantStatus::Revoked;
    input.predecessor = Some(root.id);

    let update = build_entitlement_event_builder(&input)
        .unwrap()
        .custom_created_at(Timestamp::from_secs(101))
        .sign_with_keys(&revoker)
        .unwrap();
    let parsed = parse_entitlement_event(&update).unwrap();

    assert_eq!(parsed.status, GrantStatus::Revoked);
    assert_eq!(parsed.predecessor, Some(root.id));
    assert_eq!(parsed.event.pubkey, revoker.public_key());
}

#[test]
fn generic_required_tags_have_exact_cardinality() {
    let issuer = Keys::generate();
    let recipient = Keys::generate();
    let coordinate = coordinate(&issuer, "game");

    for missing in ["d", "p", "a", "status"] {
        let tags = [
            ("d", tag(vec!["d", "grant-1"])),
            ("p", tag(vec!["p", &recipient.public_key().to_hex()])),
            ("a", tag(vec!["a", &coordinate])),
            ("status", tag(vec!["status", "granted"])),
        ]
        .into_iter()
        .filter_map(|(name, tag)| (name != missing).then_some(tag))
        .collect::<Vec<_>>();
        let event = EventBuilder::new(Kind::Custom(EXPERIMENTAL_ENTITLEMENT_GRANT_KIND), "")
            .tags(tags)
            .sign_with_keys(&issuer)
            .unwrap();
        assert!(matches!(
            parse_entitlement_event(&event),
            Err(AdpError::MissingTag(found)) if found == missing
        ));
    }

    for (name, duplicate) in [
        ("d", tag(vec!["d", "grant-1"])),
        ("p", tag(vec!["p", &recipient.public_key().to_hex()])),
        ("a", tag(vec!["a", &coordinate])),
        ("status", tag(vec!["status", "granted"])),
    ] {
        let event = signed_entitlement(
            &issuer,
            "grant-1",
            &recipient,
            &coordinate,
            "granted",
            None,
            None,
            None,
            None,
            100,
            vec![duplicate],
        );
        assert!(matches!(
            parse_entitlement_event(&event),
            Err(AdpError::DuplicateTag(ref found)) if found == name
        ));
    }
}

#[test]
fn generic_optional_tags_have_at_most_one_value() {
    let issuer = Keys::generate();
    let recipient = Keys::generate();
    let coordinate = coordinate(&issuer, "game");
    let anchor = root(&issuer, &recipient, &coordinate, 1).id;

    for (name, duplicate) in [
        ("source_event", tag(vec!["source_event", &anchor.to_hex()])),
        (
            "authorization_event",
            tag(vec!["authorization_event", &anchor.to_hex()]),
        ),
        ("reason", tag(vec!["reason", "duplicate"])),
    ] {
        let event = signed_entitlement(
            &issuer,
            "grant-1",
            &recipient,
            &coordinate,
            "granted",
            None,
            (name == "source_event").then_some(anchor),
            (name == "authorization_event").then_some(anchor),
            (name == "reason").then_some("first"),
            100,
            vec![duplicate],
        );
        assert!(matches!(
            parse_entitlement_event(&event),
            Err(AdpError::DuplicateTag(ref found)) if found == name
        ));
    }
}

#[test]
fn generic_parser_rejects_invalid_status_and_predecessor_shapes() {
    let issuer = Keys::generate();
    let recipient = Keys::generate();
    let coordinate = coordinate(&issuer, "game");
    let predecessor = root(&issuer, &recipient, &coordinate, 1).id;

    let invalid_status = signed_entitlement(
        &issuer,
        "grant-1",
        &recipient,
        &coordinate,
        "pending",
        None,
        None,
        None,
        None,
        100,
        vec![],
    );
    assert!(matches!(
        parse_entitlement_event(&invalid_status),
        Err(AdpError::MalformedTag(ref found)) if found == "status"
    ));

    let root_with_predecessor = signed_entitlement(
        &issuer,
        "grant-1",
        &recipient,
        &coordinate,
        "granted",
        Some(predecessor),
        None,
        None,
        None,
        100,
        vec![],
    );
    assert!(matches!(
        parse_entitlement_event(&root_with_predecessor),
        Err(AdpError::ForbiddenTag(ref found)) if found == "e"
    ));

    let update_without_predecessor = signed_entitlement(
        &issuer,
        "grant-1",
        &recipient,
        &coordinate,
        "revoked",
        None,
        None,
        None,
        None,
        100,
        vec![],
    );
    assert!(matches!(
        parse_entitlement_event(&update_without_predecessor),
        Err(AdpError::MissingTag("e"))
    ));

    let update_with_two_predecessors = signed_entitlement(
        &issuer,
        "grant-1",
        &recipient,
        &coordinate,
        "revoked",
        Some(predecessor),
        None,
        None,
        None,
        100,
        vec![tag(vec!["e", &predecessor.to_hex()])],
    );
    assert!(matches!(
        parse_entitlement_event(&update_with_two_predecessors),
        Err(AdpError::DuplicateTag(ref found)) if found == "e"
    ));
}

#[test]
fn generic_builder_and_parser_accept_addressable_identifiers_containing_colons() {
    let issuer = Keys::generate();
    let recipient = Keys::generate();
    let resource_coordinate = format!("30023:{}:article:section", issuer.public_key());
    let input = params(&recipient, &resource_coordinate);
    let event = build_entitlement_event_builder(&input)
        .unwrap()
        .sign_with_keys(&issuer)
        .unwrap();

    assert_eq!(
        parse_entitlement_event(&event).unwrap().coordinate,
        resource_coordinate
    );
}

#[test]
fn generic_builder_and_parser_accept_empty_identifiers_when_nip01_allows() {
    let issuer = Keys::generate();
    let recipient = Keys::generate();
    let resource_coordinate = format!("0:{}:", issuer.public_key());
    let input = params(&recipient, &resource_coordinate);
    let event = build_entitlement_event_builder(&input)
        .unwrap()
        .sign_with_keys(&issuer)
        .unwrap();

    assert_eq!(
        parse_entitlement_event(&event).unwrap().coordinate,
        resource_coordinate
    );
}

#[test]
fn generic_parser_rejects_malformed_resource_coordinates_and_ids() {
    let issuer = Keys::generate();
    let recipient = Keys::generate();
    let valid_coordinate = format!("30023:{}:article", issuer.public_key());

    for invalid_coordinate in [
        format!("not-a-kind:{}:article", issuer.public_key()),
        format!("1:{}:article", issuer.public_key()),
        "30023:not-a-public-key:article".into(),
        format!("30023:{}", issuer.public_key()),
        format!("30023:{}:", issuer.public_key()),
        format!("0:{}:unexpected-identifier", issuer.public_key()),
    ] {
        let event = root(&issuer, &recipient, &invalid_coordinate, 100);
        assert!(matches!(
            parse_entitlement_event(&event),
            Err(AdpError::MalformedTag(ref found)) if found == "a"
        ));
    }

    for name in ["source_event", "authorization_event"] {
        let event = signed_entitlement(
            &issuer,
            "grant-1",
            &recipient,
            &valid_coordinate,
            "granted",
            None,
            None,
            None,
            None,
            100,
            vec![tag(vec![name, "not-an-event-id"])],
        );
        assert!(matches!(
            parse_entitlement_event(&event),
            Err(AdpError::MalformedTag(ref found)) if found == name
        ));
    }

    let invalid_recipient =
        EventBuilder::new(Kind::Custom(EXPERIMENTAL_ENTITLEMENT_GRANT_KIND), "")
            .tags(vec![
                tag(vec!["d", "grant-1"]),
                tag(vec!["p", "not-a-public-key"]),
                tag(vec!["a", &valid_coordinate]),
                tag(vec!["status", "granted"]),
            ])
            .sign_with_keys(&issuer)
            .unwrap();
    assert!(matches!(
        parse_entitlement_event(&invalid_recipient),
        Err(AdpError::MalformedTag(ref found)) if found == "p"
    ));

    let invalid_predecessor = signed_entitlement(
        &issuer,
        "grant-1",
        &recipient,
        &valid_coordinate,
        "revoked",
        None,
        None,
        None,
        None,
        100,
        vec![tag(vec!["e", "not-an-event-id"])],
    );
    assert!(matches!(
        parse_entitlement_event(&invalid_predecessor),
        Err(AdpError::MalformedTag(ref found)) if found == "e"
    ));
}

#[test]
fn generic_parser_requires_kind_1030_and_a_valid_signature() {
    let issuer = Keys::generate();
    let recipient = Keys::generate();
    let coordinate = coordinate(&issuer, "game");
    let wrong_kind = EventBuilder::new(Kind::TextNote, "")
        .sign_with_keys(&issuer)
        .unwrap();
    assert!(matches!(
        parse_entitlement_event(&wrong_kind),
        Err(AdpError::WrongKind {
            expected: EXPERIMENTAL_ENTITLEMENT_GRANT_KIND,
            found: 1,
        })
    ));

    let valid = root(&issuer, &recipient, &coordinate, 100);
    let replacement_id = root(&issuer, &recipient, &coordinate, 101).id;
    let mut json = serde_json::to_value(valid).unwrap();
    json["id"] = replacement_id.to_hex().into();
    let invalid_signature: Event = serde_json::from_value(json).unwrap();
    assert!(matches!(
        parse_entitlement_event(&invalid_signature),
        Err(AdpError::InvalidSignature)
    ));
}

#[test]
fn generic_resolver_preserves_parse_errors_structurally() {
    let issuer = Keys::generate();
    let recipient = Keys::generate();
    let coordinate = coordinate(&issuer, "game");
    let event = root(&issuer, &recipient, &coordinate, 100);
    let mut parsed = parse_entitlement_event(&event).unwrap();
    let mut json = serde_json::to_value(event).unwrap();
    json["id"] = root(&issuer, &recipient, &coordinate, 101)
        .id
        .to_hex()
        .into();
    parsed.event = serde_json::from_value(json).unwrap();

    assert!(matches!(
        resolve_entitlement(&[parsed]),
        Err(EntitlementResolutionError::Parse(
            AdpError::InvalidSignature
        ))
    ));

    let mut parsed = parse_entitlement_event(&root(&issuer, &recipient, &coordinate, 100)).unwrap();
    parsed.event = signed_entitlement(
        &issuer,
        "grant-1",
        &recipient,
        &coordinate,
        "pending",
        None,
        None,
        None,
        None,
        100,
        vec![],
    );
    assert!(matches!(
        resolve_entitlement(&[parsed]),
        Err(EntitlementResolutionError::Parse(
            AdpError::MalformedTag(ref tag)
        )) if tag == "status"
    ));
}

#[test]
fn generic_valid_root_and_revoke_chain_resolves() {
    let issuer = Keys::generate();
    let revoker = Keys::generate();
    let recipient = Keys::generate();
    let coordinate = coordinate(&issuer, "game");
    let root = root(&issuer, &recipient, &coordinate, 100);
    let update = revoke(&revoker, &recipient, &coordinate, root.id, 101);
    let nodes = [&update, &root]
        .into_iter()
        .map(|event| parse_entitlement_event(event).unwrap())
        .collect::<Vec<_>>();

    let resolved = resolve_chain(&nodes).unwrap();

    assert_eq!(resolved.root_event_id, root.id);
    assert_eq!(resolved.events.len(), 2);
    assert_eq!(resolved.events[0].event.id, root.id);
    assert_eq!(resolved.events[1].event.id, update.id);
}

#[test]
fn generic_duplicate_root_is_deduplicated_for_grouping_and_resolution() {
    let issuer = Keys::generate();
    let recipient = Keys::generate();
    let coordinate = coordinate(&issuer, "game");
    let parsed = parse_entitlement_event(&root(&issuer, &recipient, &coordinate, 100)).unwrap();
    let candidates = vec![parsed.clone(), parsed];

    let components = group_parsed_entitlements(&candidates);
    assert_eq!(components.len(), 1);
    assert_eq!(components[0].len(), 1);
    assert_eq!(resolve_chain(&candidates).unwrap().events.len(), 1);
}

#[test]
fn generic_duplicate_revocation_is_deduplicated_for_grouping_and_resolution() {
    let issuer = Keys::generate();
    let recipient = Keys::generate();
    let coordinate = coordinate(&issuer, "game");
    let root = root(&issuer, &recipient, &coordinate, 100);
    let update = revoke(&issuer, &recipient, &coordinate, root.id, 101);
    let root = parse_entitlement_event(&root).unwrap();
    let update = parse_entitlement_event(&update).unwrap();
    let candidates = vec![root, update.clone(), update];

    let components = group_parsed_entitlements(&candidates);
    assert_eq!(components.len(), 1);
    assert_eq!(components[0].len(), 2);
    assert_eq!(resolve_chain(&candidates).unwrap().events.len(), 2);
}

#[test]
fn generic_malformed_event_does_not_abort_valid_candidate_grouping() {
    let issuer = Keys::generate();
    let recipient = Keys::generate();
    let coordinate = coordinate(&issuer, "game");
    let valid = root(&issuer, &recipient, &coordinate, 100);
    let malformed = EventBuilder::new(Kind::TextNote, "malformed candidate")
        .sign_with_keys(&issuer)
        .unwrap();
    let mut invalid_count = 0;
    let parsed = [valid, malformed]
        .iter()
        .filter_map(|event| match parse_entitlement_event(event) {
            Ok(parsed) => Some(parsed),
            Err(_) => {
                invalid_count += 1;
                None
            }
        })
        .collect::<Vec<_>>();

    let components = group_parsed_entitlements(&parsed);

    assert_eq!(invalid_count, 1);
    assert_eq!(components.len(), 1);
    assert!(resolve_chain(&components[0]).is_ok());
}

#[test]
fn generic_grouping_handles_large_same_identity_candidate_sets() {
    let issuer = Keys::generate();
    let recipient = Keys::generate();
    let coordinate = coordinate(&issuer, "game");
    let template = parse_entitlement_event(&root(&issuer, &recipient, &coordinate, 100)).unwrap();
    let candidates = (1_u64..=10_000)
        .map(|index| {
            let mut candidate = template.clone();
            candidate.event.id =
                EventId::from_hex(&format!("{index:064x}")).expect("synthetic event id");
            candidate
        })
        .collect::<Vec<_>>();
    let started = std::time::Instant::now();

    let components = group_parsed_entitlements(&candidates);

    assert_eq!(components.len(), candidates.len());
    assert!(
        started.elapsed() < std::time::Duration::from_secs(2),
        "grouping 10,000 candidates took {:?}",
        started.elapsed()
    );
}

#[test]
fn generic_chain_rejects_invariant_mutation() {
    let issuer = Keys::generate();
    let recipient = Keys::generate();
    let other_recipient = Keys::generate();
    let coordinate = coordinate(&issuer, "game");
    let other_coordinate = crate::coordinate(&issuer, "other-game");
    let root_source = root(&issuer, &recipient, &coordinate, 1).id;
    let root_authorization = root(&issuer, &recipient, &coordinate, 2).id;
    let other_source = root(&issuer, &recipient, &coordinate, 3).id;
    let other_authorization = root(&issuer, &recipient, &coordinate, 4).id;
    let root = signed_entitlement(
        &issuer,
        "grant-1",
        &recipient,
        &coordinate,
        "granted",
        None,
        Some(root_source),
        Some(root_authorization),
        None,
        100,
        vec![],
    );

    let mutation = |grant_id: &str,
                    update_recipient: &Keys,
                    update_coordinate: &str,
                    source,
                    authorization| {
        signed_entitlement(
            &issuer,
            grant_id,
            update_recipient,
            update_coordinate,
            "revoked",
            Some(root.id),
            source,
            authorization,
            None,
            101,
            vec![],
        )
    };
    let mutations = vec![
        (
            "d",
            mutation(
                "grant-2",
                &recipient,
                &coordinate,
                Some(root_source),
                Some(root_authorization),
            ),
        ),
        (
            "p",
            mutation(
                "grant-1",
                &other_recipient,
                &coordinate,
                Some(root_source),
                Some(root_authorization),
            ),
        ),
        (
            "a",
            mutation(
                "grant-1",
                &recipient,
                &other_coordinate,
                Some(root_source),
                Some(root_authorization),
            ),
        ),
        (
            "source_event",
            mutation(
                "grant-1",
                &recipient,
                &coordinate,
                None,
                Some(root_authorization),
            ),
        ),
        (
            "source_event",
            mutation(
                "grant-1",
                &recipient,
                &coordinate,
                Some(other_source),
                Some(root_authorization),
            ),
        ),
        (
            "authorization_event",
            mutation("grant-1", &recipient, &coordinate, Some(root_source), None),
        ),
        (
            "authorization_event",
            mutation(
                "grant-1",
                &recipient,
                &coordinate,
                Some(root_source),
                Some(other_authorization),
            ),
        ),
    ];

    for (field, update) in mutations {
        let nodes = [&root, &update]
            .into_iter()
            .map(|event| parse_entitlement_event(event).unwrap())
            .collect::<Vec<_>>();
        assert!(matches!(
            resolve_chain(&nodes),
            Err(ChainResolutionError::InvariantMutation(found)) if found == field
        ));
    }
}

#[test]
fn generic_successor_timestamp_must_strictly_increase() {
    let issuer = Keys::generate();
    let recipient = Keys::generate();
    let coordinate = coordinate(&issuer, "game");

    for update_time in [99, 100] {
        let root = root(&issuer, &recipient, &coordinate, 100);
        let update = revoke(&issuer, &recipient, &coordinate, root.id, update_time);
        let nodes = [&root, &update]
            .into_iter()
            .map(|event| parse_entitlement_event(event).unwrap())
            .collect::<Vec<_>>();
        assert!(matches!(
            resolve_chain(&nodes),
            Err(ChainResolutionError::TimestampRegression)
        ));
    }
}

#[test]
fn generic_revocation_is_terminal() {
    let issuer = Keys::generate();
    let recipient = Keys::generate();
    let coordinate = coordinate(&issuer, "game");
    let root = root(&issuer, &recipient, &coordinate, 100);
    let revoked = revoke(&issuer, &recipient, &coordinate, root.id, 101);
    let successor = revoke(&issuer, &recipient, &coordinate, revoked.id, 102);
    let nodes = [&root, &revoked, &successor]
        .into_iter()
        .map(|event| parse_entitlement_event(event).unwrap())
        .collect::<Vec<_>>();

    assert!(matches!(
        resolve_chain(&nodes),
        Err(ChainResolutionError::InvalidTransition(_))
    ));
}

#[test]
fn generic_fork_is_rejected_without_timestamp_selection() {
    let issuer = Keys::generate();
    let recipient = Keys::generate();
    let coordinate = coordinate(&issuer, "game");
    let root = root(&issuer, &recipient, &coordinate, 100);
    let early = revoke(&issuer, &recipient, &coordinate, root.id, 101);
    let late = revoke(&issuer, &recipient, &coordinate, root.id, 200);
    let nodes = [&root, &early, &late]
        .into_iter()
        .map(|event| parse_entitlement_event(event).unwrap())
        .collect::<Vec<_>>();

    assert!(matches!(
        resolve_chain(&nodes),
        Err(ChainResolutionError::Fork(id)) if id == root.id
    ));
}

#[test]
fn generic_missing_and_cross_grant_predecessors_are_rejected() {
    let issuer = Keys::generate();
    let recipient = Keys::generate();
    let coordinate = coordinate(&issuer, "game");
    let missing = root(&issuer, &recipient, &coordinate, 1).id;
    let detached = revoke(&issuer, &recipient, &coordinate, missing, 101);
    let detached_node = parse_entitlement_event(&detached).unwrap();
    assert!(matches!(
        resolve_entitlement(&[detached_node]),
        Err(EntitlementResolutionError::Chain(
            ChainResolutionError::MissingPredecessor(id)
        )) if id == missing
    ));

    let first_root = root(&issuer, &recipient, &coordinate, 100);
    let cross_update = signed_entitlement(
        &issuer,
        "grant-2",
        &recipient,
        &coordinate,
        "revoked",
        Some(first_root.id),
        None,
        None,
        None,
        101,
        vec![],
    );
    let candidates = parse_events(&[first_root, cross_update]);
    let components = group_parsed_entitlements(&candidates);
    assert_eq!(components.len(), 2);
    let update_component = components
        .iter()
        .find(|component| component[0].predecessor.is_some())
        .unwrap();
    assert!(matches!(
        resolve_chain(update_component),
        Err(ChainResolutionError::MissingPredecessor(_))
    ));
}

#[test]
fn generic_grouping_isolates_disconnected_colliding_roots() {
    let issuer = Keys::generate();
    let recipient = Keys::generate();
    let coordinate = coordinate(&issuer, "game");
    let valid_root = root(&issuer, &recipient, &coordinate, 100);
    let valid_update = revoke(&issuer, &recipient, &coordinate, valid_root.id, 101);
    let colliding_root = root(&issuer, &recipient, &coordinate, 200);

    let candidates = parse_events(&[
        valid_update.clone(),
        colliding_root.clone(),
        valid_root.clone(),
    ]);
    let components = group_parsed_entitlements(&candidates);

    assert_eq!(components.len(), 2);
    let valid_component = components
        .iter()
        .find(|component| component.len() == 2)
        .unwrap();
    let collision_component = components
        .iter()
        .find(|component| component.len() == 1)
        .unwrap();
    assert_eq!(
        resolve_chain(valid_component).unwrap().root_event_id,
        valid_root.id
    );
    assert_eq!(
        resolve_chain(collision_component).unwrap().root_event_id,
        colliding_root.id
    );
}

#[test]
fn generic_fork_invalidates_only_its_connected_component() {
    let issuer = Keys::generate();
    let recipient = Keys::generate();
    let coordinate = coordinate(&issuer, "game");
    let forked_root = root(&issuer, &recipient, &coordinate, 100);
    let first = revoke(&issuer, &recipient, &coordinate, forked_root.id, 101);
    let second = revoke(&issuer, &recipient, &coordinate, forked_root.id, 102);
    let isolated_root = root(&issuer, &recipient, &coordinate, 200);

    let candidates = parse_events(&[forked_root, first, isolated_root.clone(), second]);
    let components = group_parsed_entitlements(&candidates);
    assert_eq!(components.len(), 2);
    let forked = components
        .iter()
        .find(|component| component.len() == 3)
        .unwrap();
    let isolated = components
        .iter()
        .find(|component| component.len() == 1)
        .unwrap();
    assert!(matches!(
        resolve_chain(forked),
        Err(ChainResolutionError::Fork(_))
    ));
    assert_eq!(
        resolve_chain(isolated).unwrap().root_event_id,
        isolated_root.id
    );
}

#[test]
fn generic_resolver_rejects_metadata_detached_from_signed_event() {
    let issuer = Keys::generate();
    let recipient = Keys::generate();
    let coordinate = coordinate(&issuer, "game");
    let parsed = parse_entitlement_event(&root(&issuer, &recipient, &coordinate, 100)).unwrap();
    let fabricated_id = root(&issuer, &recipient, &coordinate, 1).id;

    let mut grant_id = parsed.clone();
    grant_id.grant_id = "fabricated".into();
    let mut parsed_recipient = parsed.clone();
    parsed_recipient.recipient = Keys::generate().public_key();
    let mut parsed_coordinate = parsed.clone();
    parsed_coordinate.coordinate = crate::coordinate(&issuer, "fabricated");
    let mut source = parsed.clone();
    source.source_event = Some(fabricated_id);
    let mut authorization = parsed.clone();
    authorization.authorization_event = Some(fabricated_id);
    let mut reason = parsed.clone();
    reason.reason = Some("fabricated".into());
    let mut status = parsed.clone();
    status.status = GrantStatus::Revoked;
    let mut predecessor = parsed;
    predecessor.predecessor = Some(fabricated_id);

    for node in [
        grant_id,
        parsed_recipient,
        parsed_coordinate,
        source,
        authorization,
        reason,
        status,
        predecessor,
    ] {
        assert!(matches!(
            resolve_chain(&[node]),
            Err(ChainResolutionError::InvariantMutation(
                "signed event metadata"
            ))
        ));
    }
}

#[allow(clippy::too_many_arguments)]
fn adp_campaign_event(
    developer: &Keys,
    campaign_id: &str,
    coordinate: &str,
    status: &str,
    predecessor: Option<EventId>,
    starts: Option<u64>,
    ends: Option<u64>,
    created_at: u64,
) -> Event {
    let mut tags = vec![
        tag(vec!["d", campaign_id]),
        tag(vec!["a", coordinate]),
        tag(vec!["status", status]),
    ];
    if let Some(predecessor) = predecessor {
        tags.push(tag(vec!["e", &predecessor.to_hex()]));
    }
    if let (Some(starts), Some(ends)) = (starts, ends) {
        tags.extend([
            tag(vec!["mode", "claim"]),
            tag(vec!["starts", &starts.to_string()]),
            tag(vec!["ends", &ends.to_string()]),
        ]);
    }
    EventBuilder::new(Kind::Custom(EXPERIMENTAL_ADP_CAMPAIGN_KIND), "")
        .tags(tags)
        .custom_created_at(Timestamp::from_secs(created_at))
        .sign_with_keys(developer)
        .unwrap()
}

fn adp_campaign(
    developer: &Keys,
    coordinate: &str,
    starts: u64,
    ends: u64,
    cancelled_at: Option<u64>,
) -> adp_core::ResolvedCampaign {
    let root = adp_campaign_event(
        developer,
        "campaign-1",
        coordinate,
        "active",
        None,
        Some(starts),
        Some(ends),
        80,
    );
    let mut events = vec![parse_campaign_event(&root).unwrap()];
    if let Some(cancelled_at) = cancelled_at {
        let cancel = adp_campaign_event(
            developer,
            "campaign-1",
            coordinate,
            "cancelled",
            Some(root.id),
            None,
            None,
            cancelled_at,
        );
        events.push(parse_campaign_event(&cancel).unwrap());
    }
    resolve_campaign(&events, developer.public_key(), coordinate).unwrap()
}

fn adp_authorization_event(
    developer: &Keys,
    terms: &AuthorizationTerms,
    status: &str,
    predecessor: Option<EventId>,
    created_at: u64,
) -> Event {
    let mut tags = vec![
        tag(vec![
            "d",
            if predecessor.is_some() {
                "revoke"
            } else {
                "root"
            },
        ]),
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
    EventBuilder::new(Kind::Custom(ACCEPTANCE_KIND), "")
        .tags(tags)
        .allow_self_tagging()
        .custom_created_at(Timestamp::from_secs(created_at))
        .sign_with_keys(developer)
        .unwrap()
}

fn adp_authorization(
    developer: &Keys,
    fulfillment: &Keys,
    coordinate: &str,
    valid_from: u64,
    valid_until: Option<u64>,
    revoked_at: Option<u64>,
) -> ResolvedAuthorization {
    let terms = AuthorizationTerms {
        authorization_id: "authorization-1".into(),
        coordinate: coordinate.into(),
        fulfillment_pubkey: fulfillment.public_key(),
        valid_from,
        valid_until,
    };
    let root = adp_authorization_event(developer, &terms, "active", None, 70);
    let mut events = vec![parse_authorization_event(&root).unwrap()];
    if let Some(revoked_at) = revoked_at {
        let revoke =
            adp_authorization_event(developer, &terms, "revoked", Some(root.id), revoked_at);
        events.push(parse_authorization_event(&revoke).unwrap());
    }
    resolve_authorization(root.id, &events).unwrap()
}

fn adp_grant_root(
    signer: &Keys,
    recipient: &Keys,
    coordinate: &str,
    campaign_root: EventId,
    authorization_root: Option<EventId>,
    created_at: u64,
) -> Event {
    signed_entitlement(
        signer,
        "grant-1",
        recipient,
        coordinate,
        "granted",
        None,
        Some(campaign_root),
        authorization_root,
        None,
        created_at,
        vec![],
    )
}

fn adp_grant_revoke(
    signer: &Keys,
    recipient: &Keys,
    coordinate: &str,
    campaign_root: EventId,
    authorization_root: Option<EventId>,
    predecessor: EventId,
    created_at: u64,
) -> Event {
    signed_entitlement(
        signer,
        "grant-1",
        recipient,
        coordinate,
        "revoked",
        Some(predecessor),
        Some(campaign_root),
        authorization_root,
        None,
        created_at,
        vec![],
    )
}

fn resolved_adp_grant(events: &[Event]) -> ResolvedEntitlementGrant {
    let parsed = events
        .iter()
        .map(|event| parse_entitlement_event(event).unwrap())
        .collect::<Vec<_>>();
    resolve_entitlement(&parsed).unwrap()
}

#[test]
fn adp_publisher_issuance_is_valid() {
    let developer = Keys::generate();
    let recipient = Keys::generate();
    let coordinate = coordinate(&developer, "game");
    let campaign = adp_campaign(&developer, &coordinate, 100, 200, None);
    let root = adp_grant_root(
        &developer,
        &recipient,
        &coordinate,
        campaign.root_event_id,
        None,
        120,
    );

    assert!(verify_adp_entitlement(&resolved_adp_grant(&[root]), &campaign, None).is_ok());
}

#[test]
fn adp_rejects_missing_campaign_source() {
    let developer = Keys::generate();
    let recipient = Keys::generate();
    let game_coordinate = coordinate(&developer, "game");
    let campaign = adp_campaign(&developer, &game_coordinate, 100, 200, None);
    let missing_source = root(&developer, &recipient, &game_coordinate, 120);
    assert!(matches!(
        verify_adp_entitlement(&resolved_adp_grant(&[missing_source]), &campaign, None),
        Err(AdpError::MissingCampaignSource)
    ));
}

#[test]
fn adp_rejects_forged_resolved_grant_envelopes() {
    let developer = Keys::generate();
    let recipient = Keys::generate();
    let game_coordinate = coordinate(&developer, "game");
    let campaign = adp_campaign(&developer, &game_coordinate, 100, 200, None);
    let event = adp_grant_root(
        &developer,
        &recipient,
        &game_coordinate,
        campaign.root_event_id,
        None,
        120,
    );
    let revoke = adp_grant_revoke(
        &developer,
        &recipient,
        &game_coordinate,
        campaign.root_event_id,
        None,
        event.id,
        121,
    );
    let canonical = resolved_adp_grant(&[revoke, event]);

    let mut root_id = canonical.clone();
    root_id.root_event_id = root_id.events[1].event.id;
    let mut ordering = canonical;
    ordering.events.reverse();

    for forged in [root_id, ordering] {
        assert!(matches!(
            verify_adp_entitlement(&forged, &campaign, None),
            Err(AdpError::InvalidEntitlementGrant(_))
        ));
    }
}

#[test]
fn adp_rejects_detached_signed_grant_node_metadata() {
    let developer = Keys::generate();
    let recipient = Keys::generate();
    let game_coordinate = coordinate(&developer, "game");
    let campaign = adp_campaign(&developer, &game_coordinate, 100, 200, None);
    let event = adp_grant_root(
        &developer,
        &recipient,
        &game_coordinate,
        campaign.root_event_id,
        None,
        120,
    );
    let mut detached = resolved_adp_grant(&[event]);
    detached.events[0].coordinate = coordinate(&developer, "fabricated");
    assert!(matches!(
        verify_adp_entitlement(&detached, &campaign, None),
        Err(AdpError::InvalidEntitlementGrant(_))
    ));
}

#[test]
fn adp_delegated_issuance_and_publisher_revocation_are_valid() {
    let developer = Keys::generate();
    let fulfillment = Keys::generate();
    let recipient = Keys::generate();
    let coordinate = coordinate(&developer, "game");
    let campaign = adp_campaign(&developer, &coordinate, 100, 200, None);
    let authorization =
        adp_authorization(&developer, &fulfillment, &coordinate, 100, Some(200), None);
    let root = adp_grant_root(
        &fulfillment,
        &recipient,
        &coordinate,
        campaign.root_event_id,
        Some(authorization.root_event_id),
        120,
    );
    let revoke = adp_grant_revoke(
        &developer,
        &recipient,
        &coordinate,
        campaign.root_event_id,
        Some(authorization.root_event_id),
        root.id,
        130,
    );

    assert!(verify_adp_entitlement(
        &resolved_adp_grant(&[revoke, root]),
        &campaign,
        Some(&authorization),
    )
    .is_ok());
}

#[test]
fn adp_rejects_stranger_root_and_fulfillment_revocation() {
    let developer = Keys::generate();
    let fulfillment = Keys::generate();
    let stranger = Keys::generate();
    let recipient = Keys::generate();
    let coordinate = coordinate(&developer, "game");
    let campaign = adp_campaign(&developer, &coordinate, 100, 200, None);
    let authorization =
        adp_authorization(&developer, &fulfillment, &coordinate, 100, Some(200), None);
    let stranger_root = adp_grant_root(
        &stranger,
        &recipient,
        &coordinate,
        campaign.root_event_id,
        Some(authorization.root_event_id),
        120,
    );
    assert!(matches!(
        verify_adp_entitlement(
            &resolved_adp_grant(&[stranger_root]),
            &campaign,
            Some(&authorization)
        ),
        Err(AdpError::GrantAuthorizationDenied)
    ));

    let root = adp_grant_root(
        &fulfillment,
        &recipient,
        &coordinate,
        campaign.root_event_id,
        Some(authorization.root_event_id),
        120,
    );
    let revoke = adp_grant_revoke(
        &fulfillment,
        &recipient,
        &coordinate,
        campaign.root_event_id,
        Some(authorization.root_event_id),
        root.id,
        130,
    );
    assert!(matches!(
        verify_adp_entitlement(
            &resolved_adp_grant(&[root, revoke]),
            &campaign,
            Some(&authorization)
        ),
        Err(AdpError::UnauthorizedGrantUpdateSigner)
    ));
}

#[test]
fn adp_delegated_root_requires_the_exact_authorization_anchor() {
    let developer = Keys::generate();
    let fulfillment = Keys::generate();
    let recipient = Keys::generate();
    let coordinate = coordinate(&developer, "game");
    let campaign = adp_campaign(&developer, &coordinate, 100, 200, None);
    let authorization =
        adp_authorization(&developer, &fulfillment, &coordinate, 100, Some(200), None);
    let missing_anchor = adp_grant_root(
        &fulfillment,
        &recipient,
        &coordinate,
        campaign.root_event_id,
        None,
        120,
    );
    assert!(matches!(
        verify_adp_entitlement(
            &resolved_adp_grant(&[missing_anchor]),
            &campaign,
            Some(&authorization)
        ),
        Err(AdpError::MissingGrantAuthorizationAnchor)
    ));

    let missing_resolution = adp_grant_root(
        &fulfillment,
        &recipient,
        &coordinate,
        campaign.root_event_id,
        Some(authorization.root_event_id),
        120,
    );
    assert!(matches!(
        verify_adp_entitlement(&resolved_adp_grant(&[missing_resolution]), &campaign, None),
        Err(AdpError::MissingResolvedGrantAuthorization)
    ));

    let other = adp_authorization(&developer, &fulfillment, &coordinate, 101, Some(200), None);
    let wrong_anchor = adp_grant_root(
        &fulfillment,
        &recipient,
        &coordinate,
        campaign.root_event_id,
        Some(other.root_event_id),
        120,
    );
    assert!(matches!(
        verify_adp_entitlement(
            &resolved_adp_grant(&[wrong_anchor]),
            &campaign,
            Some(&authorization)
        ),
        Err(AdpError::AuthorizationRootMismatch)
    ));
}

#[test]
fn adp_rejects_wrong_authorization_developer_key_and_coordinate() {
    let developer = Keys::generate();
    let other_developer = Keys::generate();
    let fulfillment = Keys::generate();
    let other_fulfillment = Keys::generate();
    let recipient = Keys::generate();
    let game_coordinate = coordinate(&developer, "game");
    let campaign = adp_campaign(&developer, &game_coordinate, 100, 200, None);

    let wrong_developer = adp_authorization(
        &other_developer,
        &fulfillment,
        &coordinate(&other_developer, "game"),
        100,
        Some(200),
        None,
    );
    let root = adp_grant_root(
        &fulfillment,
        &recipient,
        &game_coordinate,
        campaign.root_event_id,
        Some(wrong_developer.root_event_id),
        120,
    );
    assert!(matches!(
        verify_adp_entitlement(
            &resolved_adp_grant(&[root]),
            &campaign,
            Some(&wrong_developer)
        ),
        Err(AdpError::AuthorizationDeveloperMismatch)
    ));

    for authorization in [
        adp_authorization(
            &developer,
            &other_fulfillment,
            &game_coordinate,
            100,
            Some(200),
            None,
        ),
        adp_authorization(
            &developer,
            &fulfillment,
            &coordinate(&developer, "other"),
            100,
            Some(200),
            None,
        ),
    ] {
        let root = adp_grant_root(
            &fulfillment,
            &recipient,
            &game_coordinate,
            campaign.root_event_id,
            Some(authorization.root_event_id),
            120,
        );
        assert!(matches!(
            verify_adp_entitlement(
                &resolved_adp_grant(&[root]),
                &campaign,
                Some(&authorization)
            ),
            Err(AdpError::GrantAuthorizationDenied)
        ));
    }
}

#[test]
fn adp_binds_campaign_root_developer_and_coordinate() {
    let developer = Keys::generate();
    let other_developer = Keys::generate();
    let recipient = Keys::generate();
    let game_coordinate = coordinate(&developer, "game");
    let campaign = adp_campaign(&developer, &game_coordinate, 100, 200, None);
    let other_campaign = adp_campaign(&developer, &game_coordinate, 101, 200, None);
    let wrong_source = adp_grant_root(
        &developer,
        &recipient,
        &game_coordinate,
        other_campaign.root_event_id,
        None,
        120,
    );
    assert!(matches!(
        verify_adp_entitlement(&resolved_adp_grant(&[wrong_source]), &campaign, None),
        Err(AdpError::CampaignRootMismatch)
    ));

    let other_coordinate = coordinate(&developer, "other");
    let wrong_coordinate_campaign = adp_campaign(&developer, &other_coordinate, 100, 200, None);
    let wrong_coordinate_root = adp_grant_root(
        &developer,
        &recipient,
        &game_coordinate,
        wrong_coordinate_campaign.root_event_id,
        None,
        120,
    );
    assert!(matches!(
        verify_adp_entitlement(
            &resolved_adp_grant(&[wrong_coordinate_root]),
            &wrong_coordinate_campaign,
            None
        ),
        Err(AdpError::CampaignCoordinateMismatch)
    ));

    let foreign_coordinate = coordinate(&other_developer, "game");
    let wrong_developer_campaign =
        adp_campaign(&other_developer, &foreign_coordinate, 100, 200, None);
    let wrong_developer_root = adp_grant_root(
        &developer,
        &recipient,
        &game_coordinate,
        wrong_developer_campaign.root_event_id,
        None,
        120,
    );
    assert!(matches!(
        verify_adp_entitlement(
            &resolved_adp_grant(&[wrong_developer_root]),
            &wrong_developer_campaign,
            None
        ),
        Err(AdpError::CampaignDeveloperMismatch)
    ));
}

#[test]
fn adp_campaign_must_be_claimable_at_signed_root_time() {
    let developer = Keys::generate();
    let recipient = Keys::generate();
    let coordinate = coordinate(&developer, "game");
    for (starts, ends, issuance) in [(121, 200, 120), (100, 120, 120)] {
        let campaign = adp_campaign(&developer, &coordinate, starts, ends, None);
        let root = adp_grant_root(
            &developer,
            &recipient,
            &coordinate,
            campaign.root_event_id,
            None,
            issuance,
        );
        assert!(matches!(
            verify_adp_entitlement(&resolved_adp_grant(&[root]), &campaign, None),
            Err(AdpError::CampaignNotClaimableAtIssuance)
        ));
    }
}

#[test]
fn adp_campaign_cancellation_is_evaluated_at_issuance() {
    let developer = Keys::generate();
    let recipient = Keys::generate();
    let coordinate = coordinate(&developer, "game");
    for cancelled_at in [119, 120] {
        let campaign = adp_campaign(&developer, &coordinate, 100, 200, Some(cancelled_at));
        let root = adp_grant_root(
            &developer,
            &recipient,
            &coordinate,
            campaign.root_event_id,
            None,
            120,
        );
        assert!(matches!(
            verify_adp_entitlement(&resolved_adp_grant(&[root]), &campaign, None),
            Err(AdpError::CampaignNotClaimableAtIssuance)
        ));
    }

    let later_cancel = adp_campaign(&developer, &coordinate, 100, 200, Some(121));
    let root = adp_grant_root(
        &developer,
        &recipient,
        &coordinate,
        later_cancel.root_event_id,
        None,
        120,
    );
    assert!(verify_adp_entitlement(&resolved_adp_grant(&[root]), &later_cancel, None).is_ok());
}

#[test]
fn adp_authorization_interval_and_revocation_are_evaluated_at_issuance() {
    let developer = Keys::generate();
    let fulfillment = Keys::generate();
    let recipient = Keys::generate();
    let coordinate = coordinate(&developer, "game");
    let campaign = adp_campaign(&developer, &coordinate, 100, 200, None);
    for authorization in [
        adp_authorization(&developer, &fulfillment, &coordinate, 121, Some(200), None),
        adp_authorization(&developer, &fulfillment, &coordinate, 100, Some(120), None),
        adp_authorization(
            &developer,
            &fulfillment,
            &coordinate,
            100,
            Some(200),
            Some(119),
        ),
        adp_authorization(
            &developer,
            &fulfillment,
            &coordinate,
            100,
            Some(200),
            Some(120),
        ),
    ] {
        let root = adp_grant_root(
            &fulfillment,
            &recipient,
            &coordinate,
            campaign.root_event_id,
            Some(authorization.root_event_id),
            120,
        );
        assert!(matches!(
            verify_adp_entitlement(
                &resolved_adp_grant(&[root]),
                &campaign,
                Some(&authorization)
            ),
            Err(AdpError::GrantAuthorizationDenied)
        ));
    }

    let later_revocation = adp_authorization(
        &developer,
        &fulfillment,
        &coordinate,
        100,
        Some(200),
        Some(121),
    );
    let root = adp_grant_root(
        &fulfillment,
        &recipient,
        &coordinate,
        campaign.root_event_id,
        Some(later_revocation.root_event_id),
        120,
    );
    assert!(verify_adp_entitlement(
        &resolved_adp_grant(&[root]),
        &campaign,
        Some(&later_revocation)
    )
    .is_ok());
}

#[test]
fn adp_publisher_may_use_self_authorization() {
    let developer = Keys::generate();
    let recipient = Keys::generate();
    let coordinate = coordinate(&developer, "game");
    let campaign = adp_campaign(&developer, &coordinate, 100, 200, None);
    let authorization =
        adp_authorization(&developer, &developer, &coordinate, 100, Some(200), None);
    let root = adp_grant_root(
        &developer,
        &recipient,
        &coordinate,
        campaign.root_event_id,
        Some(authorization.root_event_id),
        120,
    );
    assert!(verify_adp_entitlement(
        &resolved_adp_grant(&[root]),
        &campaign,
        Some(&authorization)
    )
    .is_ok());
}

#[test]
fn adp_rejects_detached_campaign_dependency_metadata() {
    let developer = Keys::generate();
    let recipient = Keys::generate();
    let game_coordinate = coordinate(&developer, "game");
    let campaign = adp_campaign(&developer, &game_coordinate, 100, 200, None);
    let root = adp_grant_root(
        &developer,
        &recipient,
        &game_coordinate,
        campaign.root_event_id,
        None,
        120,
    );
    let grant = resolved_adp_grant(&[root]);

    let mut root_id = campaign.clone();
    root_id.root_event_id = grant.root_event_id;
    let mut developer_pubkey = campaign.clone();
    developer_pubkey.developer_pubkey = Keys::generate().public_key();
    let mut campaign_id = campaign.clone();
    campaign_id.campaign_id = "detached".into();
    let mut campaign_coordinate = campaign.clone();
    campaign_coordinate.coordinate = coordinate(&developer, "detached");
    let mut event_metadata = campaign.clone();
    event_metadata.events[0].campaign_id = "detached".into();

    for detached in [
        root_id,
        developer_pubkey,
        campaign_id,
        campaign_coordinate,
        event_metadata,
    ] {
        assert!(matches!(
            verify_adp_entitlement(&grant, &detached, None),
            Err(AdpError::InvalidCampaignDependency(_))
        ));
    }
}

#[test]
fn adp_rejects_detached_authorization_dependency_metadata() {
    let developer = Keys::generate();
    let recipient = Keys::generate();
    let game_coordinate = coordinate(&developer, "game");
    let campaign = adp_campaign(&developer, &game_coordinate, 100, 200, None);
    let authorization = adp_authorization(
        &developer,
        &developer,
        &game_coordinate,
        100,
        Some(200),
        None,
    );
    let root = adp_grant_root(
        &developer,
        &recipient,
        &game_coordinate,
        campaign.root_event_id,
        Some(authorization.root_event_id),
        120,
    );
    let grant = resolved_adp_grant(&[root]);

    let mut root_id = authorization.clone();
    root_id.root_event_id = campaign.root_event_id;
    let mut developer_pubkey = authorization.clone();
    developer_pubkey.developer_pubkey = Keys::generate().public_key();
    let mut terms = authorization.clone();
    terms.terms.authorization_id = "detached".into();
    let mut revoked_at = authorization.clone();
    revoked_at.revoked_at = Some(119);
    let mut event_metadata = authorization.clone();
    event_metadata.events[0].authorization_id = "detached".into();

    for detached in [root_id, developer_pubkey, terms, revoked_at, event_metadata] {
        assert!(matches!(
            verify_adp_entitlement(&grant, &campaign, Some(&detached)),
            Err(AdpError::InvalidAuthorizationDependency(_))
        ));
    }
}

#[test]
fn adp_direct_developer_anchor_requires_exact_authority_identity() {
    let developer = Keys::generate();
    let other_developer = Keys::generate();
    let other_key = Keys::generate();
    let recipient = Keys::generate();
    let game_coordinate = coordinate(&developer, "game");
    let campaign = adp_campaign(&developer, &game_coordinate, 100, 200, None);
    let self_authorization = adp_authorization(
        &developer,
        &developer,
        &game_coordinate,
        100,
        Some(200),
        None,
    );

    let verify = |authorization: &ResolvedAuthorization, anchor: EventId| {
        let root = adp_grant_root(
            &developer,
            &recipient,
            &game_coordinate,
            campaign.root_event_id,
            Some(anchor),
            120,
        );
        verify_adp_entitlement(&resolved_adp_grant(&[root]), &campaign, Some(authorization))
    };

    let other_root = adp_authorization(
        &developer,
        &developer,
        &game_coordinate,
        101,
        Some(200),
        None,
    );
    assert!(matches!(
        verify(&other_root, self_authorization.root_event_id),
        Err(AdpError::AuthorizationRootMismatch)
    ));

    let foreign_developer = adp_authorization(
        &other_developer,
        &other_developer,
        &coordinate(&other_developer, "game"),
        100,
        Some(200),
        None,
    );
    assert!(matches!(
        verify(&foreign_developer, foreign_developer.root_event_id),
        Err(AdpError::AuthorizationDeveloperMismatch)
    ));

    for denied in [
        adp_authorization(
            &developer,
            &other_key,
            &game_coordinate,
            100,
            Some(200),
            None,
        ),
        adp_authorization(
            &developer,
            &developer,
            &coordinate(&developer, "other"),
            100,
            Some(200),
            None,
        ),
        adp_authorization(
            &developer,
            &developer,
            &game_coordinate,
            121,
            Some(200),
            None,
        ),
        adp_authorization(
            &developer,
            &developer,
            &game_coordinate,
            100,
            Some(120),
            None,
        ),
        adp_authorization(
            &developer,
            &developer,
            &game_coordinate,
            100,
            Some(200),
            Some(119),
        ),
        adp_authorization(
            &developer,
            &developer,
            &game_coordinate,
            100,
            Some(200),
            Some(120),
        ),
    ] {
        assert!(matches!(
            verify(&denied, denied.root_event_id),
            Err(AdpError::GrantAuthorizationDenied)
        ));
    }
}

#[test]
fn adp_requires_kind_30402_while_generic_grants_remain_protocol_agnostic() {
    let developer = Keys::generate();
    let recipient = Keys::generate();
    let generic_coordinate = format!("30023:{}:article", developer.public_key());
    let generic = resolved_adp_grant(&[root(&developer, &recipient, &generic_coordinate, 120)]);
    assert_eq!(generic.events[0].coordinate, generic_coordinate);

    let campaign_coordinate = coordinate(&developer, "game");
    let campaign = adp_campaign(&developer, &campaign_coordinate, 100, 200, None);
    assert!(matches!(
        verify_adp_entitlement(&generic, &campaign, None),
        Err(AdpError::InvalidAdpGrantCoordinate)
    ));
}

#[test]
fn adp_coordinate_parser_accepts_nonempty_identifiers_containing_colons() {
    let developer = Keys::generate();
    let recipient = Keys::generate();
    let campaign_coordinate = coordinate(&developer, "game");
    let grant_coordinate = coordinate(&developer, "game:edition");
    let campaign = adp_campaign(&developer, &campaign_coordinate, 100, 200, None);
    let root = adp_grant_root(
        &developer,
        &recipient,
        &grant_coordinate,
        campaign.root_event_id,
        None,
        120,
    );

    assert!(matches!(
        verify_adp_entitlement(&resolved_adp_grant(&[root]), &campaign, None),
        Err(AdpError::CampaignCoordinateMismatch)
    ));
}
