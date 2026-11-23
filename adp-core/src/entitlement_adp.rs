//! ADP-specific authority checks for generic Entitlement Grant chains.
use nostr::PublicKey;

use crate::{
    campaign::resolve_campaign_at, entitlement::resolve_adp_entitlement_grant, AdpError,
    ResolvedAuthorization, ResolvedCampaign, ResolvedEntitlementGrant, CAPABILITY_ISSUE_GRANT,
};

fn adp_coordinate_developer(coordinate: &str) -> Option<PublicKey> {
    let mut parts = coordinate.splitn(3, ':');
    let kind = parts.next()?;
    let developer = parts.next()?;
    let identifier = parts.next()?;
    if kind != "30402" || identifier.is_empty() {
        return None;
    }
    PublicKey::from_hex(developer).ok()
}

/// Verifies portable ADP authority using only resolved, signed protocol state.
pub fn verify_adp_entitlement(
    grant: &ResolvedEntitlementGrant,
    campaign: &ResolvedCampaign,
    authorization: Option<&ResolvedAuthorization>,
) -> Result<(), AdpError> {
    let root_hint = grant.events.first().ok_or_else(|| {
        AdpError::InvalidEntitlementGrant("resolved grant has no root event".into())
    })?;
    let developer = adp_coordinate_developer(&root_hint.coordinate)
        .ok_or(AdpError::InvalidAdpGrantCoordinate)?;
    let resolved = resolve_adp_entitlement_grant(&grant.events, developer)
        .map_err(|error| AdpError::InvalidEntitlementGrant(error.to_string()))?;
    if resolved != *grant {
        return Err(AdpError::InvalidEntitlementGrant(
            "detached resolved metadata".into(),
        ));
    }

    let root = resolved.events.first().ok_or_else(|| {
        AdpError::InvalidEntitlementGrant("resolved grant has no root event".into())
    })?;
    let resolved_campaign = resolve_campaign_at(
        &campaign.events,
        campaign.developer_pubkey,
        &campaign.coordinate,
        root.event.created_at.as_secs(),
    )
    .map_err(|error| AdpError::InvalidCampaignDependency(error.to_string()))?;
    let source = root.source_event.ok_or(AdpError::MissingCampaignSource)?;
    if source != resolved_campaign.root_event_id {
        return Err(AdpError::CampaignRootMismatch);
    }
    if resolved_campaign.developer_pubkey != developer {
        return Err(AdpError::CampaignDeveloperMismatch);
    }
    if resolved_campaign.coordinate != root.coordinate {
        return Err(AdpError::CampaignCoordinateMismatch);
    }

    let issued_at = root.event.created_at.as_secs();
    if !resolved_campaign.is_claimable_at_trusted(issued_at) {
        return Err(AdpError::CampaignNotClaimableAtIssuance);
    }

    let root_signer = root.event.pubkey;
    match (root_signer == developer, root.authorization) {
        (true, None) => {}
        (true, Some(_)) => return Err(AdpError::GrantAuthorizationDenied),
        (false, None) => return Err(AdpError::MissingGrantAuthorizationAnchor),
        (false, Some(anchor)) => verify_authorization(
            authorization,
            anchor,
            &developer,
            &root_signer,
            &root.coordinate,
            issued_at,
        )?,
    }

    if resolved
        .events
        .iter()
        .skip(1)
        .any(|event| event.event.pubkey != developer)
    {
        return Err(AdpError::UnauthorizedGrantUpdateSigner);
    }

    Ok(())
}

fn verify_authorization(
    authorization: Option<&ResolvedAuthorization>,
    anchor: nostr::EventId,
    developer: &PublicKey,
    signer: &PublicKey,
    coordinate: &str,
    issued_at: u64,
) -> Result<(), AdpError> {
    let authorization = authorization.ok_or(AdpError::MissingResolvedGrantAuthorization)?;
    let resolved = crate::resolve_authorization_at(
        authorization.root_event_id,
        &authorization.signed_events(),
        issued_at,
    )
    .map_err(|error| AdpError::InvalidAuthorizationDependency(error.to_string()))?;
    if resolved.root_event_id != anchor {
        return Err(AdpError::AuthorizationRootMismatch);
    }
    if resolved.developer_pubkey != *developer {
        return Err(AdpError::AuthorizationDeveloperMismatch);
    }
    if !resolved.authorizes_trusted(signer, coordinate, CAPABILITY_ISSUE_GRANT, issued_at) {
        return Err(AdpError::GrantAuthorizationDenied);
    }
    Ok(())
}

/// Verifies a developer-signed ADP grant that carries no `source_event`
/// (ADP-01 §Entitlement Grants: developer-signed grants without a campaign).
///
/// Such a grant is valid on the developer's signature and the NIP-103 chain
/// checks alone: no campaign or issuance policy is consulted. It must have no
/// `authorization` anchor, because delegated grants require an independently
/// verifiable issuance policy, and every status update must be developer-signed.
/// A grant that carries `source_event` must instead be verified with
/// [`verify_adp_entitlement`].
pub fn verify_adp_developer_grant(grant: &ResolvedEntitlementGrant) -> Result<(), AdpError> {
    let root_hint = grant.events.first().ok_or_else(|| {
        AdpError::InvalidEntitlementGrant("resolved grant has no root event".into())
    })?;
    let developer = adp_coordinate_developer(&root_hint.coordinate)
        .ok_or(AdpError::InvalidAdpGrantCoordinate)?;
    let resolved = resolve_adp_entitlement_grant(&grant.events, developer)
        .map_err(|error| AdpError::InvalidEntitlementGrant(error.to_string()))?;
    if resolved != *grant {
        return Err(AdpError::InvalidEntitlementGrant(
            "detached resolved metadata".into(),
        ));
    }
    let root = resolved.events.first().ok_or_else(|| {
        AdpError::InvalidEntitlementGrant("resolved grant has no root event".into())
    })?;
    if root.source_event.is_some() {
        return Err(AdpError::InvalidEntitlementGrant(
            "a grant with source_event must be verified against its campaign".into(),
        ));
    }
    if root.event.pubkey != developer {
        // A delegated grant needs an issuance policy; a campaign is the only one.
        return Err(AdpError::MissingCampaignSource);
    }
    if root.authorization.is_some() {
        return Err(AdpError::GrantAuthorizationDenied);
    }
    if resolved
        .events
        .iter()
        .skip(1)
        .any(|event| event.event.pubkey != developer)
    {
        return Err(AdpError::UnauthorizedGrantUpdateSigner);
    }
    Ok(())
}
