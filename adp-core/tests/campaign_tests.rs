use adp_core::campaign::{parse_campaign_event, resolve_campaign};
use adp_core::{
    CampaignStatus, CampaignTerms, CampaignTransition, ChainResolutionError,
    EXPERIMENTAL_ADP_CAMPAIGN_KIND,
};
use nostr::{Event, EventBuilder, EventId, Keys, Kind, Tag, Timestamp};

fn tag(values: Vec<&str>) -> Tag {
    Tag::parse(values).expect("valid tag")
}

fn coordinate(developer: &Keys, game_id: &str) -> String {
    format!("30402:{}:{game_id}", developer.public_key())
}

fn signed_campaign(
    signer: &Keys,
    campaign_id: &str,
    coordinate: &str,
    status: &str,
    predecessor: Option<EventId>,
    terms: Option<(&str, u64, u64)>,
    created_at: u64,
    extra_tags: Vec<Tag>,
) -> Event {
    let mut tags = vec![
        tag(vec!["d", campaign_id]),
        tag(vec!["a", coordinate]),
        tag(vec!["status", status]),
    ];
    if let Some(predecessor) = predecessor {
        tags.push(tag(vec!["e", &predecessor.to_hex()]));
    }
    if let Some((mode, starts, ends)) = terms {
        tags.extend([
            tag(vec!["mode", mode]),
            tag(vec!["starts", &starts.to_string()]),
            tag(vec!["ends", &ends.to_string()]),
        ]);
    }
    tags.extend(extra_tags);

    EventBuilder::new(Kind::Custom(EXPERIMENTAL_ADP_CAMPAIGN_KIND), "")
        .tags(tags)
        .custom_created_at(Timestamp::from_secs(created_at))
        .sign_with_keys(signer)
        .expect("sign campaign")
}

fn root(developer: &Keys, campaign_id: &str, coordinate: &str) -> Event {
    signed_campaign(
        developer,
        campaign_id,
        coordinate,
        "active",
        None,
        Some(("claim", 120, 300)),
        100,
        vec![],
    )
}

#[test]
fn valid_root_resolves() {
    let developer = Keys::generate();
    let coordinate = coordinate(&developer, "game");
    let root = parse_campaign_event(&root(&developer, "campaign", &coordinate)).unwrap();

    assert!(matches!(
        root.transition,
        CampaignTransition::Root(CampaignTerms {
            starts: 120,
            ends: 300,
            ..
        })
    ));

    let resolved = resolve_campaign(&[root], developer.public_key(), &coordinate).unwrap();
    let state = resolved.state_at(100);
    assert_eq!(state.status, CampaignStatus::Active);
    assert_eq!(state.terms.starts, 120);
}

#[test]
fn invalid_signature_is_rejected() {
    let developer = Keys::generate();
    let coordinate = coordinate(&developer, "game");
    let event = root(&developer, "campaign", &coordinate);
    let mut value = serde_json::to_value(event).unwrap();
    value["content"] = serde_json::Value::String("tampered".into());
    let tampered: Event = serde_json::from_value(value).unwrap();

    assert!(parse_campaign_event(&tampered).is_err());
}

#[test]
fn coordinate_signer_mismatch_is_rejected() {
    let developer = Keys::generate();
    let other = Keys::generate();
    let coordinate = coordinate(&developer, "game");
    let event = root(&other, "campaign", &coordinate);

    assert!(parse_campaign_event(&event).is_err());
}

#[test]
fn root_starts_before_created_at_is_invalid() {
    let developer = Keys::generate();
    let coordinate = coordinate(&developer, "game");
    let event = signed_campaign(
        &developer,
        "campaign",
        &coordinate,
        "active",
        None,
        Some(("claim", 99, 300)),
        100,
        vec![],
    );
    let root = parse_campaign_event(&event).unwrap();

    assert!(matches!(
        resolve_campaign(&[root], developer.public_key(), &coordinate),
        Err(ChainResolutionError::InvalidTransition(_))
    ));
}

#[test]
fn malformed_root_interval_is_invalid() {
    let developer = Keys::generate();
    let coordinate = coordinate(&developer, "game");
    let event = signed_campaign(
        &developer,
        "campaign",
        &coordinate,
        "active",
        None,
        Some(("claim", 200, 200)),
        100,
        vec![],
    );
    let root = parse_campaign_event(&event).unwrap();

    assert!(matches!(
        resolve_campaign(&[root], developer.public_key(), &coordinate),
        Err(ChainResolutionError::InvalidTransition(_))
    ));
}

