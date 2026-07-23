use std::collections::{BTreeSet, HashMap};

use nostr::{Event, EventBuilder, EventId, Kind, PublicKey, Tag};

use crate::{AdpError, ChainResolutionError};

pub const ATTESTATION_KIND: u16 = 30404;
pub const AUTHORIZATION_KIND: u16 = 30406;
pub const ACCEPTANCE_KIND: u16 = AUTHORIZATION_KIND;

pub const CAPABILITY_ISSUE_RECEIPT: &str = "issue_receipt";
pub const CAPABILITY_ISSUE_GRANT: &str = "issue_grant";
pub const CAPABILITY_UPLOAD_BUILD: &str = "upload_build";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizationTerms {
    pub authorization_id: String,
    pub coordinate: String,
    pub operator_pubkey: PublicKey,
    pub fulfillment_pubkey: PublicKey,
    pub capabilities: BTreeSet<String>,
    pub valid_from: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorizationTransition {
    ActiveRoot,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizationEvent {
    pub event: Event,
    pub terms: AuthorizationTerms,
    pub predecessor: Option<EventId>,
    pub transition: AuthorizationTransition,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedAuthorization {
    pub root_event_id: EventId,
    pub developer_pubkey: PublicKey,
    pub terms: AuthorizationTerms,
    pub events: Vec<AuthorizationEvent>,
    pub cancelled_at: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedAttestation {
    pub developer_pubkey: PublicKey,
    pub fulfillment_pubkey: PublicKey,
    pub valid_from: u64,
    pub revoked_at: Option<u64>,
    pub scope: Option<String>,
}

fn parse_tag(values: Vec<&str>) -> Result<Tag, AdpError> {
    Tag::parse(values).map_err(|e| AdpError::Nostr(e.to_string()))
}

fn coordinate_developer(coordinate: &str) -> Option<PublicKey> {
    let mut parts = coordinate.splitn(3, ':');
    match (parts.next(), parts.next(), parts.next()) {
        (Some("30402"), Some(developer), Some(game_d)) if !game_d.is_empty() => {
            PublicKey::from_hex(developer).ok()
        }
        _ => None,
    }
}

fn exact_tag(
    event: &Event,
    name: &'static str,
    required: bool,
) -> Result<Option<String>, AdpError> {
    let matches = event
        .tags
        .iter()
        .map(|tag| tag.clone().to_vec())
        .filter(|values| values.first().is_some_and(|value| value == name))
        .collect::<Vec<_>>();
    if matches.len() > 1 {
        return Err(AdpError::DuplicateTag(name.into()));
    }
    let Some(values) = matches.first() else {
        return if required {
            Err(AdpError::MissingTag(name))
        } else {
            Ok(None)
        };
    };
    if values.len() != 2 || values[1].is_empty() {
        return Err(AdpError::MalformedTag(name.into()));
    }
    Ok(Some(values[1].clone()))
}

fn capabilities(event: &Event) -> Result<BTreeSet<String>, AdpError> {
    let mut values = BTreeSet::new();
    for tag in event.tags.iter() {
        let tag = tag.clone().to_vec();
        if tag.first().is_some_and(|name| name == "capability")
            && (tag.len() != 2 || tag[1].is_empty() || !values.insert(tag[1].clone()))
        {
            return Err(AdpError::MalformedTag("capability".into()));
        }
    }
    if values.is_empty() {
        return Err(AdpError::MissingTag("capability"));
    }
    Ok(values)
}

fn validate_terms(terms: &AuthorizationTerms) -> Result<(), AdpError> {
    if terms.authorization_id.is_empty() {
        return Err(AdpError::MalformedTag("d".into()));
    }
    if coordinate_developer(&terms.coordinate).is_none() {
        return Err(AdpError::MalformedTag("a".into()));
    }
    if terms.capabilities.is_empty() {
        return Err(AdpError::MissingTag("capability"));
    }
    Ok(())
}

fn reject_unknown_tags(event: &Event) -> Result<(), AdpError> {
    const ALLOWED: &[&str] = &[
        "d",
        "a",
        "p",
        "fulfillment_pubkey",
        "capability",
        "valid_from",
        "status",
        "e",
    ];
    for tag in event.tags.iter() {
        let values = tag.clone().to_vec();
        let name = values
            .first()
            .ok_or_else(|| AdpError::ForbiddenTag("tag".into()))?;
        if !ALLOWED.contains(&name.as_str()) {
            return Err(AdpError::ForbiddenTag(name.clone()));
        }
    }
    Ok(())
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
    if let Some(scope) = scope.filter(|scope| !scope.is_empty()) {
        tags.push(parse_tag(vec!["scope", scope])?);
    }
    Ok(EventBuilder::new(Kind::Custom(ATTESTATION_KIND), "").tags(tags))
}

pub fn parse_attestation_event(event: &Event) -> Result<ParsedAttestation, AdpError> {
    if event.kind.as_u16() != ATTESTATION_KIND {
        return Err(AdpError::MissingTag("kind:30404"));
    }
    let mut d_tag = None;
    let mut developer = None;
    let mut fulfillment = None;
    let mut valid_from = None;
    let mut revoked_at = None;
    let mut scope = None;
    for tag in event.tags.iter() {
        let values = tag.clone().to_vec();
        match values.first().map(String::as_str) {
            Some("d") => d_tag = values.get(1).cloned(),
            Some("p") => {
                developer = values
                    .get(1)
                    .and_then(|value| PublicKey::from_hex(value).ok())
            }
            Some("fulfillment_pubkey") => {
                fulfillment = values
                    .get(1)
                    .and_then(|value| PublicKey::from_hex(value).ok());
                valid_from = values.get(2).and_then(|value| value.parse().ok());
                revoked_at = values
                    .get(3)
                    .filter(|value| !value.is_empty())
                    .map(|value| value.parse::<u64>())
                    .transpose()
                    .map_err(|_| AdpError::MalformedTag("fulfillment_pubkey".into()))?;
            }
            Some("scope") => scope = values.get(1).cloned(),
            _ => {}
        }
    }
    let developer_pubkey = developer.ok_or(AdpError::MissingTag("p"))?;
    let fulfillment_pubkey = fulfillment.ok_or(AdpError::MissingTag("fulfillment_pubkey"))?;
    let valid_from = valid_from.ok_or(AdpError::MissingTag("valid_from"))?;
    let expected = format!(
        "{}:{}",
        developer_pubkey.to_hex(),
        fulfillment_pubkey.to_hex()
    );
    if d_tag.as_deref() != Some(&expected) {
        return Err(AdpError::CoordinateMismatch {
            expected,
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

pub fn build_authorization_root_event_builder(
    terms: &AuthorizationTerms,
) -> Result<EventBuilder, AdpError> {
    validate_terms(terms)?;
    let mut tags = vec![
        parse_tag(vec!["d", &terms.authorization_id])?,
        parse_tag(vec!["a", &terms.coordinate])?,
        parse_tag(vec!["p", &terms.operator_pubkey.to_hex()])?,
        parse_tag(vec![
            "fulfillment_pubkey",
            &terms.fulfillment_pubkey.to_hex(),
        ])?,
    ];
    for capability in &terms.capabilities {
        tags.push(parse_tag(vec!["capability", capability])?);
    }
    tags.extend([
        parse_tag(vec!["valid_from", &terms.valid_from.to_string()])?,
        parse_tag(vec!["status", "active"])?,
    ]);
    Ok(EventBuilder::new(Kind::Custom(AUTHORIZATION_KIND), "").tags(tags))
}

pub fn build_authorization_cancellation_event_builder(
    terms: &AuthorizationTerms,
    predecessor_id: EventId,
) -> Result<EventBuilder, AdpError> {
    validate_terms(terms)?;
    let mut tags = vec![
        parse_tag(vec!["d", &terms.authorization_id])?,
        parse_tag(vec!["a", &terms.coordinate])?,
        parse_tag(vec!["p", &terms.operator_pubkey.to_hex()])?,
        parse_tag(vec![
            "fulfillment_pubkey",
            &terms.fulfillment_pubkey.to_hex(),
        ])?,
    ];
    for capability in &terms.capabilities {
        tags.push(parse_tag(vec!["capability", capability])?);
    }
    tags.extend([
        parse_tag(vec!["valid_from", &terms.valid_from.to_string()])?,
        parse_tag(vec!["status", "cancelled"])?,
        parse_tag(vec!["e", &predecessor_id.to_hex()])?,
    ]);
    Ok(EventBuilder::new(Kind::Custom(AUTHORIZATION_KIND), "").tags(tags))
}

pub fn parse_authorization_event(event: &Event) -> Result<AuthorizationEvent, AdpError> {
    if event.kind.as_u16() != AUTHORIZATION_KIND {
        return Err(AdpError::WrongKind {
            expected: AUTHORIZATION_KIND,
            found: event.kind.as_u16(),
        });
    }
    event.verify().map_err(|_| AdpError::InvalidSignature)?;
    reject_unknown_tags(event)?;
    let authorization_id = exact_tag(event, "d", true)?.ok_or(AdpError::MissingTag("d"))?;
    let coordinate = exact_tag(event, "a", true)?.ok_or(AdpError::MissingTag("a"))?;
    let operator_pubkey =
        PublicKey::from_hex(&exact_tag(event, "p", true)?.ok_or(AdpError::MissingTag("p"))?)
            .map_err(|_| AdpError::MalformedTag("p".into()))?;
    let fulfillment_pubkey = PublicKey::from_hex(
        &exact_tag(event, "fulfillment_pubkey", true)?
            .ok_or(AdpError::MissingTag("fulfillment_pubkey"))?,
    )
    .map_err(|_| AdpError::MalformedTag("fulfillment_pubkey".into()))?;
    let capabilities = capabilities(event)?;
    let valid_from = exact_tag(event, "valid_from", true)?
        .ok_or(AdpError::MissingTag("valid_from"))?
        .parse::<u64>()
        .map_err(|_| AdpError::MalformedTag("valid_from".into()))?;
    let status = exact_tag(event, "status", true)?.ok_or(AdpError::MissingTag("status"))?;
    let predecessor = exact_tag(event, "e", false)?
        .map(|value| EventId::from_hex(&value).map_err(|_| AdpError::MalformedTag("e".into())))
        .transpose()?;
    let developer =
        coordinate_developer(&coordinate).ok_or_else(|| AdpError::MalformedTag("a".into()))?;
    if developer != event.pubkey {
        return Err(AdpError::CoordinateMismatch {
            expected: format!("30402:{}:<d>", event.pubkey),
            found: coordinate,
        });
    }
    let transition = match (status.as_str(), predecessor) {
        ("active", None) => AuthorizationTransition::ActiveRoot,
        ("cancelled", Some(_)) => AuthorizationTransition::Cancel,
        ("active", Some(_)) => return Err(AdpError::ForbiddenTag("e".into())),
        ("cancelled", None) => return Err(AdpError::MissingTag("e")),
        _ => return Err(AdpError::MalformedTag("status".into())),
    };
    let terms = AuthorizationTerms {
        authorization_id,
        coordinate,
        operator_pubkey,
        fulfillment_pubkey,
        capabilities,
        valid_from,
    };
    validate_terms(&terms)?;
    if transition == AuthorizationTransition::ActiveRoot && valid_from < event.created_at.as_secs()
    {
        return Err(AdpError::MalformedTag("valid_from".into()));
    }
    Ok(AuthorizationEvent {
        event: event.clone(),
        terms,
        predecessor,
        transition,
    })
}

pub fn resolve_authorization(
    expected_root_id: EventId,
    events: &[Event],
) -> Result<ResolvedAuthorization, ChainResolutionError> {
    resolve_authorization_at(expected_root_id, events, u64::MAX)
}

pub fn resolve_authorization_at(
    expected_root_id: EventId,
    events: &[Event],
    at: u64,
) -> Result<ResolvedAuthorization, ChainResolutionError> {
    let by_id = events
        .iter()
        .filter(|event| event.created_at.as_secs() <= at)
        .map(|event| (event.id, event))
        .collect::<HashMap<_, _>>();
    let root_event = by_id
        .get(&expected_root_id)
        .ok_or(ChainResolutionError::Empty)?;
    let root = parse_authorization_event(root_event).map_err(|error| {
        ChainResolutionError::InvalidTransition(format!("invalid authorization root: {error}"))
    })?;
    if root.transition != AuthorizationTransition::ActiveRoot || root.predecessor.is_some() {
        return Err(ChainResolutionError::InvalidTransition(
            "authorization root must be active".into(),
        ));
    }

    let mut raw_successors: HashMap<EventId, Vec<&Event>> = HashMap::new();
    for event in by_id.values() {
        for tag in event.tags.iter() {
            let values = tag.clone().to_vec();
            if values.len() == 2 && values[0] == "e" {
                if let Ok(predecessor) = EventId::from_hex(&values[1]) {
                    raw_successors.entry(predecessor).or_default().push(event);
                }
            }
        }
    }

    let mut ordered = vec![root.clone()];
    let mut valid = Vec::new();
    for candidate in raw_successors.get(&root.event.id).into_iter().flatten() {
        let Ok(parsed) = parse_authorization_event(candidate) else {
            continue;
        };
        if parsed.predecessor == Some(root.event.id)
            && parsed.transition == AuthorizationTransition::Cancel
            && parsed.terms == root.terms
            && parsed.event.created_at > root.event.created_at
        {
            valid.push(parsed);
        }
    }
    if valid.len() > 1 {
        return Err(ChainResolutionError::Fork(root.event.id));
    }
    if let Some(next) = valid.pop() {
        ordered.push(next);
    }
    let cancelled_at = ordered.get(1).map(|event| event.event.created_at.as_secs());
    Ok(ResolvedAuthorization {
        root_event_id: expected_root_id,
        developer_pubkey: ordered[0].event.pubkey,
        terms: ordered[0].terms.clone(),
        events: ordered,
        cancelled_at,
    })
}

impl ResolvedAuthorization {
    pub fn signed_events(&self) -> Vec<Event> {
        self.events
            .iter()
            .map(|event| event.event.clone())
            .collect()
    }

    pub fn has_capability(&self, capability: &str) -> bool {
        matches!(
            capability,
            CAPABILITY_ISSUE_RECEIPT | CAPABILITY_ISSUE_GRANT | CAPABILITY_UPLOAD_BUILD
        ) && self.terms.capabilities.contains(capability)
    }

    pub fn authorizes(
        &self,
        signer: &PublicKey,
        coordinate: &str,
        capability: &str,
        at: u64,
    ) -> bool {
        signer == &self.terms.fulfillment_pubkey
            && coordinate == self.terms.coordinate
            && self.terms.valid_from <= at
            && self.cancelled_at.is_none_or(|cancelled| at < cancelled)
            && self.has_capability(capability)
    }

    pub(crate) fn authorizes_trusted(
        &self,
        signer: &PublicKey,
        coordinate: &str,
        capability: &str,
        at: u64,
    ) -> bool {
        signer == &self.terms.fulfillment_pubkey
            && coordinate == self.terms.coordinate
            && self.terms.valid_from <= at
            && self.cancelled_at.is_none_or(|cancelled| at < cancelled)
            && self.has_capability(capability)
    }
}
