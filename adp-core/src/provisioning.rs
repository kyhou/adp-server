use crate::AdpError;
use nostr::{Event, EventBuilder, EventId, Kind, PublicKey, Tag};

pub const ATTESTATION_KIND: u16 = 30404;
pub const ACCEPTANCE_KIND: u16 = 30406;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizationTerms {
    pub authorization_id: String,
    pub coordinate: String,
    pub fulfillment_pubkey: PublicKey,
    pub valid_from: u64,
    pub valid_until: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthorizationTransition {
    ActiveRoot(AuthorizationTerms),
    Revoke,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizationEvent {
    pub event: Event,
    pub transition_d: String,
    pub authorization_id: String,
    pub coordinate: String,
    pub fulfillment_pubkey: PublicKey,
    pub predecessor: Option<EventId>,
    pub transition: AuthorizationTransition,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedAuthorization {
    pub root_event_id: EventId,
    pub developer_pubkey: PublicKey,
    pub terms: AuthorizationTerms,
    pub events: Vec<AuthorizationEvent>,
    pub revoked_at: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedAttestation {
    pub developer_pubkey: PublicKey,
    pub fulfillment_pubkey: PublicKey,
    pub valid_from: u64,
    pub revoked_at: Option<u64>,
    pub scope: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedAcceptance {
    pub developer_pubkey: PublicKey,
    pub operator_pubkey: PublicKey,
    pub fulfillment_pubkey: PublicKey,
}

fn parse_tag(values: Vec<&str>) -> Result<Tag, AdpError> {
    Tag::parse(values).map_err(|e| AdpError::Nostr(e.to_string()))
}

pub fn build_attestation_event_builder(
    developer_pubkey: &PublicKey,
    fulfillment_pubkey: &PublicKey,
    valid_from: u64,
    revoked_at: Option<u64>,
    scope: Option<&str>,
) -> Result<EventBuilder, AdpError> {
    let d = format!(
        "{}:{}",
        developer_pubkey.to_hex(),
        fulfillment_pubkey.to_hex()
    );
    let valid_from = valid_from.to_string();
    let revoked_at = revoked_at.map(|r| r.to_string()).unwrap_or_default();
    let mut tags = vec![
        parse_tag(vec!["d", &d])?,
        parse_tag(vec!["p", &developer_pubkey.to_hex()])?,
        parse_tag(vec![
            "fulfillment_pubkey",
            &fulfillment_pubkey.to_hex(),
            &valid_from,
            &revoked_at,
        ])?,
    ];
    if let Some(scope) = scope.filter(|s| !s.is_empty()) {
        tags.push(parse_tag(vec!["scope", scope])?);
    }
    Ok(EventBuilder::new(Kind::Custom(ATTESTATION_KIND), "").tags(tags))
}

pub fn parse_attestation_event(event: &Event) -> Result<ParsedAttestation, AdpError> {
    if event.kind.as_u16() != ATTESTATION_KIND {
        return Err(AdpError::MissingTag("kind:30404"));
    }

    let mut d_tag = None;
    let mut developer_pubkey = None;
    let mut fulfillment_pubkey = None;
    let mut valid_from = None;
    let mut revoked_at = None;
    let mut scope = None;

    for tag in event.tags.iter() {
        let values = tag.clone().to_vec();
        match values.first().map(|s| s.as_str()) {
            Some("d") => d_tag = values.get(1).cloned(),
            Some("p") => {
                developer_pubkey = values.get(1).and_then(|p| PublicKey::from_hex(p).ok());
            }
            Some("fulfillment_pubkey") => {
                fulfillment_pubkey = values.get(1).and_then(|p| PublicKey::from_hex(p).ok());
                valid_from = values.get(2).and_then(|v| v.parse::<u64>().ok());
                revoked_at = values
                    .get(3)
                    .filter(|v| !v.is_empty())
                    .map(|v| v.parse::<u64>())
                    .transpose()
                    .map_err(|_| AdpError::MissingTag("fulfillment_pubkey.revoked_at"))?;
            }
            Some("scope") => scope = values.get(1).cloned(),
            _ => {}
        }
    }

    let developer_pubkey = developer_pubkey.ok_or(AdpError::MissingTag("p"))?;
    let fulfillment_pubkey =
        fulfillment_pubkey.ok_or(AdpError::MissingTag("fulfillment_pubkey"))?;
    let valid_from = valid_from.ok_or(AdpError::MissingTag("fulfillment_pubkey.valid_from"))?;
    let expected_d = format!(
        "{}:{}",
        developer_pubkey.to_hex(),
        fulfillment_pubkey.to_hex()
    );
    if d_tag.as_deref() != Some(expected_d.as_str()) {
        return Err(AdpError::CoordinateMismatch {
            expected: expected_d,
            found: d_tag.unwrap_or_default(),
        });
    }

    Ok(ParsedAttestation {
        developer_pubkey,
        fulfillment_pubkey,
        valid_from,
        revoked_at,
        scope,
    })
}

pub fn build_acceptance_event_builder(
    operator_pubkey: &PublicKey,
    fulfillment_pubkey: &PublicKey,
) -> Result<EventBuilder, AdpError> {
    let d = format!(
        "{}:{}",
        operator_pubkey.to_hex(),
        fulfillment_pubkey.to_hex()
    );
    let tags = vec![
        parse_tag(vec!["d", &d])?,
        parse_tag(vec!["p", &operator_pubkey.to_hex()])?,
        parse_tag(vec!["fulfillment_pubkey", &fulfillment_pubkey.to_hex()])?,
    ];
    Ok(EventBuilder::new(Kind::Custom(ACCEPTANCE_KIND), "").tags(tags))
}

pub fn parse_acceptance_event(event: &Event) -> Result<ParsedAcceptance, AdpError> {
    if event.kind.as_u16() != ACCEPTANCE_KIND {
        return Err(AdpError::MissingTag("kind:30406"));
    }

    let developer_pubkey = event.pubkey;
    let mut d_tag = None;
    let mut operator_pubkey = None;
    let mut fulfillment_pubkey = None;

    for tag in event.tags.iter() {
        let values = tag.clone().to_vec();
        match values.first().map(|s| s.as_str()) {
            Some("d") => d_tag = values.get(1).cloned(),
            Some("p") => operator_pubkey = values.get(1).and_then(|p| PublicKey::from_hex(p).ok()),
            Some("fulfillment_pubkey") => {
                fulfillment_pubkey = values.get(1).and_then(|p| PublicKey::from_hex(p).ok());
            }
            _ => {}
        }
    }

    let operator_pubkey = operator_pubkey.ok_or(AdpError::MissingTag("p"))?;
    let fulfillment_pubkey =
        fulfillment_pubkey.ok_or(AdpError::MissingTag("fulfillment_pubkey"))?;
    let expected_d = format!(
        "{}:{}",
        operator_pubkey.to_hex(),
        fulfillment_pubkey.to_hex()
    );
    if d_tag.as_deref() != Some(expected_d.as_str()) {
        return Err(AdpError::CoordinateMismatch {
            expected: expected_d,
            found: d_tag.unwrap_or_default(),
        });
    }

    Ok(ParsedAcceptance {
        developer_pubkey,
        operator_pubkey,
        fulfillment_pubkey,
    })
}