#[test]
fn claimability_is_half_open() {
    let developer = Keys::generate();
    let coordinate = coordinate(&developer, "game");
    let root = parse_campaign_event(&root(&developer, "campaign", &coordinate)).unwrap();
    let resolved = resolve_campaign(&[root], developer.public_key(), &coordinate).unwrap();

    assert!(!resolved.is_claimable_at(119));
    assert!(resolved.is_claimable_at(120));
    assert!(resolved.is_claimable_at(299));
    assert!(!resolved.is_claimable_at(300));
}

#[test]
fn valid_pre_start_replacement_changes_effective_terms() {
    let developer = Keys::generate();
    let coordinate = coordinate(&developer, "game");
    let root = root(&developer, "campaign", &coordinate);
    let update = signed_campaign(
        &developer,
        "campaign",
        &coordinate,
        "active",
        Some(root.id),
        Some(("claim", 250, 400)),
        110,
        vec![],
    );
    let nodes = [
        parse_campaign_event(&update).unwrap(),
        parse_campaign_event(&root).unwrap(),
    ];

    let resolved = resolve_campaign(&nodes, developer.public_key(), &coordinate).unwrap();
    assert_eq!(resolved.events[0].event.id, root.id);
    assert_eq!(resolved.events[1].event.id, update.id);
    assert_eq!(resolved.state_at(109).terms.starts, 120);
    assert_eq!(resolved.state_at(110).terms.starts, 250);
}

#[test]
fn replacement_at_previous_start_is_invalid() {
    let developer = Keys::generate();
    let coordinate = coordinate(&developer, "game");
    let root = root(&developer, "campaign", &coordinate);
    let update = signed_campaign(
        &developer,
        "campaign",
        &coordinate,
        "active",
        Some(root.id),
        Some(("claim", 250, 400)),
        120,
        vec![],
    );
    let nodes = [
        parse_campaign_event(&root).unwrap(),
        parse_campaign_event(&update).unwrap(),
    ];

    assert!(matches!(
        resolve_campaign(&nodes, developer.public_key(), &coordinate),
        Err(ChainResolutionError::InvalidTransition(_))
    ));
}

#[test]
fn replacement_start_must_follow_update_timestamp() {
    let developer = Keys::generate();
    let coordinate = coordinate(&developer, "game");
    let root = root(&developer, "campaign", &coordinate);
    let update = signed_campaign(
        &developer,
        "campaign",
        &coordinate,
        "active",
        Some(root.id),
        Some(("claim", 110, 400)),
        110,
        vec![],
    );
    let nodes = [
        parse_campaign_event(&root).unwrap(),
        parse_campaign_event(&update).unwrap(),
    ];

    assert!(matches!(
        resolve_campaign(&nodes, developer.public_key(), &coordinate),
        Err(ChainResolutionError::InvalidTransition(_))
    ));
}

#[test]
fn post_start_terms_edit_is_invalid() {
    let developer = Keys::generate();
    let coordinate = coordinate(&developer, "game");
    let root = root(&developer, "campaign", &coordinate);
    let update = signed_campaign(
        &developer,
        "campaign",
        &coordinate,
        "active",
        Some(root.id),
        Some(("claim", 250, 400)),
        121,
        vec![],
    );
    let nodes = [
        parse_campaign_event(&root).unwrap(),
        parse_campaign_event(&update).unwrap(),
    ];

    assert!(matches!(
        resolve_campaign(&nodes, developer.public_key(), &coordinate),
        Err(ChainResolutionError::InvalidTransition(_))
    ));
}

#[test]
fn valid_cancellation_is_terminal_at_its_timestamp() {
    let developer = Keys::generate();
    let coordinate = coordinate(&developer, "game");
    let root = root(&developer, "campaign", &coordinate);
    let cancel = signed_campaign(
        &developer,
        "campaign",
        &coordinate,
        "cancelled",
        Some(root.id),
        None,
        180,
        vec![],
    );
    let nodes = [
        parse_campaign_event(&cancel).unwrap(),
        parse_campaign_event(&root).unwrap(),
    ];

    let resolved = resolve_campaign(&nodes, developer.public_key(), &coordinate).unwrap();
    assert_eq!(resolved.state_at(179).status, CampaignStatus::Active);
    assert_eq!(resolved.state_at(180).status, CampaignStatus::Cancelled);
    assert!(resolved.is_claimable_at(179));
    assert!(!resolved.is_claimable_at(180));
}

