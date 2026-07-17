use crate::{AdpError, ChainResolutionError};
use nostr::{Event, EventBuilder, EventId, Kind, PublicKey, Tag};
use std::collections::{HashMap, HashSet};

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

fn exact_authorization_tag(
    event: &Event,
    name: &str,
    required: bool,
) -> Result<Option<String>, AdpError> {
    let matches: Vec<Vec<String>> = event
        .tags
        .iter()
        .map(|tag| tag.clone().to_vec())
        .filter(|values| values.first().is_some_and(|value| value == name))
        .collect();
    if matches.len() > 1 {
        return Err(AdpError::DuplicateTag(name.into()));
    }
    let Some(tag) = matches.first() else {
        return if required {
            Err(AdpError::MissingTag(match name {
                "d" => "d",
                "authorization_id" => "authorization_id",
                "a" => "a",
                "p" => "p",
                "valid_from" => "valid_from",
                "status" => "status",
                "e" => "e",
                _ => "authorization tag",
            }))
        } else {
            Ok(None)
        };
    };
    let values = tag;
    if values.len() != 2 || values[1].is_empty() {
        return Err(AdpError::MalformedTag(name.into()));
    }
    Ok(Some(values[1].clone()))
}

fn authorization_coordinate_developer(coordinate: &str) -> Option<PublicKey> {
    let mut parts = coordinate.splitn(3, ':');
    match (parts.next(), parts.next(), parts.next()) {
        (Some("30402"), Some(developer), Some(game_d)) if !game_d.is_empty() => {
            PublicKey::from_hex(developer).ok()
        }
        _ => None,
    }
}

fn validate_authorization_terms(terms: &AuthorizationTerms) -> Result<(), AdpError> {
    if terms.authorization_id.is_empty() {
        return Err(AdpError::MalformedTag("authorization_id".into()));
    }
    if authorization_coordinate_developer(&terms.coordinate).is_none() {
        return Err(AdpError::MalformedTag("a".into()));
    }
    if terms
        .valid_until
        .is_some_and(|until| until <= terms.valid_from)
    {
        return Err(AdpError::MalformedTag("valid_until".into()));
    }
    Ok(())
}

