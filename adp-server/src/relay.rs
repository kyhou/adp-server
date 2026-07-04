use std::time::Duration;

use nostr_sdk::prelude::*;

const LISTING_KIND: u16 = 30402;
const RECEIPT_KIND: u16 = 1020;
const RELAY_FETCH_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub struct RelayClient {
    client: Client,
    server_url: String,
}

impl RelayClient {
    pub async fn connect(
        fulfillment_keys: Keys,
        relays: &[String],
        server_url: String,
    ) -> Result<Self, nostr_sdk::client::Error> {
        let client = Client::new(fulfillment_keys);
        for relay in relays {
            client.add_relay(relay.as_str()).await?;
        }
        client.connect().await;
        Ok(Self { client, server_url })
    }

    pub fn server_url(&self) -> &str {
        &self.server_url
    }

    pub async fn publish(&self, event: &Event) -> Result<(), nostr_sdk::client::Error> {
        self.client.send_event(event).await?;
        Ok(())
    }

    /// Fetches the current `kind:30402` listing for `(developer_pubkey, d_tag)` fresh
    /// from relays. MUST NOT be replaced by a local cache — see ADP-01.md's
    /// delegation freshness requirement: a cached listing may not reflect a recent
    /// `revoked_at` update.
    pub async fn fetch_listing(
        &self,
        developer_pubkey: PublicKey,
        d_tag: &str,
    ) -> Result<Option<Event>, nostr_sdk::client::Error> {
        let filter = Filter::new()
            .kind(Kind::Custom(LISTING_KIND))
            .author(developer_pubkey)
            .identifier(d_tag);
        let events = self
            .client
            .fetch_events(filter, RELAY_FETCH_TIMEOUT)
            .await?;
        Ok(events.into_iter().max_by_key(|e| e.created_at.as_secs()))
    }

    /// Fetches all `kind:1020` receipts addressed to `buyer_pubkey` referencing
    /// `game_coordinate` in an `a` tag — the "Path B" portable ownership lookup.
    pub async fn fetch_receipts(
        &self,
        buyer_pubkey: PublicKey,
        game_coordinate: &str,
    ) -> Result<Vec<Event>, nostr_sdk::client::Error> {
        let filter = Filter::new()
            .kind(Kind::Custom(RECEIPT_KIND))
            .pubkey(buyer_pubkey)
            .custom_tag(
                SingleLetterTag::lowercase(Alphabet::A),
                game_coordinate.to_string(),
            );
        let events = self
            .client
            .fetch_events(filter, RELAY_FETCH_TIMEOUT)
            .await?;
        Ok(events.into_iter().collect())
    }
}
