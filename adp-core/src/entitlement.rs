use std::collections::{HashMap, HashSet};

use nostr::nips::nip01::Coordinate;
use nostr::{Event, EventBuilder, EventId, Kind, PublicKey, Tag};
use thiserror::Error;

use crate::{AdpError, ChainResolutionError, EXPERIMENTAL_ENTITLEMENT_GRANT_KIND};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantStatus {
    Granted,
    Revoked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildEntitlementParams {
    pub grant_id: String,
    pub recipient: PublicKey,
    pub coordinate: String,
    pub source_event: Option<EventId>,
    pub authorization: Option<EventId>,
    pub reason: Option<String>,
    pub status: GrantStatus,
    pub predecessor: Option<EventId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedEntitlementGrant {
    pub event: Event,
    pub grant_id: String,
    pub recipient: PublicKey,
    pub coordinate: String,
    pub source_event: Option<EventId>,
    pub authorization: Option<EventId>,
    pub reason: Option<String>,
    pub status: GrantStatus,
    pub predecessor: Option<EventId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedEntitlementGrant {
    pub root_event_id: EventId,
    pub events: Vec<ParsedEntitlementGrant>,
}

#[derive(Debug, Error)]
pub enum EntitlementResolutionError {
    #[error("invalid entitlement event: {0}")]
    Parse(#[from] AdpError),
    #[error(transparent)]
    Chain(#[from] ChainResolutionError),
}

fn entitlement_tag(values: Vec<String>) -> Result<Tag, AdpError> {
    let name = values.first().cloned().unwrap_or_else(|| "tag".into());
    Tag::parse(values).map_err(|_| AdpError::MalformedTag(name))
}

fn exact_tag(event: &Event, name: &str, required: bool) -> Result<Option<String>, AdpError> {
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
            Err(AdpError::MissingTag(match name {
                "d" => "d",
                "p" => "p",
                "a" => "a",
                "status" => "status",
                "e" => "e",
                _ => "entitlement tag",
            }))
        } else {
            Ok(None)
        };
    };
    if values.len() != 2 || values[1].is_empty() {
        return Err(AdpError::MalformedTag(name.into()));
    }
    Ok(Some(values[1].clone()))
}

fn validate_coordinate(value: &str) -> Result<(), AdpError> {
    let mut parts = value.splitn(3, ':');
    let kind = parts
        .next()
        .and_then(|kind| kind.parse::<u16>().ok())
        .map(Kind::from)
        .ok_or_else(|| AdpError::MalformedTag("a".into()))?;
    let public_key = parts
        .next()
        .and_then(|public_key| PublicKey::from_hex(public_key).ok())
        .ok_or_else(|| AdpError::MalformedTag("a".into()))?;
    let identifier = parts
        .next()
        .ok_or_else(|| AdpError::MalformedTag("a".into()))?;
    Coordinate {
        kind,
        public_key,
        identifier: identifier.into(),
    }
    .verify()
    .map_err(|_| AdpError::MalformedTag("a".into()))
}

pub fn build_entitlement_event_builder(
    params: &BuildEntitlementParams,
) -> Result<EventBuilder, AdpError> {
    if params.grant_id.is_empty() {
        return Err(AdpError::MalformedTag("d".into()));
    }
    validate_coordinate(&params.coordinate)?;
    match (params.status, params.predecessor) {
        (GrantStatus::Granted, None) | (GrantStatus::Revoked, Some(_)) => {}
        (GrantStatus::Granted, Some(_)) => return Err(AdpError::ForbiddenTag("e".into())),
        (GrantStatus::Revoked, None) => return Err(AdpError::MissingTag("e")),
    }

    let mut tags = vec![
        entitlement_tag(vec!["d".into(), params.grant_id.clone()])?,
        entitlement_tag(vec!["p".into(), params.recipient.to_hex()])?,
        entitlement_tag(vec!["a".into(), params.coordinate.clone()])?,
        entitlement_tag(vec![
            "status".into(),
            match params.status {
                GrantStatus::Granted => "granted",
                GrantStatus::Revoked => "revoked",
            }
            .into(),
        ])?,
    ];
    if let Some(source_event) = params.source_event {
        tags.push(entitlement_tag(vec![
            "source_event".into(),
            source_event.to_hex(),
        ])?);
    }
    if let Some(authorization) = params.authorization {
        tags.push(entitlement_tag(vec![
            "authorization".into(),
            authorization.to_hex(),
        ])?);
    }
    if let Some(reason) = &params.reason {
        if reason.is_empty() {
            return Err(AdpError::MalformedTag("reason".into()));
        }
        tags.push(entitlement_tag(vec!["reason".into(), reason.clone()])?);
    }
    if let Some(predecessor) = params.predecessor {
        tags.push(entitlement_tag(vec!["e".into(), predecessor.to_hex()])?);
    }

    Ok(EventBuilder::new(Kind::Custom(EXPERIMENTAL_ENTITLEMENT_GRANT_KIND), "").tags(tags))
}

pub fn parse_entitlement_event(event: &Event) -> Result<ParsedEntitlementGrant, AdpError> {
    if event.kind.as_u16() != EXPERIMENTAL_ENTITLEMENT_GRANT_KIND {
        return Err(AdpError::WrongKind {
            expected: EXPERIMENTAL_ENTITLEMENT_GRANT_KIND,
            found: event.kind.as_u16(),
        });
    }
    if event.verify().is_err() {
        return Err(AdpError::InvalidSignature);
    }

    let grant_id = exact_tag(event, "d", true)?.ok_or(AdpError::MissingTag("d"))?;
    let recipient =
        PublicKey::from_hex(&exact_tag(event, "p", true)?.ok_or(AdpError::MissingTag("p"))?)
            .map_err(|_| AdpError::MalformedTag("p".into()))?;
    let coordinate = exact_tag(event, "a", true)?.ok_or(AdpError::MissingTag("a"))?;
    validate_coordinate(&coordinate)?;
    let source_event = exact_tag(event, "source_event", false)?
        .map(|value| {
            EventId::from_hex(&value).map_err(|_| AdpError::MalformedTag("source_event".into()))
        })
        .transpose()?;
    let authorization = exact_tag(event, "authorization", false)?
        .map(|value| {
            EventId::from_hex(&value).map_err(|_| AdpError::MalformedTag("authorization".into()))
        })
        .transpose()?;
    let reason = exact_tag(event, "reason", false)?;
    let status = match exact_tag(event, "status", true)?
        .ok_or(AdpError::MissingTag("status"))?
        .as_str()
    {
        "granted" => GrantStatus::Granted,
        "revoked" => GrantStatus::Revoked,
        _ => return Err(AdpError::MalformedTag("status".into())),
    };
    let predecessor = exact_tag(event, "e", false)?
        .map(|value| EventId::from_hex(&value).map_err(|_| AdpError::MalformedTag("e".into())))
        .transpose()?;
    match (status, predecessor) {
        (GrantStatus::Granted, None) | (GrantStatus::Revoked, Some(_)) => {}
        (GrantStatus::Granted, Some(_)) => return Err(AdpError::ForbiddenTag("e".into())),
        (GrantStatus::Revoked, None) => return Err(AdpError::MissingTag("e")),
    }

    Ok(ParsedEntitlementGrant {
        event: event.clone(),
        grant_id,
        recipient,
        coordinate,
        source_event,
        authorization,
        reason,
        status,
        predecessor,
    })
}

fn deduplicate_entitlements(nodes: &[ParsedEntitlementGrant]) -> Vec<ParsedEntitlementGrant> {
    let mut seen = HashSet::with_capacity(nodes.len());
    nodes
        .iter()
        .filter(|node| seen.insert(node.event.id))
        .cloned()
        .collect()
}

pub fn group_parsed_entitlements(
    nodes: &[ParsedEntitlementGrant],
) -> Vec<Vec<ParsedEntitlementGrant>> {
    let mut identity_groups: HashMap<(PublicKey, String, String), Vec<ParsedEntitlementGrant>> =
        HashMap::new();
    for parsed in deduplicate_entitlements(nodes) {
        identity_groups
            .entry((
                parsed.recipient,
                parsed.coordinate.clone(),
                parsed.grant_id.clone(),
            ))
            .or_default()
            .push(parsed);
    }

    let mut components = Vec::new();
    for group in identity_groups.into_values() {
        let by_id = group
            .iter()
            .map(|node| (node.event.id, node))
            .collect::<HashMap<_, _>>();
        let mut children: HashMap<EventId, Vec<EventId>> = HashMap::new();
        for node in &group {
            if let Some(predecessor) = node.predecessor.filter(|id| by_id.contains_key(id)) {
                children.entry(predecessor).or_default().push(node.event.id);
            }
        }
        let mut visited = HashSet::with_capacity(group.len());
        for node in &group {
            if visited.contains(&node.event.id) {
                continue;
            }
            let mut component = Vec::new();
            let mut pending = vec![node.event.id];
            while let Some(event_id) = pending.pop() {
                if !visited.insert(event_id) {
                    continue;
                }
                let current = by_id[&event_id];
                component.push(current.clone());
                if let Some(predecessor) = current.predecessor.filter(|id| by_id.contains_key(id)) {
                    pending.push(predecessor);
                }
                if let Some(successors) = children.get(&event_id) {
                    pending.extend(successors);
                }
            }
            components.push(component);
        }
    }
    components
}

pub fn resolve_entitlement_grant(
    nodes: &[ParsedEntitlementGrant],
) -> Result<ResolvedEntitlementGrant, EntitlementResolutionError> {
    let mut reparsed = Vec::with_capacity(nodes.len());
    for node in nodes {
        let parsed = parse_entitlement_event(&node.event)?;
        if parsed != *node {
            return Err(EntitlementResolutionError::Chain(
                ChainResolutionError::InvariantMutation("signed event metadata"),
            ));
        }
        reparsed.push(parsed);
    }
    let reparsed = deduplicate_entitlements(&reparsed);

    Ok(resolve_parsed_entitlement(&reparsed, |_| true)?)
}

pub fn resolve_adp_entitlement_grant(
    nodes: &[ParsedEntitlementGrant],
    developer: PublicKey,
) -> Result<ResolvedEntitlementGrant, EntitlementResolutionError> {
    let mut reparsed = Vec::with_capacity(nodes.len());
    for node in nodes {
        let parsed = parse_entitlement_event(&node.event)?;
        if parsed != *node {
            return Err(EntitlementResolutionError::Chain(
                ChainResolutionError::InvariantMutation("signed event metadata"),
            ));
        }
        reparsed.push(parsed);
    }
    Ok(resolve_parsed_entitlement(
        &deduplicate_entitlements(&reparsed),
        |event| event.predecessor.is_none() || event.event.pubkey == developer,
    )?)
}

fn resolve_parsed_entitlement<F>(
    nodes: &[ParsedEntitlementGrant],
    transition_authorized: F,
) -> Result<ResolvedEntitlementGrant, ChainResolutionError>
where
    F: Fn(&ParsedEntitlementGrant) -> bool,
{
    if nodes.is_empty() {
        return Err(ChainResolutionError::Empty);
    }
    let by_id = nodes
        .iter()
        .map(|node| (node.event.id, node))
        .collect::<HashMap<_, _>>();
    if by_id.len() != nodes.len() {
        return Err(ChainResolutionError::Disconnected);
    }
    let roots = nodes
        .iter()
        .filter(|node| node.predecessor.is_none() && node.status == GrantStatus::Granted)
        .collect::<Vec<_>>();
    if roots.is_empty() {
        return Err(ChainResolutionError::Cycle);
    }
    if roots.len() > 1 {
        return Err(ChainResolutionError::MultipleRoots);
    }
    let root = roots[0];
    let mut ordered = Vec::with_capacity(nodes.len());
    let mut visited = HashSet::with_capacity(nodes.len());
    let mut current = root.event.id;
    loop {
        if !visited.insert(current) {
            return Err(ChainResolutionError::Cycle);
        }
        ordered.push((*by_id[&current]).clone());
        let predecessor = by_id[&current];
        let mut valid = nodes
            .iter()
            .filter(|node| node.predecessor == Some(current))
            .filter(|node| {
                node.event.kind.as_u16() == EXPERIMENTAL_ENTITLEMENT_GRANT_KIND
                    && node.event.id != current
                    && node.grant_id == root.grant_id
                    && node.recipient == root.recipient
                    && node.coordinate == root.coordinate
                    && node.source_event == root.source_event
                    && node.authorization == root.authorization
                    && node.status == GrantStatus::Revoked
                    && node.event.created_at > predecessor.event.created_at
                    && predecessor.status != GrantStatus::Revoked
                    && transition_authorized(node)
            })
            .collect::<Vec<_>>();
        if valid.len() > 1 {
            return Err(ChainResolutionError::Fork(current));
        }
        let Some(next) = valid.pop() else { break };
        current = next.event.id;
    }
    if root.status != GrantStatus::Granted {
        return Err(ChainResolutionError::InvalidTransition(
            "entitlement root must be granted".into(),
        ));
    }
    Ok(ResolvedEntitlementGrant {
        root_event_id: root.event.id,
        events: ordered,
    })
}

#[cfg(test)]
mod tests {
    use nostr::{Keys, Timestamp};

    use super::*;
    use crate::ChainResolutionError;

    fn parsed_node(
        issuer: &Keys,
        recipient: PublicKey,
        coordinate: &str,
        status: GrantStatus,
        predecessor: Option<EventId>,
        created_at: u64,
    ) -> ParsedEntitlementGrant {
        let event = build_entitlement_event_builder(&BuildEntitlementParams {
            grant_id: "grant-1".into(),
            recipient,
            coordinate: coordinate.into(),
            source_event: None,
            authorization: None,
            reason: None,
            status,
            predecessor,
        })
        .unwrap()
        .custom_created_at(Timestamp::from_secs(created_at))
        .sign_with_keys(issuer)
        .unwrap();
        parse_entitlement_event(&event).unwrap()
    }

    #[test]
    fn pure_topology_rejects_cycle_after_signed_parsing() {
        let issuer = Keys::generate();
        let recipient = Keys::generate().public_key();
        let coordinate = format!("30402:{}:game", issuer.public_key());
        let mut root = parsed_node(
            &issuer,
            recipient,
            &coordinate,
            GrantStatus::Granted,
            None,
            100,
        );
        let update = parsed_node(
            &issuer,
            recipient,
            &coordinate,
            GrantStatus::Revoked,
            Some(root.event.id),
            101,
        );
        root.predecessor = Some(update.event.id);

        assert!(matches!(
            resolve_parsed_entitlement(&[root, update], |_| true),
            Err(ChainResolutionError::Cycle)
        ));
    }

    #[test]
    fn pure_topology_rejects_self_reference_after_signed_parsing() {
        let issuer = Keys::generate();
        let recipient = Keys::generate().public_key();
        let coordinate = format!("30402:{}:game", issuer.public_key());
        let mut root = parsed_node(
            &issuer,
            recipient,
            &coordinate,
            GrantStatus::Granted,
            None,
            100,
        );
        root.predecessor = Some(root.event.id);

        assert!(matches!(
            resolve_parsed_entitlement(&[root], |_| true),
            Err(ChainResolutionError::Cycle)
        ));
    }

    #[test]
    fn pure_topology_excludes_disconnected_nodes_after_signed_parsing() {
        let issuer = Keys::generate();
        let recipient = Keys::generate().public_key();
        let coordinate = format!("30402:{}:game", issuer.public_key());
        let root = parsed_node(
            &issuer,
            recipient,
            &coordinate,
            GrantStatus::Granted,
            None,
            100,
        );
        let update = parsed_node(
            &issuer,
            recipient,
            &coordinate,
            GrantStatus::Revoked,
            Some(root.event.id),
            101,
        );
        let mut detached_a = parsed_node(
            &issuer,
            recipient,
            &coordinate,
            GrantStatus::Revoked,
            Some(root.event.id),
            102,
        );
        let mut detached_b = parsed_node(
            &issuer,
            recipient,
            &coordinate,
            GrantStatus::Revoked,
            Some(root.event.id),
            103,
        );
        detached_a.predecessor = Some(detached_b.event.id);
        detached_b.predecessor = Some(detached_a.event.id);

        let resolved =
            resolve_parsed_entitlement(&[root, update, detached_a, detached_b], |_| true).unwrap();
        assert_eq!(resolved.events.len(), 2);
    }
}
