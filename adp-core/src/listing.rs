use nostr::{Event, EventId, PublicKey, RelayUrl};

use crate::error::AdpError;

pub const LISTING_KIND: u16 = 30402;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcquisitionPolicy {
    Gated,
    Public,
    TimedAccess { starts: u64, ends: u64 },
    Invalid,
}

impl AcquisitionPolicy {
    pub fn allows_at(&self, now: u64) -> bool {
        match self {
            Self::Public => true,
            Self::TimedAccess { starts, ends } => *starts <= now && now < *ends,
            Self::Gated | Self::Invalid => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CampaignPointer {
    pub root_event_id: EventId,
    pub relay_hint: Option<RelayUrl>,
}

/// A single `fulfillment_pubkey` tag on a listing:
/// `["fulfillment_pubkey", <pubkey-hex>, <valid_from-unix>, <revoked_at-unix-or-empty>]`
///
/// See ADP-01.md "Fulfillment Key Delegation".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FulfillmentDelegation {
    pub pubkey: PublicKey,
    pub valid_from: u64,
    pub revoked_at: Option<u64>,
}

impl FulfillmentDelegation {
    /// Whether this delegation authorized `pubkey` to sign at unix time `at`.
    /// Interval is `[valid_from, revoked_at)` — closed at the start, open at the end.
    pub fn authorizes_at(&self, at: u64, listing_created_at: u64) -> bool {
        if self.valid_from > at {
            return false;
        }
        match self.revoked_at {
            Some(revoked_at) => revoked_at.max(listing_created_at) > at,
            None => true,
        }
    }
}

/// Parsed view of a `kind:30402` NIP-99 listing, extended with ADP-01's
/// `server`, `file_hash`, `version`, `platform`, and `fulfillment_pubkey` tags.
#[derive(Debug, Clone)]
pub struct AdpListing {
    pub event: Event,
    pub developer_pubkey: PublicKey,
    pub d_tag: String,
    pub servers: Vec<String>,
    pub file_hash: Option<String>,
    pub version: Option<String>,
    pub price: Option<(String, String)>,
    pub lud16: Option<String>,
    pub platforms: Vec<String>,
    pub fulfillment_delegations: Vec<FulfillmentDelegation>,
    pub acquisition_policy: AcquisitionPolicy,
    pub campaign_pointers: Vec<CampaignPointer>,
}

impl AdpListing {
    /// Parses tags out of a signed `kind:30402` event. Does NOT verify the event's
    /// signature — callers MUST call `event.verify()` (or already have verified it,
    /// e.g. because it was fetched directly from a relay) before trusting this data
    /// for any authorization decision.
    pub fn from_event(event: Event) -> Result<Self, AdpError> {
        if event.kind.as_u16() != LISTING_KIND {
            return Err(AdpError::MissingTag("kind:30402"));
        }

        let developer_pubkey = event.pubkey;
        let mut d_tag: Option<String> = None;
        let mut d_tag_count = 0;
        let mut servers = Vec::new();
        let mut file_hash = None;
        let mut version = None;
        let mut price = None;
        let mut lud16 = None;
        let mut platforms = Vec::new();
        let mut fulfillment_delegations = Vec::new();
        let mut acquisition_policy = AcquisitionPolicy::Gated;
        let mut acquisition_seen = false;
        let mut campaign_pointers = Vec::new();

        for t in event.tags.iter() {
            let v = t.clone().to_vec();
            match v.first().map(|s| s.as_str()) {
                Some("d") => {
                    d_tag_count += 1;
                    d_tag = match v.as_slice() {
                        [tag, value] if tag == "d" => Some(value.clone()),
                        _ => None,
                    };
                }
                Some("server") => {
                    if let Some(url) = v.get(1) {
                        servers.push(url.clone());
                    }
                }
                Some("file_hash") => file_hash = v.get(1).cloned(),
                Some("version") => version = v.get(1).cloned(),
                Some("price") => {
                    if let (Some(amount), Some(currency)) = (v.get(1), v.get(2)) {
                        price = Some((amount.clone(), currency.clone()));
                    }
                }
                Some("lud16") => lud16 = v.get(1).cloned(),
                Some("platform") => {
                    if let Some(p) = v.get(1) {
                        platforms.push(p.clone());
                    }
                }
                Some("acquisition") => {
                    if acquisition_seen {
                        acquisition_policy = AcquisitionPolicy::Invalid;
                        continue;
                    }
                    acquisition_seen = true;
                    acquisition_policy = match v.as_slice() {
                        [tag, policy] if tag == "acquisition" && policy == "public" => {
                            AcquisitionPolicy::Public
                        }
                        [tag, policy, starts, ends]
                            if tag == "acquisition" && policy == "timed-access" =>
                        {
                            match (starts.parse::<u64>(), ends.parse::<u64>()) {
                                (Ok(starts), Ok(ends)) if starts < ends => {
                                    AcquisitionPolicy::TimedAccess { starts, ends }
                                }
                                _ => AcquisitionPolicy::Invalid,
                            }
                        }
                        _ => AcquisitionPolicy::Invalid,
                    };
                }
                Some("campaign") if matches!(v.len(), 2 | 3) => {
                    if let Some(root_event_id) = v.get(1).and_then(|id| EventId::from_hex(id).ok())
                    {
                        let relay_hint = v.get(2).and_then(|hint| {
                            hint.starts_with("wss://")
                                .then(|| hint.parse::<RelayUrl>().ok())
                                .flatten()
                        });
                        campaign_pointers.push(CampaignPointer {
                            root_event_id,
                            relay_hint,
                        });
                    }
                }
                Some("fulfillment_pubkey") => {
                    let pk_hex = v
                        .get(1)
                        .ok_or(AdpError::MissingTag("fulfillment_pubkey.pubkey"))?;
                    let valid_from_str = v
                        .get(2)
                        .ok_or(AdpError::MissingTag("fulfillment_pubkey.valid_from"))?;
                    let pubkey = PublicKey::from_hex(pk_hex)
                        .map_err(|_| AdpError::MissingTag("fulfillment_pubkey.pubkey"))?;
                    let valid_from = valid_from_str
                        .parse::<u64>()
                        .map_err(|_| AdpError::MissingTag("fulfillment_pubkey.valid_from"))?;
                    let revoked_at = v
                        .get(3)
                        .filter(|s| !s.is_empty())
                        .map(|s| {
                            s.parse::<u64>()
                                .map_err(|_| AdpError::MissingTag("fulfillment_pubkey.revoked_at"))
                        })
                        .transpose()?;
                    fulfillment_delegations.push(FulfillmentDelegation {
                        pubkey,
                        valid_from,
                        revoked_at,
                    });
                }
                _ => {}
            }
        }

        if d_tag_count != 1 {
            return Err(AdpError::MissingTag("d"));
        }
        let d_tag = d_tag.ok_or(AdpError::MissingTag("d"))?;

        Ok(Self {
            event,
            developer_pubkey,
            d_tag,
            servers,
            file_hash,
            version,
            price,
            lud16,
            platforms,
            fulfillment_delegations,
            acquisition_policy,
            campaign_pointers,
        })
    }

    /// The stable `30402:<developer-pubkey>:<d-tag>` game coordinate.
    pub fn coordinate(&self) -> String {
        format!(
            "{}:{}:{}",
            LISTING_KIND,
            self.developer_pubkey.to_hex(),
            self.d_tag
        )
    }

    /// Whether `server_url` is named as an authorized distributor in this listing's
    /// `server` tags. This is the *distribution authorization* check — independent
    /// of ownership proof. See ADP-01.md "Trust Model".
    pub fn distributes_via(&self, server_url: &str) -> bool {
        let normalize = |s: &str| s.trim_end_matches('/').to_string();
        let target = normalize(server_url);
        self.servers.iter().any(|s| normalize(s) == target)
    }
}
