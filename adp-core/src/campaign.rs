use nostr::{Event, EventId, PublicKey};

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
