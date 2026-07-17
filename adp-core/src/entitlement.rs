use nostr::{Event, EventId, PublicKey};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantStatus {
    Granted,
    Revoked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedEntitlementGrant {
    pub event: Event,
    pub grant_id: String,
    pub recipient: PublicKey,
    pub coordinate: String,
    pub source_event: Option<EventId>,
    pub authorization_event: Option<EventId>,
    pub reason: Option<String>,
    pub status: GrantStatus,
    pub predecessor: Option<EventId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedEntitlementGrant {
    pub root_event_id: EventId,
    pub events: Vec<ParsedEntitlementGrant>,
}
