use adp_core::campaign::{parse_campaign_event, resolve_campaign, resolve_campaign_at};
use adp_core::{CampaignStatus, ChainResolutionError, EXPERIMENTAL_ADP_CAMPAIGN_KIND};
use nostr::{Event, EventBuilder, EventId, Keys, Kind, Tag, Timestamp};

fn tag(values: &[&str]) -> Tag {
    Tag::parse(values.iter().copied()).unwrap()
}
fn coordinate(developer: &Keys) -> String {
    format!("30402:{}:game", developer.public_key())
}

fn event(
    developer: &Keys,
    coordinate: &str,
    predecessor: Option<EventId>,
    status: &str,
    terms: Option<(u64, u64)>,
    at: u64,
) -> Event {
    let mut tags = vec![
        tag(&["d", "campaign"]),
        tag(&["a", coordinate]),
        tag(&["status", status]),
    ];
    if let Some(previous) = predecessor {
        tags.push(tag(&["e", &previous.to_hex()]));
    }
    if let Some((starts, ends)) = terms {
        tags.extend([
            tag(&["mode", "claim"]),
            tag(&["starts", &starts.to_string()]),
            tag(&["ends", &ends.to_string()]),
        ]);
    }
    EventBuilder::new(Kind::Custom(EXPERIMENTAL_ADP_CAMPAIGN_KIND), "")
        .tags(tags)
        .custom_created_at(Timestamp::from_secs(at))
        .sign_with_keys(developer)
        .unwrap()
}

#[test]
fn successive_pre_start_updates_use_previously_effective_starts() {
    let developer = Keys::generate();
    let coordinate = coordinate(&developer);
    let root = event(
        &developer,
        &coordinate,
        None,
        "active",
        Some((200, 400)),
        100,
    );
    let first = event(
        &developer,
        &coordinate,
        Some(root.id),
        "active",
        Some((150, 350)),
        120,
    );
    let second = event(
        &developer,
        &coordinate,
        Some(first.id),
        "active",
        Some((250, 450)),
        140,
    );
    let nodes = [&root, &first, &second]
        .into_iter()
        .map(|event| parse_campaign_event(event).unwrap())
        .collect::<Vec<_>>();
    let resolved = resolve_campaign(&nodes, developer.public_key(), &coordinate).unwrap();
    assert_eq!(resolved.events.len(), 3);
    assert_eq!(resolved.state_at(140).unwrap().terms.starts, 250);
}

#[test]
fn update_at_currently_effective_start_is_excluded() {
    let developer = Keys::generate();
    let coordinate = coordinate(&developer);
    let root = event(
        &developer,
        &coordinate,
        None,
        "active",
        Some((200, 400)),
        100,
    );
    let invalid = event(
        &developer,
        &coordinate,
        Some(root.id),
        "active",
        Some((250, 450)),
        200,
    );
    let nodes = [&root, &invalid]
        .into_iter()
        .map(|event| parse_campaign_event(event).unwrap())
        .collect::<Vec<_>>();
    let resolved = resolve_campaign(&nodes, developer.public_key(), &coordinate).unwrap();
    assert_eq!(resolved.events.len(), 1);
}

#[test]
fn pre_start_complete_update_may_cancel() {
    let developer = Keys::generate();
    let coordinate = coordinate(&developer);
    let root = event(
        &developer,
        &coordinate,
        None,
        "active",
        Some((200, 400)),
        100,
    );
    let cancel = event(
        &developer,
        &coordinate,
        Some(root.id),
        "cancelled",
        Some((250, 450)),
        120,
    );
    let nodes = [&root, &cancel]
        .into_iter()
        .map(|event| parse_campaign_event(event).unwrap())
        .collect::<Vec<_>>();
    let resolved = resolve_campaign(&nodes, developer.public_key(), &coordinate).unwrap();
    assert_eq!(
        resolved.state_at(120).unwrap().status,
        CampaignStatus::Cancelled
    );
}

#[test]
fn cancellation_is_prospective_at_grant_root_time() {
    let developer = Keys::generate();
    let coordinate = coordinate(&developer);
    let root = event(
        &developer,
        &coordinate,
        None,
        "active",
        Some((100, 300)),
        90,
    );
    let cancel = event(
        &developer,
        &coordinate,
        Some(root.id),
        "cancelled",
        None,
        150,
    );
    let nodes = [&root, &cancel]
        .into_iter()
        .map(|event| parse_campaign_event(event).unwrap())
        .collect::<Vec<_>>();
    assert!(
        resolve_campaign_at(&nodes, developer.public_key(), &coordinate, 149)
            .unwrap()
            .is_claimable_at(149)
    );
    assert!(
        !resolve_campaign_at(&nodes, developer.public_key(), &coordinate, 150)
            .unwrap()
            .is_claimable_at(150)
    );
}

#[test]
fn malformed_later_event_does_not_invalidate_historical_campaign() {
    let developer = Keys::generate();
    let coordinate = coordinate(&developer);
    let root = event(
        &developer,
        &coordinate,
        None,
        "active",
        Some((100, 300)),
        90,
    );
    let malformed = EventBuilder::new(Kind::Custom(EXPERIMENTAL_ADP_CAMPAIGN_KIND), "")
        .tags(vec![tag(&["e", &root.id.to_hex()])])
        .custom_created_at(Timestamp::from_secs(120))
        .sign_with_keys(&developer)
        .unwrap();
    let nodes = [parse_campaign_event(&root).unwrap()];
    assert!(parse_campaign_event(&malformed).is_err());
    assert!(
        resolve_campaign(&nodes, developer.public_key(), &coordinate)
            .unwrap()
            .is_claimable_at(120)
    );
}

#[test]
fn two_valid_successors_are_a_fork() {
    let developer = Keys::generate();
    let coordinate = coordinate(&developer);
    let root = event(
        &developer,
        &coordinate,
        None,
        "active",
        Some((200, 400)),
        100,
    );
    let first = event(
        &developer,
        &coordinate,
        Some(root.id),
        "active",
        Some((250, 450)),
        120,
    );
    let second = event(
        &developer,
        &coordinate,
        Some(root.id),
        "active",
        Some((260, 460)),
        130,
    );
    let nodes = [&root, &first, &second]
        .into_iter()
        .map(|event| parse_campaign_event(event).unwrap())
        .collect::<Vec<_>>();
    assert!(matches!(
        resolve_campaign(&nodes, developer.public_key(), &coordinate),
        Err(ChainResolutionError::Fork(_))
    ));
}
