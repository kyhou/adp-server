use adp_core::{
    ADP_VERSION, AUTHORIZATION_KIND, EXPERIMENTAL_ADP_CAMPAIGN_KIND,
    EXPERIMENTAL_ENTITLEMENT_GRANT_KIND,
};

#[test]
fn protocol_constants_are_centralized_and_provisional() {
    assert_eq!(AUTHORIZATION_KIND, 30406);
    assert_ne!(EXPERIMENTAL_ENTITLEMENT_GRANT_KIND, 1021);
    assert_eq!(EXPERIMENTAL_ENTITLEMENT_GRANT_KIND, 1030);
    assert_eq!(EXPERIMENTAL_ADP_CAMPAIGN_KIND, 1031);
    assert!(!ADP_VERSION.is_empty());
}

#[test]
fn shared_protocol_types_remain_public() {
    fn contract<T: std::fmt::Debug + Clone + PartialEq + Eq>() {}
    contract::<adp_core::FulfillmentAuthorization>();
    contract::<adp_core::AuthorizationTerms>();
    contract::<adp_core::AuthorizationEvent>();
    contract::<adp_core::ResolvedAuthorization>();
    contract::<adp_core::CampaignTransition>();
    contract::<adp_core::ParsedEntitlementGrant>();
    contract::<adp_core::ParsedReceipt>();
}
