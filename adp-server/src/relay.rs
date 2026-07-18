use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::Duration;

use adp_core::{
    listing::LISTING_KIND, provisioning::ACCEPTANCE_KIND, receipt::RECEIPT_KIND,
    ChainResolutionError, EXPERIMENTAL_ADP_CAMPAIGN_KIND, EXPERIMENTAL_ENTITLEMENT_GRANT_KIND,
};
use async_trait::async_trait;
use nostr_sdk::prelude::*;

const RELAY_FETCH_TIMEOUT: Duration = Duration::from_secs(10);
pub const MIN_SUCCESSFUL_RELAYS: usize = 2;
pub const MAX_CHAIN_FETCH_DEPTH: usize = 32;
pub const MAX_CHAIN_EVENTS: usize = 64;
pub const MAX_RELAY_QUERY_EVENTS: usize = 128;
pub const MAX_RELAY_QUERY_BYTES: usize = 256 * 1024;

#[derive(Debug)]
enum RelayQueryFailureReason {
    Timeout,
    Closed,
    DisconnectedOrShutdown,
    ReceiverLagged,
    ReceiverClosed,
    Subscribe,
    Unsubscribe,
    EventCountOverflow,
    SerializedBytesOverflow,
    TaskJoin,
}

#[derive(Debug)]
struct RelayQueryFailure {
    relay_url: RelayUrl,
    reason: RelayQueryFailureReason,
    detail: Option<String>,
}

impl RelayQueryFailure {
    fn new(relay_url: &RelayUrl, reason: RelayQueryFailureReason) -> Self {
        Self {
            relay_url: relay_url.clone(),
            reason,
            detail: None,
        }
    }

    fn with_detail(
        relay_url: &RelayUrl,
        reason: RelayQueryFailureReason,
        detail: impl ToString,
    ) -> Self {
        Self {
            relay_url: relay_url.clone(),
            reason,
            detail: Some(detail.to_string()),
        }
    }
}

struct SubscriptionCleanup {
    trigger: Option<tokio::sync::oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<Result<(), RelayQueryFailure>>>,
}

impl SubscriptionCleanup {
    fn new(relay: nostr_sdk::Relay, relay_url: RelayUrl, subscription_id: SubscriptionId) -> Self {
        let (trigger, triggered) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let _ = triggered.await;
            relay.unsubscribe(&subscription_id).await.map_err(|error| {
                RelayQueryFailure::with_detail(
                    &relay_url,
                    RelayQueryFailureReason::Unsubscribe,
                    error,
                )
            })
        });
        Self {
            trigger: Some(trigger),
            task: Some(task),
        }
    }

    async fn finish(mut self, relay_url: &RelayUrl) -> Option<RelayQueryFailure> {
        drop(self.trigger.take());
        match self.task.take().expect("cleanup task is present").await {
            Ok(Ok(())) => None,
            Ok(Err(failure)) => Some(failure),
            Err(error) => Some(RelayQueryFailure::with_detail(
                relay_url,
                RelayQueryFailureReason::TaskJoin,
                error,
            )),
        }
    }
}

impl Drop for SubscriptionCleanup {
    fn drop(&mut self) {
        drop(self.trigger.take());
        drop(self.task.take());
    }
}

fn select_latest_listing(
    events: impl IntoIterator<Item = Event>,
    developer_pubkey: PublicKey,
    d_tag: &str,
) -> Option<Event> {
    events
        .into_iter()
        .filter(|event| {
            let mut identifiers = event
                .tags
                .iter()
                .filter(|tag| tag.as_slice().first().is_some_and(|name| name == "d"));
            let identifier_matches = identifiers
                .next()
                .filter(|tag| tag.as_slice().len() == 2)
                .and_then(|tag| tag.as_slice().get(1))
                .is_some_and(|identifier| identifier == d_tag)
                && identifiers.next().is_none();

            event.kind == Kind::Custom(LISTING_KIND)
                && event.pubkey == developer_pubkey
                && identifier_matches
                && event.verify().is_ok()
        })
        .map(|event| (event.id, event))
        .collect::<BTreeMap<_, _>>()
        .into_values()
        // NIP-01 retains the lowest lexical event ID when timestamps tie.
        .max_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| right.id.cmp(&left.id))
        })
}

fn has_tag(event: &Event, name: &str, value: &str) -> bool {
    event.tags.iter().any(|tag| {
        let values = tag.as_slice();
        values.len() == 2 && values[0] == name && values[1] == value
    })
}