#[test]
fn cancellation_cannot_carry_terms() {
    let developer = Keys::generate();
    let coordinate = coordinate(&developer, "game");
    let root = root(&developer, "campaign", &coordinate);
    let cancel = signed_campaign(
        &developer,
        "campaign",
        &coordinate,
        "cancelled",
        Some(root.id),
        Some(("claim", 120, 300)),
        180,
        vec![],
    );

    assert!(parse_campaign_event(&cancel).is_err());
}

#[test]
fn cancellation_is_terminal() {
    let developer = Keys::generate();
    let coordinate = coordinate(&developer, "game");
    let root = root(&developer, "campaign", &coordinate);
    let cancel = signed_campaign(
        &developer,
        "campaign",
        &coordinate,
        "cancelled",
        Some(root.id),
        None,
        180,
        vec![],
    );
    let successor = signed_campaign(
        &developer,
        "campaign",
        &coordinate,
        "cancelled",
        Some(cancel.id),
        None,
        181,
        vec![],
    );
    let nodes = [
        parse_campaign_event(&root).unwrap(),
        parse_campaign_event(&cancel).unwrap(),
        parse_campaign_event(&successor).unwrap(),
    ];

    assert!(matches!(
        resolve_campaign(&nodes, developer.public_key(), &coordinate),
        Err(ChainResolutionError::InvalidTransition(_))
    ));
}

#[test]
fn fork_is_rejected_without_timestamp_selection() {
    let developer = Keys::generate();
    let coordinate = coordinate(&developer, "game");
    let root = root(&developer, "campaign", &coordinate);
    let first = signed_campaign(
        &developer,
        "campaign",
        &coordinate,
        "active",
        Some(root.id),
        Some(("claim", 200, 350)),
        105,
        vec![],
    );
    let second = signed_campaign(
        &developer,
        "campaign",
        &coordinate,
        "active",
        Some(root.id),
        Some(("claim", 220, 360)),
        110,
        vec![],
    );
    let nodes = [
        parse_campaign_event(&root).unwrap(),
        parse_campaign_event(&first).unwrap(),
        parse_campaign_event(&second).unwrap(),
    ];

    assert!(matches!(
        resolve_campaign(&nodes, developer.public_key(), &coordinate),
        Err(ChainResolutionError::Fork(id)) if id == root.id
    ));
}

#[test]
fn cycle_is_rejected_by_pure_resolver() {
    let developer = Keys::generate();
    let coordinate = coordinate(&developer, "game");
    let root_event = root(&developer, "campaign", &coordinate);
    let update_event = signed_campaign(
        &developer,
        "campaign",
        &coordinate,
        "active",
        Some(root_event.id),
        Some(("claim", 200, 350)),
        110,
        vec![],
    );
    let mut root = parse_campaign_event(&root_event).unwrap();
    let update = parse_campaign_event(&update_event).unwrap();
    root.predecessor = Some(update.event.id);

    assert!(matches!(
        resolve_campaign(&[root, update], developer.public_key(), &coordinate),
        Err(ChainResolutionError::Cycle)
    ));
}

#[test]
fn missing_predecessor_has_distinct_error() {
    let developer = Keys::generate();
    let coordinate = coordinate(&developer, "game");
    let missing = root(&developer, "missing", &coordinate).id;
    let update = signed_campaign(
        &developer,
        "campaign",
        &coordinate,
        "active",
        Some(missing),
        Some(("claim", 200, 350)),
        110,
        vec![],
    );
    let update = parse_campaign_event(&update).unwrap();

    assert!(matches!(
        resolve_campaign(&[update], developer.public_key(), &coordinate),
        Err(ChainResolutionError::MissingPredecessor(id)) if id == missing
    ));
}

#[test]
fn changed_campaign_d_is_rejected() {
    let developer = Keys::generate();
    let coordinate = coordinate(&developer, "game");
    let root = root(&developer, "campaign", &coordinate);
    let update = signed_campaign(
        &developer,
        "other-campaign",
        &coordinate,
        "active",
        Some(root.id),
        Some(("claim", 200, 350)),
        110,
        vec![],
    );
    let nodes = [
        parse_campaign_event(&root).unwrap(),
        parse_campaign_event(&update).unwrap(),
    ];

    assert!(matches!(
        resolve_campaign(&nodes, developer.public_key(), &coordinate),
        Err(ChainResolutionError::InvariantMutation("d"))
    ));
}

