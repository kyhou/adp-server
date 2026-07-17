use std::time::Duration;

use async_trait::async_trait;
use nostr_sdk::prelude::*;

const LISTING_KIND: u16 = 30402;
const RECEIPT_KIND: u16 = 1020;
const RELAY_FETCH_TIMEOUT: Duration = Duration::from_secs(10);

fn select_latest_listing(events: impl IntoIterator<Item = Event>) -> Option<Event> {
    events.into_iter().max_by_key(|e| e.created_at.as_secs())
}

fn build_receipts_filter(buyer_pubkey: PublicKey, game_coordinate: &str) -> Filter {
    Filter::new()
        .kind(Kind::Custom(RECEIPT_KIND))
        .pubkey(buyer_pubkey)
        .custom_tag(
            SingleLetterTag::lowercase(Alphabet::A),
            game_coordinate.to_string(),
        )
}

#[derive(Clone)]
pub struct RelayClient {
    client: Client,
    server_url: String,
}

pub type RelayResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayLookup<T> {
    Found(T),
    ConfirmedAbsent {
        successful_relays: usize,
    },
    Unavailable {
        successful_relays: usize,
        failed_relays: usize,
    },
}

#[async_trait]
pub trait Relay: Send + Sync {
    fn server_url(&self) -> &str;
    async fn publish(&self, event: &Event) -> RelayResult<()>;
    async fn fetch_listing(
        &self,
        developer_pubkey: PublicKey,
        d_tag: &str,
    ) -> RelayResult<Option<Event>>;
    async fn fetch_receipts(
        &self,
        buyer_pubkey: PublicKey,
        game_coordinate: &str,
    ) -> RelayResult<Vec<Event>>;
}

impl RelayClient {
    pub async fn connect(
        operator_keys: Keys,
        relays: &[String],
        server_url: String,
    ) -> Result<Self, nostr_sdk::client::Error> {
        let client = Client::new(operator_keys);
        for relay in relays {
            client.add_relay(relay.as_str()).await?;
        }
        client.connect().await;
        Ok(Self { client, server_url })
    }
}

#[async_trait]
impl Relay for RelayClient {
    fn server_url(&self) -> &str {
        &self.server_url
    }

    async fn publish(&self, event: &Event) -> RelayResult<()> {
        self.client.send_event(event).await?;
        Ok(())
    }

    /// Fetches the current `kind:30402` listing for `(developer_pubkey, d_tag)` fresh
    /// from relays. MUST NOT be replaced by a local cache — see ADP-01.md's
    /// delegation freshness requirement: a cached listing may not reflect a recent
    /// `revoked_at` update.
    async fn fetch_listing(
        &self,
        developer_pubkey: PublicKey,
        d_tag: &str,
    ) -> RelayResult<Option<Event>> {
        let filter = Filter::new()
            .kind(Kind::Custom(LISTING_KIND))
            .author(developer_pubkey)
            .identifier(d_tag);
        let events = self
            .client
            .fetch_events(filter, RELAY_FETCH_TIMEOUT)
            .await?;
        Ok(select_latest_listing(events))
    }

    /// Fetches all `kind:1020` receipts addressed to `buyer_pubkey` referencing
    /// `game_coordinate` in an `a` tag — the "Path B" portable ownership lookup.
    async fn fetch_receipts(
        &self,
        buyer_pubkey: PublicKey,
        game_coordinate: &str,
    ) -> RelayResult<Vec<Event>> {
        let filter = build_receipts_filter(buyer_pubkey, game_coordinate);
        let events = self
            .client
            .fetch_events(filter, RELAY_FETCH_TIMEOUT)
            .await?;
        Ok(events.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing_at(keys: &Keys, d_tag: &str, created_at: u64) -> Event {
        EventBuilder::new(Kind::Custom(LISTING_KIND), "")
            .tags(vec![Tag::parse(vec!["d", d_tag]).expect("valid d tag")])
            .custom_created_at(Timestamp::from(created_at))
            .sign_with_keys(keys)
            .expect("sign listing")
    }

    #[test]
    fn select_latest_listing_uses_highest_created_at_regardless_order() {
        let developer = Keys::generate();
        let older = listing_at(&developer, "game-1", 1_700_000_000);
        let newer = listing_at(&developer, "game-1", 1_800_000_000);

        let selected = select_latest_listing(vec![newer.clone(), older]).expect("selected event");

        assert_eq!(selected.id, newer.id);
    }

    #[test]
    fn receipt_filter_targets_buyer_p_tag_not_author() {
        let buyer = Keys::generate().public_key();
        let coordinate = "30402:developer:game-1";

        let filter = build_receipts_filter(buyer, coordinate);
        let serialized = serde_json::to_value(&filter).expect("serialize filter");

        assert_eq!(serialized.get("authors"), None);
        assert_eq!(serialized["#p"][0], buyer.to_hex());
        assert_eq!(serialized["#a"][0], coordinate);
    }
}
