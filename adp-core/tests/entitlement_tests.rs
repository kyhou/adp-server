use std::collections::BTreeSet;

use adp_core::campaign::{parse_campaign_event, resolve_campaign};
use adp_core::{
    build_authorization_cancellation_event_builder, build_authorization_root_event_builder,
    build_entitlement_event_builder, parse_entitlement_event, resolve_adp_entitlement_grant,
    resolve_authorization_at, verify_adp_entitlement, AuthorizationTerms, BuildEntitlementParams,
    GrantStatus, CAPABILITY_ISSUE_GRANT, CAPABILITY_ISSUE_RECEIPT, EXPERIMENTAL_ADP_CAMPAIGN_KIND,
};
use nostr::{Event, EventBuilder, Keys, Kind, Tag, Timestamp};

fn tag(values: &[&str]) -> Tag {
    Tag::parse(values.iter().copied()).unwrap()
}
fn coordinate(developer: &Keys) -> String {
    format!("30402:{}:game", developer.public_key())
}

fn campaign(developer: &Keys, coordinate: &str) -> adp_core::ResolvedCampaign {
    let root = EventBuilder::new(Kind::Custom(EXPERIMENTAL_ADP_CAMPAIGN_KIND), "")
        .tags(vec![
            tag(&["d", "campaign"]),
            tag(&["a", coordinate]),
            tag(&["mode", "claim"]),
            tag(&["starts", "100"]),
            tag(&["ends", "200"]),
            tag(&["status", "active"]),
        ])
        .custom_created_at(Timestamp::from_secs(90))
        .sign_with_keys(developer)
        .unwrap();
    resolve_campaign(
        &[parse_campaign_event(&root).unwrap()],
        developer.public_key(),
        coordinate,
    )
    .unwrap()
}

fn authorization(
    developer: &Keys,
    fulfillment: &Keys,
    capability: &str,
    cancel: Option<u64>,
) -> adp_core::ResolvedAuthorization {
    let terms = AuthorizationTerms {
        authorization_id: "auth".into(),
        coordinate: coordinate(developer),
        operator_pubkey: Keys::generate().public_key(),
        fulfillment_pubkey: fulfillment.public_key(),
        capabilities: BTreeSet::from([capability.into()]),
        valid_from: 90,
    };
    let root = build_authorization_root_event_builder(&terms)
        .unwrap()
        .custom_created_at(Timestamp::from_secs(90))
        .sign_with_keys(developer)
        .unwrap();
    let mut events = vec![root.clone()];
    if let Some(at) = cancel {
        events.push(
            build_authorization_cancellation_event_builder(&terms, root.id)
                .unwrap()
                .custom_created_at(Timestamp::from_secs(at))
                .sign_with_keys(developer)
                .unwrap(),
        );
    }
    resolve_authorization_at(root.id, &events, u64::MAX).unwrap()
}

fn grant(
    signer: &Keys,
    buyer: &Keys,
    coordinate: &str,
    campaign: &adp_core::ResolvedCampaign,
    authorization: Option<nostr::EventId>,
    at: u64,
) -> Event {
    build_entitlement_event_builder(&BuildEntitlementParams {
        grant_id: "grant".into(),
        recipient: buyer.public_key(),
        coordinate: coordinate.into(),
        source_event: Some(campaign.root_event_id),
        authorization,
        reason: Some("promotional-claim".into()),
        status: GrantStatus::Granted,
        predecessor: None,
    })
    .unwrap()
    .custom_created_at(Timestamp::from_secs(at))
    .sign_with_keys(signer)
    .unwrap()
}

fn resolved(event: &Event, developer: &Keys) -> adp_core::ResolvedEntitlementGrant {
    resolve_adp_entitlement_grant(
        &[parse_entitlement_event(event).unwrap()],
        developer.public_key(),
    )
    .unwrap()
}