#[test]
fn changed_coordinate_a_is_rejected() {
    let developer = Keys::generate();
    let other_coordinate = coordinate(&developer, "other-game");
    let coordinate = coordinate(&developer, "game");
    let root = root(&developer, "campaign", &coordinate);
    let update = signed_campaign(
        &developer,
        "campaign",
        &other_coordinate,
        "active",
        Some(root.id),
        Some(("claim", 200, 350)),
        110,
        vec![],
    );
    let nodes = [
        parse_campaign_event(&root).unwrap(),
        parse_campaign_event(&update).unwrap(),
    ];

    assert!(matches!(
        resolve_campaign(&nodes, developer.public_key(), &coordinate),
        Err(ChainResolutionError::InvariantMutation("a"))
    ));
}

#[test]
fn expected_developer_is_part_of_group_identity() {
    let developer = Keys::generate();
    let other = Keys::generate();
    let coordinate = coordinate(&developer, "game");
    let root = parse_campaign_event(&root(&developer, "campaign", &coordinate)).unwrap();

    assert!(matches!(
        resolve_campaign(&[root], other.public_key(), &coordinate),
        Err(ChainResolutionError::InvariantMutation("developer"))
    ));
}

#[test]
fn successor_timestamp_must_strictly_increase() {
    let developer = Keys::generate();
    let coordinate = coordinate(&developer, "game");
    let root = root(&developer, "campaign", &coordinate);
    let update = signed_campaign(
        &developer,
        "campaign",
        &coordinate,
        "active",
        Some(root.id),
        Some(("claim", 200, 350)),
        100,
        vec![],
    );
    let nodes = [
        parse_campaign_event(&root).unwrap(),
        parse_campaign_event(&update).unwrap(),
    ];

    assert!(matches!(
        resolve_campaign(&nodes, developer.public_key(), &coordinate),
        Err(ChainResolutionError::TimestampRegression)
    ));
}

#[test]
fn self_reference_is_rejected_by_pure_resolver() {
    let developer = Keys::generate();
    let coordinate = coordinate(&developer, "game");
    let mut root = parse_campaign_event(&root(&developer, "campaign", &coordinate)).unwrap();
    root.predecessor = Some(root.event.id);

    assert!(matches!(
        resolve_campaign(&[root], developer.public_key(), &coordinate),
        Err(ChainResolutionError::Cycle)
    ));
}

#[test]
fn disconnected_component_is_rejected() {
    let developer = Keys::generate();
    let coordinate = coordinate(&developer, "game");
    let root_event = root(&developer, "campaign", &coordinate);
    let first_event = signed_campaign(
        &developer,
        "campaign",
        &coordinate,
        "active",
        Some(root_event.id),
        Some(("claim", 200, 350)),
        105,
        vec![],
    );
    let second_event = signed_campaign(
        &developer,
        "campaign",
        &coordinate,
        "active",
        Some(first_event.id),
        Some(("claim", 220, 360)),
        110,
        vec![],
    );
    let root = parse_campaign_event(&root_event).unwrap();
    let mut first = parse_campaign_event(&first_event).unwrap();
    let mut second = parse_campaign_event(&second_event).unwrap();
    first.predecessor = Some(second.event.id);
    second.predecessor = Some(first.event.id);

    assert!(matches!(
        resolve_campaign(&[root, first, second], developer.public_key(), &coordinate),
        Err(ChainResolutionError::Disconnected)
    ));
}

#[test]
fn multiple_roots_are_rejected() {
    let developer = Keys::generate();
    let coordinate = coordinate(&developer, "game");
    let first = parse_campaign_event(&root(&developer, "campaign", &coordinate)).unwrap();
    let second_event = signed_campaign(
        &developer,
        "campaign",
        &coordinate,
        "active",
        None,
        Some(("claim", 130, 300)),
        101,
        vec![],
    );
    let second = parse_campaign_event(&second_event).unwrap();

    assert!(matches!(
        resolve_campaign(&[first, second], developer.public_key(), &coordinate),
        Err(ChainResolutionError::MultipleRoots)
    ));
}

#[test]
fn required_identity_tags_have_exact_cardinality() {
    let developer = Keys::generate();
    let coordinate = coordinate(&developer, "game");

    for duplicate in [
        tag(vec!["d", "campaign"]),
        tag(vec!["a", &coordinate]),
        tag(vec!["status", "active"]),
    ] {
        let event = signed_campaign(
            &developer,
            "campaign",
            &coordinate,
            "active",
            None,
            Some(("claim", 120, 300)),
            100,
            vec![duplicate],
        );
        assert!(parse_campaign_event(&event).is_err());
    }
}
