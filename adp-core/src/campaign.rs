use std::collections::{HashMap, HashSet};

use nostr::{Event, EventId, PublicKey, Tag};

use crate::chain::ChainResolutionError;
use crate::error::AdpError;
use crate::kinds::EXPERIMENTAL_ADP_CAMPAIGN_KIND;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CampaignMode {
    Claim,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CampaignTerms {
    pub mode: CampaignMode,
    pub starts: u64,
    pub ends: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CampaignTransition {
    Root(CampaignTerms),
    ReplaceTerms(CampaignTerms),
    ReplaceTermsAndCancel(CampaignTerms),
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CampaignStatus {
    Active,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CampaignEvent {
    pub event: Event,
    pub campaign_id: String,
    pub coordinate: String,
    pub predecessor: Option<EventId>,
    pub transition: CampaignTransition,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedCampaign {
    pub root_event_id: EventId,
    pub developer_pubkey: PublicKey,
    pub campaign_id: String,
    pub coordinate: String,
    pub events: Vec<CampaignEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CampaignState {
    pub status: CampaignStatus,
    pub terms: CampaignTerms,
}

fn exact_tag(
    event: &Event,
    name: &'static str,
    required: bool,
) -> Result<Option<String>, AdpError> {
    let matching: Vec<Vec<String>> = event
        .tags
        .iter()
        .map(|tag| tag.clone().to_vec())
        .filter(|values| values.first().is_some_and(|value| value == name))
        .collect();

    if matching.len() > 1 {
        return Err(AdpError::DuplicateTag(name.into()));
    }
    if matching.first().is_some_and(|values| values.len() != 2) {
        return Err(AdpError::MalformedTag(name.into()));
    }
    match matching.first().and_then(|values| values.get(1)).cloned() {
        Some(value) => Ok(Some(value)),
        None if required => Err(AdpError::MissingTag(name)),
        None => Ok(None),
    }
}

fn coordinate_developer(coordinate: &str) -> Option<PublicKey> {
    let mut parts = coordinate.split(':');
    let kind = parts.next()?;
    let developer = parts.next()?;
    let game_id = parts.next()?;
    if kind != "30402" || game_id.is_empty() || parts.next().is_some() {
        return None;
    }
    PublicKey::from_hex(developer).ok()
}

fn parse_terms(event: &Event) -> Result<CampaignTerms, AdpError> {
    let mode = exact_tag(event, "mode", true)?;
    if mode.as_deref() != Some("claim") {
        return Err(AdpError::MalformedTag("mode".into()));
    }
    let starts = exact_tag(event, "starts", true)?
        .ok_or(AdpError::MissingTag("starts"))?
        .parse::<u64>()
        .map_err(|_| AdpError::MalformedTag("starts".into()))?;
    let ends = exact_tag(event, "ends", true)?
        .ok_or(AdpError::MissingTag("ends"))?
        .parse::<u64>()
        .map_err(|_| AdpError::MalformedTag("ends".into()))?;
    Ok(CampaignTerms {
        mode: CampaignMode::Claim,
        starts,
        ends,
    })
}

pub fn parse_campaign_event(event: &Event) -> Result<CampaignEvent, AdpError> {
    if event.kind.as_u16() != EXPERIMENTAL_ADP_CAMPAIGN_KIND {
        return Err(AdpError::MissingTag("kind:1031"));
    }
    if event.verify().is_err() {
        return Err(AdpError::InvalidSignature);
    }

    let campaign_id = exact_tag(event, "d", true)?.ok_or(AdpError::MissingTag("d"))?;
    let coordinate = exact_tag(event, "a", true)?.ok_or(AdpError::MissingTag("a"))?;
    let developer =
        coordinate_developer(&coordinate).ok_or_else(|| AdpError::CoordinateMismatch {
            expected: format!("30402:{}:<d>", event.pubkey),
            found: coordinate.clone(),
        })?;
    if developer != event.pubkey {
        return Err(AdpError::CoordinateMismatch {
            expected: format!("30402:{}:<d>", event.pubkey),
            found: coordinate,
        });
    }

    let status = exact_tag(event, "status", true)?.ok_or(AdpError::MissingTag("status"))?;
    let predecessor = exact_tag(event, "e", false)?
        .map(|value| EventId::from_hex(&value).map_err(|_| AdpError::MalformedTag("e".into())))
        .transpose()?;

    let transition = match (status.as_str(), predecessor) {
        ("active", None) => CampaignTransition::Root(parse_terms(event)?),
        ("active", Some(_)) => CampaignTransition::ReplaceTerms(parse_terms(event)?),
        ("cancelled", Some(_)) => {
            let has_terms = ["mode", "starts", "ends"]
                .iter()
                .any(|name| exact_tag(event, name, false).ok().flatten().is_some());
            if has_terms {
                return Ok(CampaignEvent {
                    event: event.clone(),
                    campaign_id,
                    coordinate,
                    predecessor,
                    transition: CampaignTransition::ReplaceTermsAndCancel(parse_terms(event)?),
                });
            }
            let allowed = ["d", "a", "status", "e"];
            if event.tags.iter().any(|tag: &Tag| {
                tag.clone()
                    .to_vec()
                    .first()
                    .is_none_or(|name| !allowed.contains(&name.as_str()))
            }) {
                let name = event
                    .tags
                    .iter()
                    .map(|tag| tag.clone().to_vec())
                    .find_map(|values| {
                        values
                            .first()
                            .filter(|name| !allowed.contains(&name.as_str()))
                            .cloned()
                    })
                    .unwrap_or_else(|| "tag".into());
                return Err(AdpError::ForbiddenTag(name));
            }
            CampaignTransition::Cancel
        }
        ("cancelled", None) => return Err(AdpError::MissingTag("e")),
        _ => return Err(AdpError::MalformedTag("status".into())),
    };

    Ok(CampaignEvent {
        event: event.clone(),
        campaign_id,
        coordinate,
        predecessor,
        transition,
    })
}

fn validate_terms(terms: &CampaignTerms) -> Result<(), ChainResolutionError> {
    if terms.starts >= terms.ends {
        return Err(ChainResolutionError::InvalidTransition(
            "campaign starts must precede ends".into(),
        ));
    }
    Ok(())
}

pub fn resolve_campaign(
    nodes: &[CampaignEvent],
    expected_developer: PublicKey,
    expected_coordinate: &str,
) -> Result<ResolvedCampaign, ChainResolutionError> {
    resolve_campaign_at(nodes, expected_developer, expected_coordinate, u64::MAX)
}

pub fn resolve_campaign_at(
    nodes: &[CampaignEvent],
    expected_developer: PublicKey,
    expected_coordinate: &str,
    at: u64,
) -> Result<ResolvedCampaign, ChainResolutionError> {
    let reparsed = nodes
        .iter()
        .filter(|node| node.event.created_at.as_secs() <= at)
        .map(|node| {
            let parsed = parse_campaign_event(&node.event).map_err(|error| {
                ChainResolutionError::InvalidTransition(format!(
                    "invalid signed campaign event: {error}"
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

    resolve_parsed_campaign(&reparsed, expected_developer, expected_coordinate)
}

fn resolve_parsed_campaign(
    nodes: &[CampaignEvent],
    expected_developer: PublicKey,
    expected_coordinate: &str,
) -> Result<ResolvedCampaign, ChainResolutionError> {
    if nodes.is_empty() {
        return Err(ChainResolutionError::Empty);
    }

    let by_id: HashMap<EventId, &CampaignEvent> =
        nodes.iter().map(|node| (node.event.id, node)).collect();
    if by_id.len() != nodes.len() {
        return Err(ChainResolutionError::Disconnected);
    }
    let roots: Vec<&CampaignEvent> = nodes
        .iter()
        .filter(|node| {
            node.predecessor.is_none()
                && node.event.pubkey == expected_developer
                && node.coordinate == expected_coordinate
                && matches!(node.transition, CampaignTransition::Root(_))
        })
        .collect();
    if roots.is_empty() {
        return Err(ChainResolutionError::Cycle);
    }
    if roots.len() > 1 {
        return Err(ChainResolutionError::MultipleRoots);
    }
    let root = roots[0];
    let campaign_id = root.campaign_id.clone();
    let mut ordered = Vec::with_capacity(nodes.len());
    let mut visited = HashSet::with_capacity(nodes.len());
    let mut current = root.event.id;
    let mut terms = match &root.transition {
        CampaignTransition::Root(terms) => {
            validate_terms(terms)?;
            if terms.starts < root.event.created_at.as_secs() {
                return Err(ChainResolutionError::InvalidTransition(
                    "campaign starts before root publication".into(),
                ));
            }
            terms.clone()
        }
        _ => unreachable!(),
    };
    let mut cancelled = false;
    loop {
        if !visited.insert(current) {
            return Err(ChainResolutionError::Cycle);
        }
        ordered.push((*by_id[&current]).clone());
        let predecessor = by_id[&current];
        let mut valid = Vec::new();
        for candidate in nodes
            .iter()
            .filter(|node| node.predecessor == Some(current))
        {
            if candidate.event.pubkey != expected_developer
                || candidate.coordinate != expected_coordinate
                || candidate.campaign_id != campaign_id
                || candidate.event.created_at <= predecessor.event.created_at
                || candidate.predecessor == Some(candidate.event.id)
                || cancelled
            {
                continue;
            }
            let admitted = match &candidate.transition {
                CampaignTransition::ReplaceTerms(replacement) => {
                    validate_terms(replacement).is_ok()
                        && candidate.event.created_at.as_secs() < terms.starts
                }
                CampaignTransition::ReplaceTermsAndCancel(replacement) => {
                    validate_terms(replacement).is_ok()
                        && candidate.event.created_at.as_secs() < terms.starts
                }
                CampaignTransition::Cancel => true,
                CampaignTransition::Root(_) => false,
            };
            if admitted {
                valid.push(candidate);
            }
        }
        if valid.len() > 1 {
            return Err(ChainResolutionError::Fork(current));
        }
        let Some(next) = valid.pop() else { break };
        match &next.transition {
            CampaignTransition::ReplaceTerms(replacement) => terms = replacement.clone(),
            CampaignTransition::ReplaceTermsAndCancel(replacement) => {
                terms = replacement.clone();
                cancelled = true;
            }
            CampaignTransition::Cancel => cancelled = true,
            CampaignTransition::Root(_) => unreachable!(),
        }
        current = next.event.id;
    }

    Ok(ResolvedCampaign {
        root_event_id: root.event.id,
        developer_pubkey: expected_developer,
        campaign_id,
        coordinate: expected_coordinate.to_owned(),
        events: ordered,
    })
}

impl ResolvedCampaign {
    pub fn state_at(&self, at: u64) -> Option<CampaignState> {
        let resolved =
            resolve_campaign_at(&self.events, self.developer_pubkey, &self.coordinate, at).ok()?;
        if resolved.root_event_id != self.root_event_id
            || resolved.campaign_id != self.campaign_id
            || resolved.events != self.events
        {
            return None;
        }

        resolved.state_at_trusted(at)
    }

    pub(crate) fn state_at_trusted(&self, at: u64) -> Option<CampaignState> {
        let root = self.events.first()?;
        if at < root.event.created_at.as_secs() {
            return None;
        }
        let terms = match &root.transition {
            CampaignTransition::Root(terms) => terms.clone(),
            _ => return None,
        };
        let mut state = CampaignState {
            status: CampaignStatus::Active,
            terms,
        };

        for event in self.events.iter().skip(1) {
            if event.event.created_at.as_secs() > at {
                break;
            }
            match &event.transition {
                CampaignTransition::ReplaceTerms(terms) => state.terms = terms.clone(),
                CampaignTransition::ReplaceTermsAndCancel(terms) => {
                    state.terms = terms.clone();
                    state.status = CampaignStatus::Cancelled;
                }
                CampaignTransition::Cancel => state.status = CampaignStatus::Cancelled,
                CampaignTransition::Root(_) => return None,
            }
        }
        Some(state)
    }

    pub fn is_claimable_at(&self, at: u64) -> bool {
        self.state_at(at).is_some_and(|state| {
            state.status == CampaignStatus::Active
                && state.terms.starts <= at
                && at < state.terms.ends
        })
    }

    pub(crate) fn is_claimable_at_trusted(&self, at: u64) -> bool {
        self.state_at_trusted(at).is_some_and(|state| {
            state.status == CampaignStatus::Active
                && state.terms.starts <= at
                && at < state.terms.ends
        })
    }
}

#[cfg(test)]
mod tests {
    use nostr::{EventBuilder, Keys, Kind, Tag, Timestamp};

    use super::*;

    fn parsed_node(
        developer: &Keys,
        coordinate: &str,
        predecessor: Option<EventId>,
        created_at: u64,
    ) -> CampaignEvent {
        let mut tags = vec![
            Tag::parse(["d", "campaign"]).unwrap(),
            Tag::parse(["a", coordinate]).unwrap(),
            Tag::parse(["status", "active"]).unwrap(),
            Tag::parse(["mode", "claim"]).unwrap(),
            Tag::parse(["starts", "200"]).unwrap(),
            Tag::parse(["ends", "300"]).unwrap(),
        ];
        if let Some(predecessor) = predecessor {
            tags.push(Tag::parse(["e", &predecessor.to_hex()]).unwrap());
        }
        let event = EventBuilder::new(Kind::Custom(EXPERIMENTAL_ADP_CAMPAIGN_KIND), "")
            .tags(tags)
            .custom_created_at(Timestamp::from_secs(created_at))
            .sign_with_keys(developer)
            .unwrap();
        parse_campaign_event(&event).unwrap()
    }

    #[test]
    fn trusted_topology_rejects_cycle() {
        let developer = Keys::generate();
        let coordinate = format!("30402:{}:game", developer.public_key());
        let mut root = parsed_node(&developer, &coordinate, None, 100);
        let update = parsed_node(&developer, &coordinate, Some(root.event.id), 110);
        root.predecessor = Some(update.event.id);

        assert!(matches!(
            resolve_parsed_campaign(&[root, update], developer.public_key(), &coordinate),
            Err(ChainResolutionError::Cycle)
        ));
    }

    #[test]
    fn trusted_topology_rejects_self_reference() {
        let developer = Keys::generate();
        let coordinate = format!("30402:{}:game", developer.public_key());
        let mut root = parsed_node(&developer, &coordinate, None, 100);
        root.predecessor = Some(root.event.id);

        assert!(matches!(
            resolve_parsed_campaign(&[root], developer.public_key(), &coordinate),
            Err(ChainResolutionError::Cycle)
        ));
    }

    #[test]
    fn trusted_topology_excludes_disconnected_component() {
        let developer = Keys::generate();
        let coordinate = format!("30402:{}:game", developer.public_key());
        let root = parsed_node(&developer, &coordinate, None, 100);
        let mut first = parsed_node(&developer, &coordinate, Some(root.event.id), 105);
        let mut second = parsed_node(&developer, &coordinate, Some(first.event.id), 110);
        first.predecessor = Some(second.event.id);
        second.predecessor = Some(first.event.id);

        let resolved =
            resolve_parsed_campaign(&[root, first, second], developer.public_key(), &coordinate)
                .unwrap();
        assert_eq!(resolved.events.len(), 1);
    }
}