fn reject_unlisted_authorization_tags(event: &Event, allowed: &[&str]) -> Result<(), AdpError> {
    for tag in event.tags.iter() {
        let values = tag.clone().to_vec();
        let Some(name) = values.first() else {
            return Err(AdpError::ForbiddenTag("tag".into()));
        };
        if !allowed.contains(&name.as_str()) {
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

pub fn build_authorization_root_event_builder(
    terms: &AuthorizationTerms,
    transition_d: &str,
) -> Result<EventBuilder, AdpError> {
    validate_authorization_terms(terms)?;
    if transition_d.is_empty() {
        return Err(AdpError::MalformedTag("d".into()));
    }
    let valid_from = terms.valid_from.to_string();
    let mut tags = vec![
        parse_tag(vec!["d", transition_d])?,
        parse_tag(vec!["authorization_id", &terms.authorization_id])?,
        parse_tag(vec!["a", &terms.coordinate])?,
        parse_tag(vec!["p", &terms.fulfillment_pubkey.to_hex()])?,
        parse_tag(vec!["valid_from", &valid_from])?,
        parse_tag(vec!["status", "active"])?,
    ];
    if let Some(valid_until) = terms.valid_until {
        tags.push(parse_tag(vec!["valid_until", &valid_until.to_string()])?);
    }
    Ok(EventBuilder::new(Kind::Custom(ACCEPTANCE_KIND), "").tags(tags))
}

pub fn build_authorization_revocation_event_builder(
    root_terms: &AuthorizationTerms,
    transition_d: &str,
    predecessor_id: EventId,
) -> Result<EventBuilder, AdpError> {
    validate_authorization_terms(root_terms)?;
    if transition_d.is_empty() {
        return Err(AdpError::MalformedTag("d".into()));
    }
    let tags = vec![
        parse_tag(vec!["d", transition_d])?,
        parse_tag(vec!["authorization_id", &root_terms.authorization_id])?,
        parse_tag(vec!["a", &root_terms.coordinate])?,
        parse_tag(vec!["p", &root_terms.fulfillment_pubkey.to_hex()])?,
        parse_tag(vec!["status", "revoked"])?,
        parse_tag(vec!["e", &predecessor_id.to_hex()])?,
    ];
    Ok(EventBuilder::new(Kind::Custom(ACCEPTANCE_KIND), "").tags(tags))
}

pub fn parse_authorization_event(event: &Event) -> Result<AuthorizationEvent, AdpError> {
    if event.kind.as_u16() != ACCEPTANCE_KIND {
        return Err(AdpError::MissingTag("kind:30406"));
    }
    if event.verify().is_err() {
        return Err(AdpError::InvalidSignature);
    }

    let transition_d =
        exact_authorization_tag(event, "d", true)?.ok_or(AdpError::MissingTag("d"))?;
    let authorization_id = exact_authorization_tag(event, "authorization_id", true)?
        .ok_or(AdpError::MissingTag("authorization_id"))?;
    let coordinate = exact_authorization_tag(event, "a", true)?.ok_or(AdpError::MissingTag("a"))?;
    let fulfillment_pubkey = exact_authorization_tag(event, "p", true)?
        .and_then(|value| PublicKey::from_hex(&value).ok())
        .ok_or_else(|| AdpError::MalformedTag("p".into()))?;
    let status =
        exact_authorization_tag(event, "status", true)?.ok_or(AdpError::MissingTag("status"))?;
    let predecessor = exact_authorization_tag(event, "e", false)?
        .map(|value| EventId::from_hex(&value).map_err(|_| AdpError::MalformedTag("e".into())))
        .transpose()?;

    let coordinate_developer =
        authorization_coordinate_developer(&coordinate).ok_or_else(|| {
            AdpError::CoordinateMismatch {
                expected: format!("30402:{}:<game-d>", event.pubkey),
                found: coordinate.clone(),
            }
        })?;
    if coordinate_developer != event.pubkey {
        return Err(AdpError::CoordinateMismatch {
            expected: format!("30402:{}:<game-d>", event.pubkey),
            found: coordinate,
        });
    }

    let transition = match (status.as_str(), predecessor) {
        ("active", None) => {
            reject_unlisted_authorization_tags(
                event,
                &[
                    "d",
                    "authorization_id",
                    "a",
                    "p",
                    "valid_from",
                    "valid_until",
                    "status",
                ],
            )?;
            let valid_from = exact_authorization_tag(event, "valid_from", true)?
                .and_then(|value| value.parse::<u64>().ok())
                .ok_or_else(|| AdpError::MalformedTag("valid_from".into()))?;
            let valid_until = exact_authorization_tag(event, "valid_until", false)?
                .map(|value| {
                    value
                        .parse::<u64>()
                        .map_err(|_| AdpError::MalformedTag("valid_until".into()))
                })
                .transpose()?;
            let terms = AuthorizationTerms {
                authorization_id: authorization_id.clone(),
                coordinate: coordinate.clone(),
                fulfillment_pubkey,
                valid_from,
                valid_until,
            };
            validate_authorization_terms(&terms)?;
            AuthorizationTransition::ActiveRoot(terms)
        }
        ("active", Some(_)) => return Err(AdpError::ForbiddenTag("e".into())),
        ("revoked", Some(_)) => {
            reject_unlisted_authorization_tags(
                event,
                &["d", "authorization_id", "a", "p", "status", "e"],
            )?;
            AuthorizationTransition::Revoke
        }
        ("revoked", None) => return Err(AdpError::MissingTag("e")),
        _ => return Err(AdpError::MalformedTag("status".into())),
    };

    Ok(AuthorizationEvent {
        event: event.clone(),
        transition_d,
        authorization_id,
        coordinate,
        fulfillment_pubkey,
        predecessor,
        transition,
    })
}

pub fn resolve_authorization(
    expected_root_id: EventId,
    nodes: &[AuthorizationEvent],
) -> Result<ResolvedAuthorization, ChainResolutionError> {
    let reparsed = nodes
        .iter()
        .map(|node| {
            let parsed = parse_authorization_event(&node.event).map_err(|error| {
                ChainResolutionError::InvalidTransition(format!(
                    "invalid signed authorization event: {error}"
                ))
            })?;
            if parsed != *node {
                return Err(ChainResolutionError::InvariantMutation(
                    "signed event metadata",
                ));
            }
            Ok(parsed)
        })
        .collect::<Result<Vec<_>, _>>()?;

    resolve_parsed_authorization(&reparsed, expected_root_id)
}

fn resolve_parsed_authorization(
    nodes: &[AuthorizationEvent],
    expected_root_id: EventId,
) -> Result<ResolvedAuthorization, ChainResolutionError> {
    if nodes.is_empty() {
        return Err(ChainResolutionError::Empty);
    }
    if nodes
        .iter()
        .any(|node| node.predecessor == Some(node.event.id))
    {
        return Err(ChainResolutionError::Cycle);
    }

    let by_id: HashMap<EventId, &AuthorizationEvent> =
        nodes.iter().map(|node| (node.event.id, node)).collect();
    if by_id.len() != nodes.len() {
        return Err(ChainResolutionError::Disconnected);
    }
    for node in nodes {
        if let Some(predecessor) = node.predecessor {
            if !by_id.contains_key(&predecessor) {
                return Err(ChainResolutionError::MissingPredecessor(predecessor));
            }
        }
    }

    let roots = nodes
        .iter()
        .filter(|node| node.predecessor.is_none())
        .collect::<Vec<_>>();
    if roots.is_empty() {
        return Err(ChainResolutionError::Cycle);
    }
    if roots.len() > 1 {
        return Err(ChainResolutionError::MultipleRoots);
    }
    let root = roots[0];
    if root.event.id != expected_root_id {
        return Err(ChainResolutionError::Disconnected);
    }
    let terms = match &root.transition {
        AuthorizationTransition::ActiveRoot(terms) => terms.clone(),
        AuthorizationTransition::Revoke => {
            return Err(ChainResolutionError::InvalidTransition(
                "authorization root must be active".into(),
            ));
        }
    };
    validate_authorization_terms(&terms).map_err(|error| {
        ChainResolutionError::InvalidTransition(format!("invalid authorization terms: {error}"))
    })?;

    let developer = root.event.pubkey;
    let mut transition_ds = HashSet::with_capacity(nodes.len());
    for node in nodes {
        if node.event.kind.as_u16() != ACCEPTANCE_KIND {
            return Err(ChainResolutionError::InvariantMutation("kind"));
        }
        if node.event.pubkey != developer {
            return Err(ChainResolutionError::InvariantMutation("developer"));
        }
        if node.authorization_id != terms.authorization_id {
            return Err(ChainResolutionError::InvariantMutation("authorization_id"));
        }
        if node.coordinate != terms.coordinate {
            return Err(ChainResolutionError::InvariantMutation("a"));
        }
        if node.fulfillment_pubkey != terms.fulfillment_pubkey {
            return Err(ChainResolutionError::InvariantMutation("p"));
        }
        if !transition_ds.insert(node.transition_d.as_str()) {
            return Err(ChainResolutionError::InvalidTransition(
                "duplicate transition d".into(),
            ));
        }
    }

    let mut successors = HashMap::new();
    for node in nodes {
        if let Some(predecessor) = node.predecessor {
            if successors.insert(predecessor, node.event.id).is_some() {
                return Err(ChainResolutionError::Fork(predecessor));
            }
        }
    }

    let mut ordered = Vec::with_capacity(nodes.len());
    let mut visited = HashSet::with_capacity(nodes.len());
    let mut current = root.event.id;
    loop {
        if !visited.insert(current) {
            return Err(ChainResolutionError::Cycle);
        }
        ordered.push((*by_id[&current]).clone());
        match successors.get(&current) {
            Some(next) => current = *next,
            None => break,
        }
    }
    if ordered.len() != nodes.len() {
        return Err(ChainResolutionError::Disconnected);
    }
    if ordered.len() > 2 {
        return Err(ChainResolutionError::InvalidTransition(
            "revocation is terminal".into(),
        ));
    }

    let revoked_at = if let Some(revoke) = ordered.get(1) {
        if revoke.event.created_at <= ordered[0].event.created_at {
            return Err(ChainResolutionError::TimestampRegression);
        }
        if !matches!(revoke.transition, AuthorizationTransition::Revoke) {
            return Err(ChainResolutionError::InvalidTransition(
                "successor must revoke authorization".into(),
            ));
        }
        Some(revoke.event.created_at.as_secs())
    } else {
        None
    };

    Ok(ResolvedAuthorization {
        root_event_id: root.event.id,
        developer_pubkey: developer,
        terms,
        events: ordered,
        revoked_at,
    })
}

impl ResolvedAuthorization {
    pub fn authorizes(&self, signer: &PublicKey, coordinate: &str, at: u64) -> bool {
        let Ok(resolved) = resolve_authorization(self.root_event_id, &self.events) else {
            return false;
        };
        if resolved != *self {
            return false;
        }
        resolved.authorizes_trusted(signer, coordinate, at)
    }

    pub(crate) fn authorizes_trusted(&self, signer: &PublicKey, coordinate: &str, at: u64) -> bool {
        signer == &self.terms.fulfillment_pubkey
            && coordinate == self.terms.coordinate
            && self.terms.valid_from <= at
            && self.terms.valid_until.is_none_or(|until| at < until)
            && self.revoked_at.is_none_or(|revoked_at| at < revoked_at)
    }
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

#[cfg(test)]
mod authorization_topology_tests {
    use super::*;
    use crate::ChainResolutionError;
    use nostr::{Keys, Timestamp};

    fn node(
        keys: &Keys,
        d: &str,
        predecessor: Option<EventId>,
        created_at: u64,
    ) -> AuthorizationEvent {
        let event = EventBuilder::new(Kind::Custom(ACCEPTANCE_KIND), "")
            .custom_created_at(Timestamp::from_secs(created_at))
            .sign_with_keys(keys)
            .unwrap();
        let terms = AuthorizationTerms {
            authorization_id: "authorization".into(),
            coordinate: format!("30402:{}:game", keys.public_key()),
            fulfillment_pubkey: keys.public_key(),
            valid_from: 1,
            valid_until: None,
        };
        AuthorizationEvent {
            event,
            transition_d: d.into(),
            authorization_id: terms.authorization_id.clone(),
            coordinate: terms.coordinate.clone(),
            fulfillment_pubkey: terms.fulfillment_pubkey,
            predecessor,
            transition: if predecessor.is_none() {
                AuthorizationTransition::ActiveRoot(terms)
            } else {
                AuthorizationTransition::Revoke
            },
        }
    }

    #[test]
    fn pure_topology_rejects_cycle() {
        let developer = Keys::generate();
        let mut first = node(&developer, "first", None, 1);
        let mut second = node(&developer, "second", Some(first.event.id), 2);
        first.predecessor = Some(second.event.id);
        second.transition = AuthorizationTransition::Revoke;

        assert!(matches!(
            resolve_parsed_authorization(&[first.clone(), second], first.event.id),
            Err(ChainResolutionError::Cycle)
        ));
    }

    #[test]
    fn pure_topology_rejects_self_reference() {
        let developer = Keys::generate();
        let mut root = node(&developer, "root", None, 1);
        root.predecessor = Some(root.event.id);
        root.transition = AuthorizationTransition::Revoke;

        assert!(matches!(
            resolve_parsed_authorization(&[root.clone()], root.event.id),
            Err(ChainResolutionError::Cycle)
        ));
    }

    #[test]
    fn pure_topology_rejects_disconnected_component() {
        let developer = Keys::generate();
        let root = node(&developer, "root", None, 1);
        let mut first = node(&developer, "first", Some(root.event.id), 2);
        let second = node(&developer, "second", Some(first.event.id), 3);
        first.predecessor = Some(second.event.id);

        assert!(matches!(
            resolve_parsed_authorization(&[root.clone(), first, second], root.event.id),
            Err(ChainResolutionError::Disconnected | ChainResolutionError::Cycle)
        ));
    }
}
