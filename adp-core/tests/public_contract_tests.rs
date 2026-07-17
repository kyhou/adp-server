use adp_core::{
    AcquisitionPolicy, AuthorizationTransition, CampaignTransition, ChainResolutionError,
    GrantStatus, ADP_VERSION, EXPERIMENTAL_ADP_CAMPAIGN_KIND, EXPERIMENTAL_ENTITLEMENT_GRANT_KIND,
};

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
    let _ = std::mem::size_of::<AcquisitionPolicy>();
    let _ = std::mem::size_of::<AuthorizationTransition>();
    let _ = std::mem::size_of::<CampaignTransition>();
    let _ = std::mem::size_of::<ChainResolutionError>();
    let _ = std::mem::size_of::<GrantStatus>();
}