#[test]
fn delegated_grant_requires_issue_grant() {
    let developer = Keys::generate();
    let fulfillment = Keys::generate();
    let buyer = Keys::generate();
    let coordinate = coordinate(&developer);
    let campaign = campaign(&developer, &coordinate);
    let auth = authorization(&developer, &fulfillment, CAPABILITY_ISSUE_GRANT, None);
    let valid_grant = grant(
        &fulfillment,
        &buyer,
        &coordinate,
        &campaign,
        Some(auth.root_event_id),
        120,
    );
    assert!(
        verify_adp_entitlement(&resolved(&valid_grant, &developer), &campaign, Some(&auth)).is_ok()
    );

    let receipt_only = authorization(&developer, &fulfillment, CAPABILITY_ISSUE_RECEIPT, None);
    let invalid = grant(
        &fulfillment,
        &buyer,
        &coordinate,
        &campaign,
        Some(receipt_only.root_event_id),
        120,
    );
    assert!(verify_adp_entitlement(
        &resolved(&invalid, &developer),
        &campaign,
        Some(&receipt_only)
    )
    .is_err());
}

#[test]
fn direct_grant_with_redundant_authorization_is_rejected() {
    let developer = Keys::generate();
    let buyer = Keys::generate();
    let coordinate = coordinate(&developer);
    let campaign = campaign(&developer, &coordinate);
    let grant = grant(
        &developer,
        &buyer,
        &coordinate,
        &campaign,
        Some(nostr::EventId::all_zeros()),
        120,
    );
    assert!(verify_adp_entitlement(&resolved(&grant, &developer), &campaign, None).is_err());
}

#[test]
fn only_developer_revocation_is_admitted_even_after_authorization_cancellation() {
    let developer = Keys::generate();
    let fulfillment = Keys::generate();
    let buyer = Keys::generate();
    let coordinate = coordinate(&developer);
    let campaign = campaign(&developer, &coordinate);
    let auth = authorization(&developer, &fulfillment, CAPABILITY_ISSUE_GRANT, Some(130));
    let root = grant(
        &fulfillment,
        &buyer,
        &coordinate,
        &campaign,
        Some(auth.root_event_id),
        120,
    );
    let params = BuildEntitlementParams {
        grant_id: "grant".into(),
        recipient: buyer.public_key(),
        coordinate: coordinate.clone(),
        source_event: Some(campaign.root_event_id),
        authorization: Some(auth.root_event_id),
        reason: None,
        status: GrantStatus::Revoked,
        predecessor: Some(root.id),
    };
    let developer_revoke = build_entitlement_event_builder(&params)
        .unwrap()
        .custom_created_at(Timestamp::from_secs(140))
        .sign_with_keys(&developer)
        .unwrap();
    let nodes = [&root, &developer_revoke]
        .into_iter()
        .map(|event| parse_entitlement_event(event).unwrap())
        .collect::<Vec<_>>();
    let chain = resolve_adp_entitlement_grant(&nodes, developer.public_key()).unwrap();
    assert!(verify_adp_entitlement(&chain, &campaign, Some(&auth)).is_ok());

    let fulfillment_revoke = build_entitlement_event_builder(&params)
        .unwrap()
        .custom_created_at(Timestamp::from_secs(141))
        .sign_with_keys(&fulfillment)
        .unwrap();
    let nodes = [&root, &fulfillment_revoke]
        .into_iter()
        .map(|event| parse_entitlement_event(event).unwrap())
        .collect::<Vec<_>>();
    let chain = resolve_adp_entitlement_grant(&nodes, developer.public_key()).unwrap();
    assert_eq!(chain.events.len(), 1);
}

