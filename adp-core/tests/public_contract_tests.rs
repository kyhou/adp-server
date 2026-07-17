use adp_core::{
    AcquisitionPolicy, AuthorizationEvent, AuthorizationTerms, AuthorizationTransition,
    CampaignEvent, CampaignMode, CampaignPointer, CampaignStatus, CampaignTerms,
    CampaignTransition, ChainResolutionError, GrantStatus, ParsedEntitlementGrant,
    ResolvedAuthorization, ResolvedCampaign, ResolvedEntitlementGrant, ADP_VERSION,
    EXPERIMENTAL_ADP_CAMPAIGN_KIND, EXPERIMENTAL_ENTITLEMENT_GRANT_KIND,
};
use nostr::{EventBuilder, Keys, Kind, RelayUrl};

fn acquisition_discriminant(value: AcquisitionPolicy) -> u8 {
    match value {
        AcquisitionPolicy::Gated => 0,
        AcquisitionPolicy::Public => 1,
        AcquisitionPolicy::TimedAccess { .. } => 2,
        AcquisitionPolicy::Invalid => 3,
    }
}

fn authorization_discriminant(value: AuthorizationTransition) -> u8 {
    match value {
        AuthorizationTransition::ActiveRoot(_) => 0,
        AuthorizationTransition::Revoke => 1,
    }
}

fn campaign_discriminant(value: CampaignTransition) -> u8 {
    match value {
        CampaignTransition::Root(_) => 0,
        CampaignTransition::ReplaceTerms(_) => 1,
        CampaignTransition::Cancel => 2,
    }
}

fn campaign_mode_discriminant(value: CampaignMode) -> u8 {
    match value {
        CampaignMode::Claim => 0,
    }
}

fn campaign_status_discriminant(value: CampaignStatus) -> u8 {
    match value {
        CampaignStatus::Active => 0,
        CampaignStatus::Cancelled => 1,
    }
}

fn grant_status_discriminant(value: GrantStatus) -> u8 {
    match value {
        GrantStatus::Granted => 0,
        GrantStatus::Revoked => 1,
    }
}

fn chain_error_discriminant(value: ChainResolutionError) -> u8 {
    match value {
        ChainResolutionError::Empty => 0,
        ChainResolutionError::MissingPredecessor(_) => 1,
        ChainResolutionError::MultipleRoots => 2,
        ChainResolutionError::Fork(_) => 3,
        ChainResolutionError::Cycle => 4,
        ChainResolutionError::Disconnected => 5,
        ChainResolutionError::InvariantMutation(_) => 6,
        ChainResolutionError::InvalidTransition(_) => 7,
        ChainResolutionError::TimestampRegression => 8,
        ChainResolutionError::LimitExceeded => 9,
    }
}

#[test]
fn protocol_constants_are_centralized_and_provisional() {
    assert_eq!(EXPERIMENTAL_ENTITLEMENT_GRANT_KIND, 1030);
    assert_eq!(EXPERIMENTAL_ADP_CAMPAIGN_KIND, 1031);
    assert_ne!(EXPERIMENTAL_ENTITLEMENT_GRANT_KIND, 1021);
    assert_ne!(EXPERIMENTAL_ADP_CAMPAIGN_KIND, 1021);
    assert_ne!(
        EXPERIMENTAL_ENTITLEMENT_GRANT_KIND,
        EXPERIMENTAL_ADP_CAMPAIGN_KIND
    );
    assert_eq!(ADP_VERSION, "0.3.0");
}

#[test]
fn shared_protocol_types_are_public() {
    fn assert_contract<T: std::fmt::Debug + Clone + PartialEq + Eq>() {}

    assert_contract::<AcquisitionPolicy>();
    assert_contract::<AuthorizationEvent>();
    assert_contract::<AuthorizationTerms>();
    assert_contract::<AuthorizationTransition>();
    assert_contract::<CampaignEvent>();
    assert_contract::<CampaignMode>();
    assert_contract::<CampaignPointer>();
    assert_contract::<CampaignStatus>();
    assert_contract::<CampaignTerms>();
    assert_contract::<CampaignTransition>();
    assert_contract::<ChainResolutionError>();
    assert_contract::<GrantStatus>();
    assert_contract::<ParsedEntitlementGrant>();
    assert_contract::<ResolvedAuthorization>();
    assert_contract::<ResolvedCampaign>();
    assert_contract::<ResolvedEntitlementGrant>();
}

