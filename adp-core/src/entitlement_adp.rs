//! ADP-specific authority checks for generic Entitlement Grant chains.
use nostr::PublicKey;

use crate::{
    entitlement::resolve_entitlement_grant, AdpError, ResolvedAuthorization, ResolvedCampaign,
    ResolvedEntitlementGrant,
};

fn adp_coordinate_developer(coordinate: &str) -> Option<PublicKey> {
    let mut parts = coordinate.split(':');
    let kind = parts.next()?;
    let developer = parts.next()?;
    let identifier = parts.next()?;
    if kind != "30402" || identifier.is_empty() || parts.next().is_some() {
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
    let resolved = resolve_entitlement_grant(&grant.events)
        .map_err(|error| AdpError::InvalidEntitlementGrant(error.to_string()))?;
    if resolved != *grant {
        return Err(AdpError::InvalidEntitlementGrant(
            "detached resolved metadata".into(),
        ));
    }

    let root = resolved.events.first().ok_or_else(|| {
        AdpError::InvalidEntitlementGrant("resolved grant has no root event".into())
    })?;
    let developer =
        adp_coordinate_developer(&root.coordinate).ok_or(AdpError::InvalidAdpGrantCoordinate)?;

    let source = root.source_event.ok_or(AdpError::MissingCampaignSource)?;
    if source != campaign.root_event_id {
        return Err(AdpError::CampaignRootMismatch);
    }
    if campaign.developer_pubkey != developer {
        return Err(AdpError::CampaignDeveloperMismatch);
    }
    if campaign.coordinate != root.coordinate {
        return Err(AdpError::CampaignCoordinateMismatch);
    }

    let issued_at = root.event.created_at.as_secs();
    if !campaign.is_claimable_at(issued_at) {
        return Err(AdpError::CampaignNotClaimableAtIssuance);
    }

    let root_signer = root.event.pubkey;
    match (root_signer == developer, root.authorization_event) {
        (true, None) => {}
        (true, Some(anchor)) => verify_authorization(
            authorization,
            anchor,
            &developer,
            &root_signer,
            &root.coordinate,
            issued_at,
        )?,
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
    if authorization.root_event_id != anchor {
        return Err(AdpError::AuthorizationRootMismatch);
    }
    if authorization.developer_pubkey != *developer {
        return Err(AdpError::AuthorizationDeveloperMismatch);
    }
    if !authorization.authorizes(signer, coordinate, issued_at) {
        return Err(AdpError::GrantAuthorizationDenied);
    }
    Ok(())
}