#[test]
fn grant_at_authorization_cancellation_boundary_is_invalid() {
    let developer = Keys::generate();
    let fulfillment = Keys::generate();
    let buyer = Keys::generate();
    let coordinate = coordinate(&developer);
    let campaign = campaign(&developer, &coordinate);
    let auth = authorization(&developer, &fulfillment, CAPABILITY_ISSUE_GRANT, Some(120));
    let grant = grant(
        &fulfillment,
        &buyer,
        &coordinate,
        &campaign,
        Some(auth.root_event_id),
        120,
    );
    assert!(verify_adp_entitlement(&resolved(&grant, &developer), &campaign, Some(&auth)).is_err());
}

#[test]
fn historical_grant_survives_later_campaign_cancellation() {
    let developer = Keys::generate();
    let buyer = Keys::generate();
    let coordinate = coordinate(&developer);
    let campaign_root = EventBuilder::new(Kind::Custom(EXPERIMENTAL_ADP_CAMPAIGN_KIND), "")
        .tags(vec![
            tag(&["d", "campaign"]),
            tag(&["a", &coordinate]),
            tag(&["mode", "claim"]),
            tag(&["starts", "100"]),
            tag(&["ends", "200"]),
            tag(&["status", "active"]),
        ])
        .custom_created_at(Timestamp::from_secs(90))
        .sign_with_keys(&developer)
        .unwrap();
    let cancellation = EventBuilder::new(Kind::Custom(EXPERIMENTAL_ADP_CAMPAIGN_KIND), "")
        .tags(vec![
            tag(&["d", "campaign"]),
            tag(&["a", &coordinate]),
            tag(&["status", "cancelled"]),
            tag(&["e", &campaign_root.id.to_hex()]),
        ])
        .custom_created_at(Timestamp::from_secs(130))
        .sign_with_keys(&developer)
        .unwrap();
    let campaign_nodes = [&campaign_root, &cancellation]
        .into_iter()
        .map(|event| parse_campaign_event(event).unwrap())
        .collect::<Vec<_>>();
    let full_campaign =
        resolve_campaign(&campaign_nodes, developer.public_key(), &coordinate).unwrap();
    let grant = grant(&developer, &buyer, &coordinate, &full_campaign, None, 120);
    assert!(verify_adp_entitlement(&resolved(&grant, &developer), &full_campaign, None).is_ok());
}

#[test]
fn adp_grant_requires_campaign_source_root() {
    let developer = Keys::generate();
    let buyer = Keys::generate();
    let coordinate = coordinate(&developer);
    let campaign = campaign(&developer, &coordinate);
    let event = build_entitlement_event_builder(&BuildEntitlementParams {
        grant_id: "grant".into(),
        recipient: buyer.public_key(),
        coordinate: coordinate.clone(),
        source_event: None,
        authorization: None,
        reason: None,
        status: GrantStatus::Granted,
        predecessor: None,
    })
    .unwrap()
    .custom_created_at(Timestamp::from_secs(120))
    .sign_with_keys(&developer)
    .unwrap();
    assert!(verify_adp_entitlement(&resolved(&event, &developer), &campaign, None).is_err());
}

#[test]
fn adp_grant_binds_exact_campaign_root() {
    let developer = Keys::generate();
    let buyer = Keys::generate();
    let coordinate = coordinate(&developer);
    let expected = campaign(&developer, &coordinate);
    let other_root = EventBuilder::new(Kind::Custom(EXPERIMENTAL_ADP_CAMPAIGN_KIND), "other")
        .tags(vec![
            tag(&["d", "other-campaign"]),
            tag(&["a", &coordinate]),
            tag(&["mode", "claim"]),
            tag(&["starts", "100"]),
            tag(&["ends", "200"]),
            tag(&["status", "active"]),
        ])
        .custom_created_at(Timestamp::from_secs(90))
        .sign_with_keys(&developer)
        .unwrap();
    let other = resolve_campaign(
        &[parse_campaign_event(&other_root).unwrap()],
        developer.public_key(),
        &coordinate,
    )
    .unwrap();
    let event = grant(&developer, &buyer, &coordinate, &other, None, 120);
    assert!(verify_adp_entitlement(&resolved(&event, &developer), &expected, None).is_err());
}