#[test]
fn public_struct_fields_and_variants_match_the_contract() {
    let developer = Keys::generate();
    let fulfillment = Keys::generate();
    let recipient = Keys::generate();
    let event = EventBuilder::new(Kind::TextNote, "contract")
        .sign_with_keys(&developer)
        .expect("event signs");
    let coordinate = format!("30402:{}:game", developer.public_key());
    let terms = CampaignTerms {
        mode: CampaignMode::Claim,
        starts: 10,
        ends: 20,
    };
    let campaign_event = CampaignEvent {
        event: event.clone(),
        campaign_id: "campaign".into(),
        coordinate: coordinate.clone(),
        predecessor: None,
        transition: CampaignTransition::Root(terms.clone()),
    };
    let campaign = ResolvedCampaign {
        root_event_id: event.id,
        developer_pubkey: developer.public_key(),
        campaign_id: campaign_event.campaign_id.clone(),
        coordinate: coordinate.clone(),
        events: vec![campaign_event],
    };
    assert_eq!(campaign.events.len(), 1);
    assert_eq!(CampaignStatus::Active, CampaignStatus::Active);
    assert_ne!(CampaignStatus::Active, CampaignStatus::Cancelled);
    assert!(matches!(
        CampaignTransition::ReplaceTerms(terms),
        CampaignTransition::ReplaceTerms(_)
    ));
    assert!(matches!(
        CampaignTransition::Cancel,
        CampaignTransition::Cancel
    ));

    let authorization_terms = AuthorizationTerms {
        authorization_id: "authorization".into(),
        coordinate: coordinate.clone(),
        fulfillment_pubkey: fulfillment.public_key(),
        valid_from: 1,
        valid_until: Some(30),
    };
    let authorization_event = AuthorizationEvent {
        event: event.clone(),
        transition_d: "transition".into(),
        authorization_id: authorization_terms.authorization_id.clone(),
        coordinate: coordinate.clone(),
        fulfillment_pubkey: fulfillment.public_key(),
        predecessor: None,
        transition: AuthorizationTransition::ActiveRoot(authorization_terms.clone()),
    };
    let authorization = ResolvedAuthorization {
        root_event_id: event.id,
        developer_pubkey: developer.public_key(),
        terms: authorization_terms,
        events: vec![authorization_event],
        revoked_at: None,
    };
    assert!(authorization.revoked_at.is_none());
    assert!(matches!(
        AuthorizationTransition::Revoke,
        AuthorizationTransition::Revoke
    ));

    let grant = ParsedEntitlementGrant {
        event: event.clone(),
        grant_id: "grant".into(),
        recipient: recipient.public_key(),
        coordinate: coordinate.clone(),
        source_event: Some(event.id),
        authorization_event: Some(event.id),
        reason: Some("promotional-claim".into()),
        status: GrantStatus::Granted,
        predecessor: None,
    };
    let resolved_grant = ResolvedEntitlementGrant {
        root_event_id: event.id,
        events: vec![grant],
    };
    assert_eq!(resolved_grant.events[0].status, GrantStatus::Granted);
    assert_ne!(GrantStatus::Granted, GrantStatus::Revoked);

    let pointer = CampaignPointer {
        root_event_id: event.id,
        relay_hint: Some(
            "wss://relay.example.com"
                .parse::<RelayUrl>()
                .expect("relay URL parses"),
        ),
    };
    assert!(pointer.relay_hint.is_some());
    assert!(matches!(AcquisitionPolicy::Gated, AcquisitionPolicy::Gated));
    assert!(matches!(
        AcquisitionPolicy::Public,
        AcquisitionPolicy::Public
    ));
    assert!(matches!(
        AcquisitionPolicy::TimedAccess { starts: 1, ends: 2 },
        AcquisitionPolicy::TimedAccess { .. }
    ));
    assert!(matches!(
        AcquisitionPolicy::Invalid,
        AcquisitionPolicy::Invalid
    ));

    assert_eq!(acquisition_discriminant(AcquisitionPolicy::Gated), 0);
    assert_eq!(
        authorization_discriminant(AuthorizationTransition::Revoke),
        1
    );
    assert_eq!(campaign_discriminant(CampaignTransition::Cancel), 2);
    assert_eq!(campaign_mode_discriminant(CampaignMode::Claim), 0);
    assert_eq!(campaign_status_discriminant(CampaignStatus::Cancelled), 1);
    assert_eq!(grant_status_discriminant(GrantStatus::Revoked), 1);
    assert_eq!(chain_error_discriminant(ChainResolutionError::Empty), 0);
    assert_eq!(
        chain_error_discriminant(ChainResolutionError::MissingPredecessor(event.id)),
        1
    );
    assert_eq!(
        chain_error_discriminant(ChainResolutionError::MultipleRoots),
        2
    );
    assert_eq!(
        chain_error_discriminant(ChainResolutionError::Fork(event.id)),
        3
    );
    assert_eq!(chain_error_discriminant(ChainResolutionError::Cycle), 4);
    assert_eq!(
        chain_error_discriminant(ChainResolutionError::Disconnected),
        5
    );
    assert_eq!(
        chain_error_discriminant(ChainResolutionError::InvariantMutation("d")),
        6
    );
    assert_eq!(
        chain_error_discriminant(ChainResolutionError::InvalidTransition("status".into())),
        7
    );
    assert_eq!(
        chain_error_discriminant(ChainResolutionError::TimestampRegression),
        8
    );
    assert_eq!(
        chain_error_discriminant(ChainResolutionError::LimitExceeded),
        9
    );
}