fn merge_events(events: impl IntoIterator<Item = Event>) -> Vec<Event> {
    events
        .into_iter()
        .map(|event| (event.id, event))
        .collect::<BTreeMap<_, _>>()
        .into_values()
        .collect()
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

fn build_campaigns_filter(game_coordinate: &str) -> Filter {
    Filter::new()
        .kind(Kind::Custom(EXPERIMENTAL_ADP_CAMPAIGN_KIND))
        .custom_tag(
            SingleLetterTag::lowercase(Alphabet::A),
            game_coordinate.to_string(),
        )
}

fn build_campaign_chain_filter(developer_pubkey: PublicKey, campaign_d: &str) -> Filter {
    Filter::new()
        .kind(Kind::Custom(EXPERIMENTAL_ADP_CAMPAIGN_KIND))
        .author(developer_pubkey)
        .identifier(campaign_d)
}

fn build_entitlements_filter(buyer_pubkey: PublicKey, game_coordinate: &str) -> Filter {
    Filter::new()
        .kind(Kind::Custom(EXPERIMENTAL_ENTITLEMENT_GRANT_KIND))
        .pubkey(buyer_pubkey)
        .custom_tag(
            SingleLetterTag::lowercase(Alphabet::A),
            game_coordinate.to_string(),
        )
}

fn build_grant_events_by_d_filter(grant_d: &str) -> Filter {
    Filter::new()
        .kind(Kind::Custom(EXPERIMENTAL_ENTITLEMENT_GRANT_KIND))
        .identifier(grant_d)
}

fn build_authorizations_filter(developer_pubkey: PublicKey, game_coordinate: &str) -> Filter {
    Filter::new()
        .kind(Kind::Custom(ACCEPTANCE_KIND))
        .author(developer_pubkey)
        .custom_tag(
            SingleLetterTag::lowercase(Alphabet::A),
            game_coordinate.to_string(),
        )
}

fn build_authorization_successors_filter(
    developer_pubkey: PublicKey,
    predecessor: EventId,
) -> Filter {
    Filter::new()
        .kind(Kind::Custom(ACCEPTANCE_KIND))
        .author(developer_pubkey)
        .event(predecessor)
}

#[derive(Clone)]
pub struct RelayClient {
    client: Client,
    relay_urls: Vec<RelayUrl>,
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

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RelayCompletionError {
    #[error("relay lookup unavailable ({successful_relays} successful, {failed_relays} failed)")]
    Unavailable {
        successful_relays: usize,
        failed_relays: usize,
    },
    #[error(transparent)]
    Malformed(#[from] ChainResolutionError),
}

#[async_trait]
pub trait Relay: Send + Sync {
    fn server_url(&self) -> &str;
    async fn publish(&self, event: &Event) -> RelayResult<()>;

    /// Transitional adapter with a legacy unchecked return shape.
    /// It refuses unavailable lookups but erases completeness metadata.
    #[doc(hidden)]
    async fn fetch_listing(
        &self,
        developer_pubkey: PublicKey,
        d_tag: &str,
    ) -> RelayResult<Option<Event>> {
        match self.lookup_listing(developer_pubkey, d_tag).await {
            RelayLookup::Found(event) => Ok(Some(event)),
            RelayLookup::ConfirmedAbsent { .. } => Ok(None),
            RelayLookup::Unavailable {
                successful_relays,
                failed_relays,
            } => Err(std::io::Error::other(format!(
                "relay lookup unavailable ({successful_relays} successful, {failed_relays} failed)"
            ))
            .into()),
        }
    }

    /// Transitional adapter with a legacy unchecked return shape.
    /// It refuses unavailable lookups but erases completeness metadata.
    #[doc(hidden)]
    async fn fetch_receipts(
        &self,
        buyer_pubkey: PublicKey,
        game_coordinate: &str,
    ) -> RelayResult<Vec<Event>> {
        match self.lookup_receipts(buyer_pubkey, game_coordinate).await {
            RelayLookup::Found(events) => Ok(events),
            RelayLookup::ConfirmedAbsent { .. } => Ok(Vec::new()),
            RelayLookup::Unavailable {
                successful_relays,
                failed_relays,
            } => Err(std::io::Error::other(format!(
                "relay lookup unavailable ({successful_relays} successful, {failed_relays} failed)"
            ))
            .into()),
        }
    }

    async fn lookup_event(&self, _event_id: EventId) -> RelayLookup<Event> {
        RelayLookup::Unavailable {
            successful_relays: 0,
            failed_relays: 0,
        }
    }

    async fn lookup_listing(
        &self,
        _developer_pubkey: PublicKey,
        _d_tag: &str,
    ) -> RelayLookup<Event> {
        RelayLookup::Unavailable {
            successful_relays: 0,
            failed_relays: 0,
        }
    }

    async fn lookup_campaigns(&self, _game_coordinate: &str) -> RelayLookup<Vec<Event>> {
        RelayLookup::Unavailable {
            successful_relays: 0,
            failed_relays: 0,
        }
    }

    async fn lookup_campaign_chain(
        &self,
        _developer_pubkey: PublicKey,
        _campaign_d: &str,
    ) -> RelayLookup<Vec<Event>> {
        RelayLookup::Unavailable {
            successful_relays: 0,
            failed_relays: 0,
        }
    }

    async fn lookup_entitlements(
        &self,
        _buyer_pubkey: PublicKey,
        _game_coordinate: &str,
    ) -> RelayLookup<Vec<Event>> {
        RelayLookup::Unavailable {
            successful_relays: 0,
            failed_relays: 0,
        }
    }

    async fn lookup_grant_events_by_d(&self, _grant_d: &str) -> RelayLookup<Vec<Event>> {
        RelayLookup::Unavailable {
            successful_relays: 0,
            failed_relays: 0,
        }
    }

    async fn lookup_receipts(
        &self,
        _buyer_pubkey: PublicKey,
        _game_coordinate: &str,
    ) -> RelayLookup<Vec<Event>> {
        RelayLookup::Unavailable {
            successful_relays: 0,
            failed_relays: 0,
        }
    }

    async fn lookup_authorizations(
        &self,
        _developer_pubkey: PublicKey,
        _game_coordinate: &str,
    ) -> RelayLookup<Vec<Event>> {
        RelayLookup::Unavailable {
            successful_relays: 0,
            failed_relays: 0,
        }
    }

    async fn lookup_authorization_successors(
        &self,
        _developer_pubkey: PublicKey,
        _event_id: EventId,
    ) -> RelayLookup<Vec<Event>> {
        RelayLookup::Unavailable {
            successful_relays: 0,
            failed_relays: 0,
        }
    }
}

impl RelayClient {
    pub async fn connect(
        operator_keys: Keys,
        relays: &[String],
        server_url: String,
    ) -> Result<Self, nostr_sdk::client::Error> {
        let client = Client::new(operator_keys);
        let mut relay_urls = BTreeSet::new();
        for relay in relays {
            client.add_relay(relay.as_str()).await?;
            relay_urls.insert(RelayUrl::parse(relay).expect("add_relay validated relay URL"));
        }
        client.connect().await;
        Ok(Self {
            client,
            relay_urls: relay_urls.into_iter().collect(),
            server_url,
        })
    }

    async fn query_relay(
        client: Client,
        relay_url: RelayUrl,
        filter: Filter,
    ) -> Result<Vec<Event>, RelayQueryFailure> {
        let relay = client.relay(relay_url.clone()).await.map_err(|error| {
            RelayQueryFailure::with_detail(&relay_url, RelayQueryFailureReason::Subscribe, error)
        })?;
        if !relay.is_connected() {
            relay.wait_for_connection(RELAY_FETCH_TIMEOUT).await;
        }
        if !relay.is_connected() {
            return Err(RelayQueryFailure::new(
                &relay_url,
                RelayQueryFailureReason::DisconnectedOrShutdown,
            ));
        }

        // nostr-relay-pool 0.44.1's fetch streams end with Ok on timeout or
        // disconnect because those auto-close paths drop the activity sender
        // without an error. Observe this relay's matching EOSE explicitly so
        // only a completed stored-event query contributes to completeness.
        let mut notifications = relay.notifications();
        let subscription_id = SubscriptionId::generate();
        relay
            .subscribe_with_id(subscription_id.clone(), filter, SubscribeOptions::default())
            .await
            .map_err(|error| {
                RelayQueryFailure::with_detail(
                    &relay_url,
                    RelayQueryFailureReason::Subscribe,
                    error,
                )
            })?;
        let cleanup =
            SubscriptionCleanup::new(relay.clone(), relay_url.clone(), subscription_id.clone());

        let result = tokio::time::timeout(RELAY_FETCH_TIMEOUT, async {
            let mut events = Vec::new();
            let mut serialized_bytes = 0usize;
            loop {
                match notifications.recv().await {
                    Ok(RelayNotification::Message { message }) => match message {
                        RelayMessage::Event {
                            subscription_id: event_subscription_id,
                            event,
                        } if event_subscription_id.as_ref() == &subscription_id => {
                            if events.len() >= MAX_RELAY_QUERY_EVENTS {
                                return Err(RelayQueryFailure::new(
                                    &relay_url,
                                    RelayQueryFailureReason::EventCountOverflow,
                                ));
                            }
                            let event = event.into_owned();
                            serialized_bytes = serialized_bytes
                                .checked_add(event.as_json().len())
                                .filter(|total| *total <= MAX_RELAY_QUERY_BYTES)
                                .ok_or_else(|| {
                                    RelayQueryFailure::new(
                                        &relay_url,
                                        RelayQueryFailureReason::SerializedBytesOverflow,
                                    )
                                })?;
                            events.push(event);
                        }
                        RelayMessage::EndOfStoredEvents(eose_subscription_id)
                            if eose_subscription_id.as_ref() == &subscription_id =>
                        {
                            return Ok(events);
                        }
                        RelayMessage::Closed {
                            subscription_id: closed_subscription_id,
                            message,
                        } if closed_subscription_id.as_ref() == &subscription_id => {
                            return Err(RelayQueryFailure::with_detail(
                                &relay_url,
                                RelayQueryFailureReason::Closed,
                                message,
                            ));
                        }
                        _ => {}
                    },
                    Ok(RelayNotification::RelayStatus { status })
                        if matches!(
                            status,
                            RelayStatus::Disconnected
                                | RelayStatus::Terminated
                                | RelayStatus::Banned
                        ) =>
                    {
                        return Err(RelayQueryFailure::new(
                            &relay_url,
                            RelayQueryFailureReason::DisconnectedOrShutdown,
                        ));
                    }
                    Ok(RelayNotification::Shutdown) => {
                        return Err(RelayQueryFailure::new(
                            &relay_url,
                            RelayQueryFailureReason::DisconnectedOrShutdown,
                        ));
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        return Err(RelayQueryFailure::with_detail(
                            &relay_url,
                            RelayQueryFailureReason::ReceiverLagged,
                            skipped,
                        ));
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        return Err(RelayQueryFailure::new(
                            &relay_url,
                            RelayQueryFailureReason::ReceiverClosed,
                        ));
                    }
                    _ => {}
                }
            }
        })
        .await
        .unwrap_or_else(|_| {
            Err(RelayQueryFailure::new(
                &relay_url,
                RelayQueryFailureReason::Timeout,
            ))
        });

        let unsubscribe_failure = cleanup.finish(&relay_url).await;
        match (result, unsubscribe_failure) {
            (Ok(_), Some(failure)) => Err(failure),
            (Err(failure), Some(cleanup_failure)) => {
                tracing::warn!(
                    relay_url = %cleanup_failure.relay_url,
                    reason = ?cleanup_failure.reason,
                    detail = cleanup_failure.detail.as_deref().unwrap_or(""),
                    "Relay query cleanup failed"
                );
                Err(failure)
            }
            (result, None) => result,
        }
    }

    async fn query_relays(&self, filter: Filter) -> (Vec<Event>, usize, usize) {
        let mut queries = tokio::task::JoinSet::new();
        let mut query_urls = HashMap::new();
        for relay_url in &self.relay_urls {
            let client = self.client.clone();
            let relay_url = relay_url.clone();
            let filter = filter.clone();
            let task = queries.spawn(Self::query_relay(client, relay_url.clone(), filter));
            query_urls.insert(task.id(), relay_url);
        }

        let mut events = Vec::new();
        let mut successful_relays = 0;
        let mut failed_relays = 0;
        while let Some(query) = queries.join_next_with_id().await {
            match query {
                Ok((task_id, Ok(fetched))) => {
                    query_urls.remove(&task_id);
                    successful_relays += 1;
                    events.extend(fetched);
                }
                Ok((task_id, Err(failure))) => {
                    query_urls.remove(&task_id);
                    tracing::warn!(
                        relay_url = %failure.relay_url,
                        reason = ?failure.reason,
                        detail = failure.detail.as_deref().unwrap_or(""),
                        "Relay query failed"
                    );
                    failed_relays += 1;
                }
                Err(error) => {
                    let relay_url = query_urls
                        .remove(&error.id())
                        .expect("relay query task URL is tracked");
                    let failure = RelayQueryFailure {
                        relay_url,
                        reason: RelayQueryFailureReason::TaskJoin,
                        detail: Some(error.to_string()),
                    };
                    tracing::warn!(
                        relay_url = %failure.relay_url,
                        reason = ?failure.reason,
                        detail = failure.detail.as_deref().unwrap_or(""),
                        "Relay query task failed"
                    );
                    failed_relays += 1;
                }
            }
        }

        (events, successful_relays, failed_relays)
    }

    fn collection_lookup(
        events: Vec<Event>,
        successful_relays: usize,
        failed_relays: usize,
    ) -> RelayLookup<Vec<Event>> {
        if successful_relays < MIN_SUCCESSFUL_RELAYS {
            return RelayLookup::Unavailable {
                successful_relays,
                failed_relays,
            };
        }

        let events = merge_events(events);
        if events.is_empty() {
            RelayLookup::ConfirmedAbsent { successful_relays }
        } else {
            RelayLookup::Found(events)
        }
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

    async fn lookup_event(&self, event_id: EventId) -> RelayLookup<Event> {
        let (events, successful_relays, failed_relays) =
            self.query_relays(Filter::new().event(event_id)).await;
        if let Some(event) = events.into_iter().find(|event| event.id == event_id) {
            return RelayLookup::Found(event);
        }
        if successful_relays >= MIN_SUCCESSFUL_RELAYS {
            RelayLookup::ConfirmedAbsent { successful_relays }
        } else {
            RelayLookup::Unavailable {
                successful_relays,
                failed_relays,
            }
        }
    }

    async fn lookup_listing(&self, developer_pubkey: PublicKey, d_tag: &str) -> RelayLookup<Event> {
        let filter = Filter::new()
            .kind(Kind::Custom(LISTING_KIND))
            .author(developer_pubkey)
            .identifier(d_tag);
        let (events, successful_relays, failed_relays) = self.query_relays(filter).await;
        if successful_relays < MIN_SUCCESSFUL_RELAYS {
            return RelayLookup::Unavailable {
                successful_relays,
                failed_relays,
            };
        }

        match select_latest_listing(events, developer_pubkey, d_tag) {
            Some(event) => RelayLookup::Found(event),
            None => RelayLookup::ConfirmedAbsent { successful_relays },
        }
    }

    async fn lookup_campaigns(&self, game_coordinate: &str) -> RelayLookup<Vec<Event>> {
        let (events, successful, failed) = self
            .query_relays(build_campaigns_filter(game_coordinate))
            .await;
        Self::collection_lookup(
            events
                .into_iter()
                .filter(|event| {
                    event.kind == Kind::Custom(EXPERIMENTAL_ADP_CAMPAIGN_KIND)
                        && has_tag(event, "a", game_coordinate)
                })
                .collect(),
            successful,
            failed,
        )
    }

    async fn lookup_campaign_chain(
        &self,
        developer_pubkey: PublicKey,
        campaign_d: &str,
    ) -> RelayLookup<Vec<Event>> {
        let (events, successful, failed) = self
            .query_relays(build_campaign_chain_filter(developer_pubkey, campaign_d))
            .await;
        Self::collection_lookup(
            events
                .into_iter()
                .filter(|event| {
                    event.kind == Kind::Custom(EXPERIMENTAL_ADP_CAMPAIGN_KIND)
                        && event.pubkey == developer_pubkey
                        && has_tag(event, "d", campaign_d)
                })
                .collect(),
            successful,
            failed,
        )
    }

    async fn lookup_entitlements(
        &self,
        buyer_pubkey: PublicKey,
        game_coordinate: &str,
    ) -> RelayLookup<Vec<Event>> {
        let (events, successful, failed) = self
            .query_relays(build_entitlements_filter(buyer_pubkey, game_coordinate))
            .await;
        Self::collection_lookup(
            events
                .into_iter()
                .filter(|event| {
                    event.kind == Kind::Custom(EXPERIMENTAL_ENTITLEMENT_GRANT_KIND)
                        && has_tag(event, "p", &buyer_pubkey.to_hex())
                        && has_tag(event, "a", game_coordinate)
                })
                .collect(),
            successful,
            failed,
        )
    }

    async fn lookup_grant_events_by_d(&self, grant_d: &str) -> RelayLookup<Vec<Event>> {
        let (events, successful, failed) = self
            .query_relays(build_grant_events_by_d_filter(grant_d))
            .await;
        Self::collection_lookup(
            events
                .into_iter()
                .filter(|event| {
                    event.kind == Kind::Custom(EXPERIMENTAL_ENTITLEMENT_GRANT_KIND)
                        && has_tag(event, "d", grant_d)
                })
                .collect(),
            successful,
            failed,
        )
    }

    async fn lookup_receipts(
        &self,
        buyer_pubkey: PublicKey,
        game_coordinate: &str,
    ) -> RelayLookup<Vec<Event>> {
        let (events, successful, failed) = self
            .query_relays(build_receipts_filter(buyer_pubkey, game_coordinate))
            .await;
        Self::collection_lookup(
            events
                .into_iter()
                .filter(|event| {
                    event.kind == Kind::Custom(RECEIPT_KIND)
                        && has_tag(event, "p", &buyer_pubkey.to_hex())
                        && has_tag(event, "a", game_coordinate)
                })
                .collect(),
            successful,
            failed,
        )
    }

    async fn lookup_authorizations(
        &self,
        developer_pubkey: PublicKey,
        game_coordinate: &str,
    ) -> RelayLookup<Vec<Event>> {
        let (events, successful, failed) = self
            .query_relays(build_authorizations_filter(
                developer_pubkey,
                game_coordinate,
            ))
            .await;
        Self::collection_lookup(
            events
                .into_iter()
                .filter(|event| {
                    event.kind == Kind::Custom(ACCEPTANCE_KIND)
                        && event.pubkey == developer_pubkey
                        && has_tag(event, "a", game_coordinate)
                })
                .collect(),
            successful,
            failed,
        )
    }

    async fn lookup_authorization_successors(
        &self,
        developer_pubkey: PublicKey,
        event_id: EventId,
    ) -> RelayLookup<Vec<Event>> {
        let (events, successful, failed) = self
            .query_relays(build_authorization_successors_filter(
                developer_pubkey,
                event_id,
            ))
            .await;
        Self::collection_lookup(
            events
                .into_iter()
                .filter(|event| {
                    event.kind == Kind::Custom(ACCEPTANCE_KIND)
                        && event.pubkey == developer_pubkey
                        && has_tag(event, "e", &event_id.to_hex())
                })
                .collect(),
            successful,
            failed,
        )
    }
}

pub async fn complete_predecessor_chain<R, F>(
    relay: &R,
    initial_events: Vec<Event>,
    predecessor: F,
) -> Result<Vec<Event>, RelayCompletionError>
where
    R: Relay + ?Sized,
    F: Fn(&Event) -> Result<Option<EventId>, ChainResolutionError>,
{
    let mut events: BTreeMap<EventId, Event> = initial_events
        .into_iter()
        .map(|event| (event.id, event))
        .collect();
    if events.len() > MAX_CHAIN_EVENTS {
        return Err(ChainResolutionError::LimitExceeded.into());
    }

    let mut pending: Vec<(EventId, usize, BTreeSet<EventId>)> = events
        .keys()
        .copied()
        .map(|event_id| (event_id, 0, BTreeSet::from([event_id])))
        .collect();
    while let Some((event_id, depth, path)) = pending.pop() {
        let event = events
            .get(&event_id)
            .expect("pending event is present")
            .clone();
        let Some(predecessor_id) = predecessor(&event)? else {
            continue;
        };
        if path.contains(&predecessor_id) {
            return Err(ChainResolutionError::Cycle.into());
        }
        if depth >= MAX_CHAIN_FETCH_DEPTH {
            return Err(ChainResolutionError::LimitExceeded.into());
        }

        if !events.contains_key(&predecessor_id) {
            let predecessor_event = match relay.lookup_event(predecessor_id).await {
                RelayLookup::Found(event) if event.id == predecessor_id => event,
                RelayLookup::Found(_) => {
                    return Err(ChainResolutionError::InvariantMutation(
                        "relay returned the wrong immutable event",
                    )
                    .into())
                }
                RelayLookup::ConfirmedAbsent { successful_relays }
                    if successful_relays >= MIN_SUCCESSFUL_RELAYS =>
                {
                    return Err(ChainResolutionError::MissingPredecessor(predecessor_id).into())
                }
                RelayLookup::ConfirmedAbsent { successful_relays } => {
                    return Err(RelayCompletionError::Unavailable {
                        successful_relays,
                        failed_relays: 0,
                    })
                }
                RelayLookup::Unavailable {
                    successful_relays,
                    failed_relays,
                } => {
                    return Err(RelayCompletionError::Unavailable {
                        successful_relays,
                        failed_relays,
                    })
                }
            };
            if events.len() >= MAX_CHAIN_EVENTS {
                return Err(ChainResolutionError::LimitExceeded.into());
            }
            events.insert(predecessor_id, predecessor_event);
        }

        let mut predecessor_path = path;
        predecessor_path.insert(predecessor_id);
        pending.push((predecessor_id, depth + 1, predecessor_path));
    }

    Ok(events.into_values().collect())
}

pub async fn complete_authorization_successors<R>(
    relay: &R,
    developer_pubkey: PublicKey,
    root: Event,
) -> Result<Vec<Event>, RelayCompletionError>
where
    R: Relay + ?Sized,
{
    let root_id = root.id;
    let mut events = BTreeMap::from([(root_id, root)]);
    let mut pending = vec![(root_id, 0, BTreeSet::from([root_id]))];
    let mut enqueued = BTreeSet::from([root_id]);
    let mut expanded = BTreeSet::new();

    while let Some((event_id, depth, path)) = pending.pop() {
        if !expanded.insert(event_id) {
            continue;
        }
        let successors = match relay
            .lookup_authorization_successors(developer_pubkey, event_id)
            .await
        {
            RelayLookup::Found(events) => merge_events(events),
            RelayLookup::ConfirmedAbsent { successful_relays }
                if successful_relays >= MIN_SUCCESSFUL_RELAYS =>
            {
                continue
            }
            RelayLookup::ConfirmedAbsent { successful_relays } => {
                return Err(RelayCompletionError::Unavailable {
                    successful_relays,
                    failed_relays: 0,
                })
            }
            RelayLookup::Unavailable {
                successful_relays,
                failed_relays,
            } => {
                return Err(RelayCompletionError::Unavailable {
                    successful_relays,
                    failed_relays,
                })
            }
        };

        if !successors.is_empty() && depth >= MAX_CHAIN_FETCH_DEPTH {
            return Err(ChainResolutionError::LimitExceeded.into());
        }
        for successor in successors {
            if path.contains(&successor.id) {
                return Err(ChainResolutionError::Cycle.into());
            }
            if !events.contains_key(&successor.id) {
                if events.len() >= MAX_CHAIN_EVENTS {
                    return Err(ChainResolutionError::LimitExceeded.into());
                }
                events.insert(successor.id, successor.clone());
            }
            let mut successor_path = path.clone();
            successor_path.insert(successor.id);
            if enqueued.insert(successor.id) {
                pending.push((successor.id, depth + 1, successor_path));
            }
        }
    }

    Ok(events.into_values().collect())
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet, HashMap};
    use std::sync::Mutex;

    use adp_core::ChainResolutionError;

    use super::*;

    fn serialized(filter: Filter) -> serde_json::Value {
        serde_json::to_value(filter).expect("serialize filter")
    }

    fn listing_at(keys: &Keys, d_tag: &str, created_at: u64) -> Event {
        listing_with(keys, vec![vec!["d", d_tag]], "", created_at)
    }

    fn listing_with(keys: &Keys, tags: Vec<Vec<&str>>, content: &str, created_at: u64) -> Event {
        EventBuilder::new(Kind::Custom(LISTING_KIND), content)
            .tags(
                tags.into_iter()
                    .map(|tag| Tag::parse(tag).expect("valid tag")),
            )
            .custom_created_at(Timestamp::from(created_at))
            .sign_with_keys(keys)
            .expect("sign listing")
    }

    fn event_at(created_at: u64) -> Event {
        EventBuilder::new(Kind::TextNote, "")
            .custom_created_at(Timestamp::from(created_at))
            .sign_with_keys(&Keys::generate())
            .expect("sign event")
    }

    #[derive(Default)]
    struct LookupRelay {
        events: Mutex<BTreeMap<EventId, Event>>,
        unavailable_events: Mutex<BTreeSet<EventId>>,
        successors: Mutex<BTreeMap<EventId, Vec<Event>>>,
        unavailable_successors: Mutex<BTreeSet<EventId>>,
        successor_queries: Mutex<BTreeMap<EventId, usize>>,
    }

    struct TypedOnlyRelay {
        listing: Event,
        receipts: Vec<Event>,
    }

    #[async_trait]
    impl Relay for TypedOnlyRelay {
        fn server_url(&self) -> &str {
            "http://localhost:8080"
        }

        async fn publish(&self, _event: &Event) -> RelayResult<()> {
            Ok(())
        }

        async fn lookup_listing(
            &self,
            _developer_pubkey: PublicKey,
            _d_tag: &str,
        ) -> RelayLookup<Event> {
            RelayLookup::Found(self.listing.clone())
        }

        async fn lookup_receipts(
            &self,
            _buyer_pubkey: PublicKey,
            _game_coordinate: &str,
        ) -> RelayLookup<Vec<Event>> {
            RelayLookup::Found(self.receipts.clone())
        }
    }

    #[async_trait]
    impl Relay for LookupRelay {
        fn server_url(&self) -> &str {
            "http://localhost:8080"
        }

        async fn publish(&self, _event: &Event) -> RelayResult<()> {
            Ok(())
        }

        async fn fetch_listing(
            &self,
            _developer_pubkey: PublicKey,
            _d_tag: &str,
        ) -> RelayResult<Option<Event>> {
            Ok(None)
        }

        async fn fetch_receipts(
            &self,
            _buyer_pubkey: PublicKey,
            _game_coordinate: &str,
        ) -> RelayResult<Vec<Event>> {
            Ok(Vec::new())
        }

        async fn lookup_event(&self, event_id: EventId) -> RelayLookup<Event> {
            if self
                .unavailable_events
                .lock()
                .expect("lock unavailable events")
                .contains(&event_id)
            {
                return RelayLookup::Unavailable {
                    successful_relays: 1,
                    failed_relays: 1,
                };
            }
            self.events
                .lock()
                .expect("lock events")
                .get(&event_id)
                .cloned()
                .map(RelayLookup::Found)
                .unwrap_or(RelayLookup::ConfirmedAbsent {
                    successful_relays: MIN_SUCCESSFUL_RELAYS,
                })
        }

        async fn lookup_authorization_successors(
            &self,
            _developer_pubkey: PublicKey,
            event_id: EventId,
        ) -> RelayLookup<Vec<Event>> {
            *self
                .successor_queries
                .lock()
                .expect("lock successor queries")
                .entry(event_id)
                .or_default() += 1;
            if self
                .unavailable_successors
                .lock()
                .expect("lock unavailable successors")
                .contains(&event_id)
            {
                return RelayLookup::Unavailable {
                    successful_relays: 1,
                    failed_relays: 1,
                };
            }
            self.successors
                .lock()
                .expect("lock successors")
                .get(&event_id)
                .cloned()
                .map(RelayLookup::Found)
                .unwrap_or(RelayLookup::ConfirmedAbsent {
                    successful_relays: MIN_SUCCESSFUL_RELAYS,
                })
        }
    }

    #[test]
    fn select_latest_listing_uses_highest_created_at_regardless_order() {
        let developer = Keys::generate();
        let older = listing_at(&developer, "game-1", 1_700_000_000);
        let newer = listing_at(&developer, "game-1", 1_800_000_000);

        let selected =
            select_latest_listing(vec![newer.clone(), older], developer.public_key(), "game-1")
                .expect("selected event");

        assert_eq!(selected.id, newer.id);
    }

    #[test]
    fn select_latest_listing_rejects_duplicate_d_tags() {
        let developer = Keys::generate();
        let valid = listing_at(&developer, "game-1", 1);
        let duplicate_d = listing_with(
            &developer,
            vec![vec!["d", "game-1"], vec!["d", "other"]],
            "",
            2,
        );

        let selected = select_latest_listing(
            vec![duplicate_d, valid.clone()],
            developer.public_key(),
            "game-1",
        )
        .expect("valid listing remains");

        assert_eq!(selected.id, valid.id);
    }

    #[test]
    fn select_latest_listing_rejects_d_tag_with_extra_field() {
        let developer = Keys::generate();
        let valid = listing_at(&developer, "game-1", 1);
        let malformed = listing_with(&developer, vec![vec!["d", "game-1", "extra"]], "", 2);

        let selected = select_latest_listing(
            vec![malformed, valid.clone()],
            developer.public_key(),
            "game-1",
        )
        .expect("valid listing remains");

        assert_eq!(selected.id, valid.id);
    }

    #[test]
    fn select_latest_listing_rejects_invalid_signature() {
        let developer = Keys::generate();
        let valid = listing_at(&developer, "game-1", 1);
        let mut invalid = listing_with(&developer, vec![vec!["d", "game-1"]], "different", 2);
        invalid.sig = valid.sig;

        let selected = select_latest_listing(
            vec![invalid, valid.clone()],
            developer.public_key(),
            "game-1",
        )
        .expect("valid listing remains");

        assert_eq!(selected.id, valid.id);
    }

    #[test]
    fn select_latest_listing_validates_before_deduplicating_event_id() {
        let developer = Keys::generate();
        let valid = listing_at(&developer, "game-1", 1);
        let mut invalid_copy = valid.clone();
        invalid_copy.sig = listing_at(&developer, "other", 2).sig;

        let selected = select_latest_listing(
            vec![valid.clone(), invalid_copy],
            developer.public_key(),
            "game-1",
        )
        .expect("valid copy remains");

        assert_eq!(selected, valid);
    }

    #[test]
    fn select_latest_listing_uses_lowest_event_id_for_equal_timestamp() {
        let developer = Keys::generate();
        let first = listing_with(&developer, vec![vec!["d", "game-1"]], "first", 1);
        let second = listing_with(&developer, vec![vec!["d", "game-1"]], "second", 1);
        let expected = if first.id < second.id {
            first.clone()
        } else {
            second.clone()
        };

        assert_eq!(
            select_latest_listing(
                vec![first.clone(), second.clone()],
                developer.public_key(),
                "game-1",
            ),
            Some(expected.clone())
        );
        assert_eq!(
            select_latest_listing(vec![second, first], developer.public_key(), "game-1",),
            Some(expected)
        );
    }

    #[test]
    fn receipt_filter_targets_buyer_p_tag_not_author() {
        let buyer = Keys::generate().public_key();
        let coordinate = "30402:developer:game-1";

        let filter = build_receipts_filter(buyer, coordinate);
        let serialized = serde_json::to_value(&filter).expect("serialize filter");

        let serialized = serialized.as_object().expect("filter object");

        assert_eq!(serialized.len(), 3);
        assert_eq!(serialized["kinds"][0], RECEIPT_KIND);
        assert_eq!(serialized["#p"][0], buyer.to_hex());
        assert_eq!(serialized["#a"][0], coordinate);
        assert!(serialized
            .keys()
            .all(|key| matches!(key.as_str(), "kinds" | "#p" | "#a")));
    }

    #[test]
    fn campaign_filter_targets_exact_kind_and_a_tag() {
        let coordinate = "30402:developer:game-1";
        let serialized = serialized(build_campaigns_filter(coordinate));

        assert_eq!(serialized["kinds"][0], EXPERIMENTAL_ADP_CAMPAIGN_KIND);
        assert_eq!(serialized["#a"][0], coordinate);
        assert_eq!(serialized.as_object().expect("filter object").len(), 2);
    }

    #[test]
    fn campaign_chain_filter_targets_author_and_exact_d_tag() {
        let developer = Keys::generate().public_key();
        let serialized = serialized(build_campaign_chain_filter(developer, "campaign-1"));

        assert_eq!(serialized["kinds"][0], EXPERIMENTAL_ADP_CAMPAIGN_KIND);
        assert_eq!(serialized["authors"][0], developer.to_hex());
        assert_eq!(serialized["#d"][0], "campaign-1");
        assert_eq!(serialized.as_object().expect("filter object").len(), 3);
    }

    #[test]
    fn entitlement_filter_targets_buyer_and_coordinate_tags() {
        let buyer = Keys::generate().public_key();
        let coordinate = "30402:developer:game-1";
        let serialized = serialized(build_entitlements_filter(buyer, coordinate));

        assert_eq!(serialized["kinds"][0], EXPERIMENTAL_ENTITLEMENT_GRANT_KIND);
        assert_eq!(serialized["#p"][0], buyer.to_hex());
        assert_eq!(serialized["#a"][0], coordinate);
        assert_eq!(serialized.as_object().expect("filter object").len(), 3);
    }

    #[test]
    fn grant_events_by_d_filter_targets_exact_d_tag() {
        let serialized = serialized(build_grant_events_by_d_filter("grant-1"));

        assert_eq!(serialized["kinds"][0], EXPERIMENTAL_ENTITLEMENT_GRANT_KIND);
        assert_eq!(serialized["#d"][0], "grant-1");
        assert_eq!(serialized.as_object().expect("filter object").len(), 2);
    }

    #[test]
    fn authorization_filter_targets_author_and_coordinate() {
        let developer = Keys::generate().public_key();
        let coordinate = "30402:developer:game-1";
        let serialized = serialized(build_authorizations_filter(developer, coordinate));

        assert_eq!(serialized["kinds"][0], ACCEPTANCE_KIND);
        assert_eq!(serialized["authors"][0], developer.to_hex());
        assert_eq!(serialized["#a"][0], coordinate);
        assert_eq!(serialized.as_object().expect("filter object").len(), 3);
    }

    #[test]
    fn authorization_successor_filter_targets_author_and_e_tag() {
        let developer = Keys::generate().public_key();
        let predecessor = EventId::all_zeros();
        let serialized = serialized(build_authorization_successors_filter(
            developer,
            predecessor,
        ));

        assert_eq!(serialized["kinds"][0], ACCEPTANCE_KIND);
        assert_eq!(serialized["authors"][0], developer.to_hex());
        assert_eq!(serialized["#e"][0], predecessor.to_hex());
        assert_eq!(serialized.as_object().expect("filter object").len(), 3);
    }

    #[test]
    fn protocol_kind_constants_are_centralized_and_exclude_1021() {
        assert_eq!(LISTING_KIND, adp_core::listing::LISTING_KIND);
        assert_eq!(RECEIPT_KIND, adp_core::receipt::RECEIPT_KIND);
        assert_eq!(EXPERIMENTAL_ENTITLEMENT_GRANT_KIND, 1030);
        assert_eq!(EXPERIMENTAL_ADP_CAMPAIGN_KIND, 1031);
        assert_ne!(EXPERIMENTAL_ENTITLEMENT_GRANT_KIND, 1021);
        assert_ne!(EXPERIMENTAL_ADP_CAMPAIGN_KIND, 1021);
    }

    #[tokio::test]
    async fn predecessor_completion_recovers_missing_immutable_event() {
        let root = event_at(1);
        let tip = event_at(2);
        let relay = LookupRelay::default();
        relay
            .events
            .lock()
            .expect("lock events")
            .insert(root.id, root.clone());
        let predecessors = HashMap::from([(tip.id, root.id)]);

        let completed = complete_predecessor_chain(&relay, vec![tip.clone()], |event| {
            Ok(predecessors.get(&event.id).copied())
        })
        .await
        .expect("complete predecessor chain");

        let mut expected = vec![root, tip];
        expected.sort_by_key(|event| event.id);
        assert_eq!(completed, expected);
    }

    #[tokio::test]
    async fn predecessor_completion_allows_depth_32_and_rejects_33() {
        let events: Vec<Event> = (0..=33).map(event_at).collect();
        let predecessors: HashMap<EventId, EventId> = events
            .windows(2)
            .map(|pair| (pair[1].id, pair[0].id))
            .collect();
        let relay = LookupRelay::default();
        relay.events.lock().expect("lock events").extend(
            events
                .iter()
                .take(33)
                .cloned()
                .map(|event| (event.id, event)),
        );

        let completed = complete_predecessor_chain(&relay, vec![events[32].clone()], |event| {
            Ok(predecessors.get(&event.id).copied())
        })
        .await
        .expect("depth 32 is allowed");
        assert_eq!(completed.len(), 33);

        let error = complete_predecessor_chain(&relay, vec![events[33].clone()], |event| {
            Ok(predecessors.get(&event.id).copied())
        })
        .await
        .expect_err("depth 33 exceeds the limit");
        assert_eq!(
            error,
            RelayCompletionError::Malformed(ChainResolutionError::LimitExceeded)
        );
    }

    #[tokio::test]
    async fn predecessor_completion_allows_64_events_and_rejects_65() {
        let relay = LookupRelay::default();
        let events: Vec<Event> = (0..65).map(event_at).collect();

        let completed = complete_predecessor_chain(&relay, events[..64].to_vec(), |_| Ok(None))
            .await
            .expect("64 events are allowed");
        assert_eq!(completed.len(), 64);

        let error = complete_predecessor_chain(&relay, events, |_| Ok(None))
            .await
            .expect_err("65 events exceed the limit");
        assert_eq!(
            error,
            RelayCompletionError::Malformed(ChainResolutionError::LimitExceeded)
        );
    }

    #[tokio::test]
    async fn predecessor_completion_detects_cycle() {
        let first = event_at(1);
        let second = event_at(2);
        let predecessors = HashMap::from([(first.id, second.id), (second.id, first.id)]);

        let error =
            complete_predecessor_chain(&LookupRelay::default(), vec![first, second], |event| {
                Ok(predecessors.get(&event.id).copied())
            })
            .await
            .expect_err("cycle must fail closed");

        assert_eq!(
            error,
            RelayCompletionError::Malformed(ChainResolutionError::Cycle)
        );
    }

    #[tokio::test]
    async fn predecessor_completion_distinguishes_missing_from_unavailable() {
        let tip = event_at(1);
        let missing = event_at(2).id;
        let predecessors = HashMap::from([(tip.id, missing)]);

        let missing_error =
            complete_predecessor_chain(&LookupRelay::default(), vec![tip.clone()], |event| {
                Ok(predecessors.get(&event.id).copied())
            })
            .await
            .expect_err("confirmed missing predecessor is malformed");
        assert_eq!(
            missing_error,
            RelayCompletionError::Malformed(ChainResolutionError::MissingPredecessor(missing))
        );

        let unavailable_relay = LookupRelay::default();
        unavailable_relay
            .unavailable_events
            .lock()
            .expect("lock unavailable events")
            .insert(missing);
        let unavailable_error =
            complete_predecessor_chain(&unavailable_relay, vec![tip], |event| {
                Ok(predecessors.get(&event.id).copied())
            })
            .await
            .expect_err("unavailable predecessor remains distinct");
        assert_eq!(
            unavailable_error,
            RelayCompletionError::Unavailable {
                successful_relays: 1,
                failed_relays: 1,
            }
        );
    }

    #[tokio::test]
    async fn authorization_completion_requires_complete_empty_successor_lookup() {
        let root = event_at(1);
        let relay = LookupRelay::default();
        relay
            .unavailable_successors
            .lock()
            .expect("lock unavailable successors")
            .insert(root.id);

        let error = complete_authorization_successors(&relay, Keys::generate().public_key(), root)
            .await
            .expect_err("one success cannot prove no revocation");

        assert_eq!(
            error,
            RelayCompletionError::Unavailable {
                successful_relays: 1,
                failed_relays: 1,
            }
        );
    }

    #[tokio::test]
    async fn authorization_completion_returns_forks_without_selecting_one() {
        let root = event_at(1);
        let left = event_at(2);
        let right = event_at(3);
        let relay = LookupRelay::default();
        relay
            .successors
            .lock()
            .expect("lock successors")
            .insert(root.id, vec![right.clone(), left.clone()]);

        let completed =
            complete_authorization_successors(&relay, Keys::generate().public_key(), root.clone())
                .await
                .expect("fork is returned to core resolver");

        let mut expected = vec![root, left, right];
        expected.sort_by_key(|event| event.id);
        assert_eq!(completed, expected);
    }

    #[tokio::test]
    async fn authorization_completion_queries_convergent_successor_once() {
        let root = event_at(1);
        let left = event_at(2);
        let right = event_at(3);
        let shared = event_at(4);
        let relay = LookupRelay::default();
        relay.successors.lock().expect("lock successors").extend([
            (root.id, vec![left.clone(), right.clone()]),
            (left.id, vec![shared.clone()]),
            (right.id, vec![shared.clone()]),
        ]);

        let completed =
            complete_authorization_successors(&relay, Keys::generate().public_key(), root.clone())
                .await
                .expect("convergent graph completes");

        assert_eq!(completed.len(), 4);
        assert_eq!(
            relay
                .successor_queries
                .lock()
                .expect("lock successor queries")
                .get(&shared.id),
            Some(&1)
        );
    }

    #[tokio::test]
    async fn authorization_completion_detects_cycle() {
        let root = event_at(1);
        let relay = LookupRelay::default();
        relay
            .successors
            .lock()
            .expect("lock successors")
            .insert(root.id, vec![root.clone()]);

        let error = complete_authorization_successors(&relay, Keys::generate().public_key(), root)
            .await
            .expect_err("cycle must fail closed");

        assert_eq!(
            error,
            RelayCompletionError::Malformed(ChainResolutionError::Cycle)
        );
    }

    #[tokio::test]
    async fn transitional_fetch_defaults_adapt_typed_lookup_results() {
        let developer = Keys::generate();
        let listing = listing_at(&developer, "game-1", 1);
        let receipt = event_at(2);
        let relay = TypedOnlyRelay {
            listing: listing.clone(),
            receipts: vec![receipt.clone()],
        };

        assert_eq!(
            relay
                .fetch_listing(developer.public_key(), "game-1")
                .await
                .expect("transitional listing wrapper"),
            Some(listing)
        );
        assert_eq!(
            relay
                .fetch_receipts(developer.public_key(), "30402:developer:game-1")
                .await
                .expect("transitional receipt wrapper"),
            vec![receipt]
        );
    }
}
