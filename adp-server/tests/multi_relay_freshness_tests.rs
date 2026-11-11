use std::collections::BTreeSet;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use adp_server::relay::{
    Relay, RelayClient, RelayLookup, MAX_RELAY_QUERY_BYTES, MAX_RELAY_QUERY_EVENTS,
};
use futures_util::{SinkExt, StreamExt};
use nostr_sdk::{Event, EventBuilder, Keys, Kind, Tag, Timestamp};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::sync::{Mutex, Notify};
use tokio::task::{JoinHandle, JoinSet};
use tokio_tungstenite::{accept_async, tungstenite::Message};

const TEST_FETCH_TIMEOUT: Duration = Duration::from_millis(100);

struct LocalRelay {
    url: String,
    task: JoinHandle<()>,
    active_subscriptions: Arc<StdMutex<BTreeSet<String>>>,
    subscription_changed: Arc<Notify>,
}

impl LocalRelay {
    async fn run(events: Vec<Event>) -> Self {
        Self::run_with_behavior(events, true, false).await
    }

    async fn run_without_eose(events: Vec<Event>) -> Self {
        Self::run_with_behavior(events, false, false).await
    }

    async fn run_disconnect_before_eose(events: Vec<Event>) -> Self {
        Self::run_with_behavior(events, false, true).await
    }

    async fn run_with_behavior(
        events: Vec<Event>,
        send_eose: bool,
        disconnect_before_eose: bool,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind local relay");
        let addr = listener.local_addr().expect("local relay addr");
        let events = Arc::new(Mutex::new(events));
        let active_subscriptions = Arc::new(StdMutex::new(BTreeSet::new()));
        let subscription_changed = Arc::new(Notify::new());
        let task = tokio::spawn(run_relay(
            listener,
            events,
            send_eose,
            disconnect_before_eose,
            active_subscriptions.clone(),
            subscription_changed.clone(),
        ));

        Self {
            url: format!("ws://{addr}"),
            task,
            active_subscriptions,
            subscription_changed,
        }
    }

    async fn wait_for_active_subscriptions(&self, expected: usize) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let changed = self.subscription_changed.notified();
                if self
                    .active_subscriptions
                    .lock()
                    .expect("lock active subscriptions")
                    .len()
                    == expected
                {
                    return;
                }
                changed.await;
            }
        })
        .await
        .expect("active subscription count reached expected value");
    }
}

impl Drop for LocalRelay {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn run_relay(
    listener: TcpListener,
    events: Arc<Mutex<Vec<Event>>>,
    send_eose: bool,
    disconnect_before_eose: bool,
    active_subscriptions: Arc<StdMutex<BTreeSet<String>>>,
    subscription_changed: Arc<Notify>,
) {
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else {
                    break;
                };
                let events = events.clone();
                let active_subscriptions = active_subscriptions.clone();
                let subscription_changed = subscription_changed.clone();
                connections.spawn(async move {
                    let mut connection_subscriptions = BTreeSet::new();
                    run_connection(
                        stream,
                        events,
                        send_eose,
                        disconnect_before_eose,
                        &active_subscriptions,
                        &subscription_changed,
                        &mut connection_subscriptions,
                    )
                    .await;
                    let mut active = active_subscriptions
                        .lock()
                        .expect("lock active subscriptions");
                    for subscription_id in connection_subscriptions {
                        active.remove(&subscription_id);
                    }
                    drop(active);
                    subscription_changed.notify_waiters();
                });
            }
            _ = connections.join_next(), if !connections.is_empty() => {}
        }
    }
}

async fn run_connection(
    stream: tokio::net::TcpStream,
    events: Arc<Mutex<Vec<Event>>>,
    send_eose: bool,
    disconnect_before_eose: bool,
    active_subscriptions: &Arc<StdMutex<BTreeSet<String>>>,
    subscription_changed: &Arc<Notify>,
    connection_subscriptions: &mut BTreeSet<String>,
) {
    let Ok(mut socket) = accept_async(stream).await else {
        return;
    };

    while let Some(Ok(message)) = socket.next().await {
        let Message::Text(text) = message else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let Some(parts) = value.as_array() else {
            continue;
        };
        let Some(kind) = parts.first().and_then(|part| part.as_str()) else {
            continue;
        };

        match kind {
            "REQ" => {
                let Some(subscription_id) = parts.get(1).and_then(|part| part.as_str()) else {
                    continue;
                };
                connection_subscriptions.insert(subscription_id.to_string());
                active_subscriptions
                    .lock()
                    .expect("lock active subscriptions")
                    .insert(subscription_id.to_string());
                subscription_changed.notify_waiters();
                let stored_events = events.lock().await.clone();
                for event in stored_events {
                    let response = json!(["EVENT", subscription_id, event]);
                    if socket
                        .send(Message::Text(response.to_string().into()))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                if disconnect_before_eose {
                    return;
                }
                if send_eose {
                    let eose = json!(["EOSE", subscription_id]);
                    if socket
                        .send(Message::Text(eose.to_string().into()))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
            }
            "EVENT" => {
                let Some(event_value) = parts.get(1) else {
                    continue;
                };
                let Ok(event) = serde_json::from_value::<Event>(event_value.clone()) else {
                    continue;
                };
                let event_id = event.id.to_hex();
                events.lock().await.push(event);
                let ok = json!(["OK", event_id, true, ""]);
                if socket
                    .send(Message::Text(ok.to_string().into()))
                    .await
                    .is_err()
                {
                    return;
                }
            }
            "CLOSE" => {
                let Some(subscription_id) = parts.get(1).and_then(|part| part.as_str()) else {
                    continue;
                };
                connection_subscriptions.remove(subscription_id);
                active_subscriptions
                    .lock()
                    .expect("lock active subscriptions")
                    .remove(subscription_id);
                subscription_changed.notify_waiters();
            }
            _ => {}
        }
    }
}

fn signed_listing(developer: &Keys, d_tag: &str, created_at: u64) -> Event {
    signed_listing_with_content(developer, d_tag, created_at, "")
}

fn signed_listing_with_content(
    developer: &Keys,
    d_tag: &str,
    created_at: u64,
    content: &str,
) -> Event {
    EventBuilder::new(Kind::Custom(30402), content)
        .tags(vec![Tag::parse(vec!["d", d_tag]).expect("valid d tag")])
        .custom_created_at(Timestamp::from(created_at))
        .sign_with_keys(developer)
        .expect("sign listing")
}

fn signed_receipt(buyer: &Keys, coordinate: &str, created_at: u64) -> Event {
    EventBuilder::new(Kind::Custom(adp_core::receipt::RECEIPT_KIND), "encrypted")
        .tags(vec![
            Tag::parse(vec!["p", &buyer.public_key().to_hex()]).expect("valid p tag"),
            Tag::parse(vec!["a", coordinate]).expect("valid a tag"),
        ])
        .custom_created_at(Timestamp::from(created_at))
        .sign_with_keys(&Keys::generate())
        .expect("sign receipt")
}

async fn closed_relay_url() -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind closed relay endpoint");
    let addr = listener.local_addr().expect("closed relay endpoint addr");
    drop(listener);
    format!("ws://{addr}")
}

async fn connect_client(relays: &[String]) -> RelayClient {
    RelayClient::connect_with_timeout(
        Keys::generate(),
        relays,
        "http://localhost:8080".to_string(),
        TEST_FETCH_TIMEOUT,
    )
    .await
    .expect("connect relay client")
}

#[tokio::test]
async fn sanity_check_connect_tolerates_dead_relay() {
    let result = RelayClient::connect_with_timeout(
        Keys::generate(),
        &[closed_relay_url().await],
        "http://localhost:8080".to_string(),
        TEST_FETCH_TIMEOUT,
    )
    .await;

    assert!(
        result.is_ok(),
        "connect() should not eagerly fail on an unreachable relay"
    );
}

#[tokio::test]
async fn fetch_listing_uses_highest_created_at_regardless_of_relay_response_order() {
    let developer = Keys::generate();
    let older_listing = signed_listing(&developer, "game-1", 1_700_000_000);
    let newer_listing = signed_listing(&developer, "game-1", 1_800_000_000);
    let relay_a = LocalRelay::run(vec![older_listing]).await;
    let relay_b = LocalRelay::run(vec![newer_listing]).await;

    let relay_client = connect_client(&[relay_a.url.clone(), relay_b.url.clone()]).await;

    let fetched = relay_client
        .fetch_listing(developer.public_key(), "game-1")
        .await
        .expect("fetch listing")
        .expect("listing should be found across both relays");

    assert_eq!(
        fetched.created_at.as_secs(),
        1_800_000_000,
        "expected the newer conflicting version from relay_b; an older timestamp means fetch_listing is not aggregating across all configured relays"
    );
}

#[tokio::test]
async fn transitional_fetch_listing_rejects_incomplete_relay_lookup() {
    let developer = Keys::generate();
    let relay_a = LocalRelay::run(Vec::new()).await;
    let relay_failed = LocalRelay::run_disconnect_before_eose(Vec::new()).await;

    let relay_client = connect_client(&[relay_a.url.clone(), relay_failed.url.clone()]).await;

    let error = relay_client
        .fetch_listing(developer.public_key(), "game-1")
        .await
        .expect_err("transitional adapter must preserve unavailable outcome");

    assert!(
        error.to_string().contains("relay lookup unavailable"),
        "unexpected transitional adapter error: {error}"
    );
}

#[tokio::test]
async fn lookup_listing_returns_verified_event_despite_failed_relay() {
    let developer = Keys::generate();
    let listing = signed_listing(&developer, "game-1", 1_700_000_000);
    let relay = LocalRelay::run(vec![listing.clone()]).await;
    let relay_failed = LocalRelay::run_disconnect_before_eose(Vec::new()).await;
    let relay_client = connect_client(&[relay.url.clone(), relay_failed.url.clone()]).await;

    let lookup = relay_client
        .lookup_listing(developer.public_key(), "game-1")
        .await;

    assert_eq!(lookup, RelayLookup::Found(listing));
}

#[tokio::test]
async fn lookup_listing_rejects_malformed_and_invalid_signature_candidates() {
    let developer = Keys::generate();
    let mut invalid_signature = signed_listing(&developer, "game-1", 1_700_000_000);
    invalid_signature.sig = signed_listing(&developer, "other-game", 1_700_000_000).sig;
    assert!(invalid_signature.verify().is_err());
    let malformed = EventBuilder::new(Kind::Custom(30402), "")
        .tags(vec![Tag::identifier("game-1"), Tag::identifier("game-1")])
        .custom_created_at(Timestamp::from(1_700_000_000))
        .sign_with_keys(&developer)
        .expect("sign malformed listing");
    for candidate in [invalid_signature, malformed] {
        let relay = LocalRelay::run(vec![candidate]).await;
        let failed = LocalRelay::run_disconnect_before_eose(Vec::new()).await;
        let client = connect_client(&[relay.url.clone(), failed.url.clone()]).await;
        assert_eq!(
            client
                .lookup_listing(developer.public_key(), "game-1")
                .await,
            RelayLookup::Unavailable {
                successful_relays: 1,
                failed_relays: 1,
            }
        );
    }
}

#[tokio::test]
async fn lookup_listing_discards_valid_events_without_eose() {
    let developer = Keys::generate();
    let listing = signed_listing(&developer, "game-1", 1_700_000_000);
    for disconnect in [false, true] {
        let complete = LocalRelay::run(Vec::new()).await;
        let incomplete =
            LocalRelay::run_with_behavior(vec![listing.clone()], false, disconnect).await;
        let client = connect_client(&[complete.url.clone(), incomplete.url.clone()]).await;
        assert_eq!(
            client
                .lookup_listing(developer.public_key(), "game-1")
                .await,
            RelayLookup::Unavailable {
                successful_relays: 1,
                failed_relays: 1,
            }
        );
    }
}

#[tokio::test]
async fn lookup_listing_preserves_latest_and_tie_ordering_with_failed_relay() {
    let developer = Keys::generate();
    let older = signed_listing(&developer, "game-1", 1_700_000_000);
    let first = signed_listing_with_content(&developer, "game-1", 1_800_000_000, "first");
    let second = signed_listing_with_content(&developer, "game-1", 1_800_000_000, "second");
    let expected = if first.id < second.id {
        first.clone()
    } else {
        second.clone()
    };
    let events = vec![older, first, second];
    for reverse in [false, true] {
        let mut events = events.clone();
        if reverse {
            events.reverse();
        }
        let relay = LocalRelay::run(events).await;
        let failed = LocalRelay::run_disconnect_before_eose(Vec::new()).await;
        let client = connect_client(&[relay.url.clone(), failed.url.clone()]).await;
        assert_eq!(
            client
                .lookup_listing(developer.public_key(), "game-1")
                .await,
            RelayLookup::Found(expected.clone())
        );
    }
}

#[tokio::test]
async fn lookup_listing_confirms_absence_with_two_independent_completions() {
    let developer = Keys::generate();
    let relay_a = LocalRelay::run(Vec::new()).await;
    let relay_b = LocalRelay::run(Vec::new()).await;
    let relay_client = connect_client(&[relay_a.url.clone(), relay_b.url.clone()]).await;

    let lookup = relay_client
        .lookup_listing(developer.public_key(), "missing")
        .await;

    assert_eq!(
        lookup,
        RelayLookup::ConfirmedAbsent {
            successful_relays: 2,
        }
    );
}

#[tokio::test]
async fn one_completion_and_one_timeout_is_unavailable() {
    let relay_complete = LocalRelay::run(Vec::new()).await;
    let relay_timeout = LocalRelay::run_without_eose(Vec::new()).await;
    let relay_client =
        connect_client(&[relay_complete.url.clone(), relay_timeout.url.clone()]).await;

    let lookup = relay_client
        .lookup_listing(Keys::generate().public_key(), "missing")
        .await;

    assert_eq!(
        lookup,
        RelayLookup::Unavailable {
            successful_relays: 1,
            failed_relays: 1,
        }
    );
}

#[tokio::test]
async fn one_eose_and_one_disconnect_before_eose_is_unavailable() {
    let relay_complete = LocalRelay::run(Vec::new()).await;
    let relay_disconnect = LocalRelay::run_disconnect_before_eose(Vec::new()).await;
    let relay_client =
        connect_client(&[relay_complete.url.clone(), relay_disconnect.url.clone()]).await;

    let lookup = relay_client
        .lookup_listing(Keys::generate().public_key(), "missing")
        .await;

    assert_eq!(
        lookup,
        RelayLookup::Unavailable {
            successful_relays: 1,
            failed_relays: 1,
        }
    );
}

#[tokio::test]
async fn cancelling_lookup_unsubscribes_active_relay_query() {
    let relay = LocalRelay::run_without_eose(Vec::new()).await;
    let relay_client = connect_client(std::slice::from_ref(&relay.url)).await;
    let lookup_client = relay_client.clone();
    let lookup = tokio::spawn(async move {
        lookup_client
            .lookup_listing(Keys::generate().public_key(), "missing")
            .await
    });
    relay.wait_for_active_subscriptions(1).await;

    lookup.abort();
    let _ = lookup.await;

    relay.wait_for_active_subscriptions(0).await;
    drop(relay_client);
}

#[tokio::test]
async fn relay_exceeding_raw_event_count_never_counts_as_complete() {
    let developer = Keys::generate();
    let excessive_events = (0..=MAX_RELAY_QUERY_EVENTS)
        .map(|index| signed_listing(&developer, "game-1", index as u64 + 1))
        .collect();
    let relay_complete = LocalRelay::run(Vec::new()).await;
    let relay_overflow = LocalRelay::run(excessive_events).await;
    let relay_client =
        connect_client(&[relay_complete.url.clone(), relay_overflow.url.clone()]).await;

    let lookup = relay_client
        .lookup_listing(developer.public_key(), "game-1")
        .await;

    assert_eq!(
        lookup,
        RelayLookup::Unavailable {
            successful_relays: 1,
            failed_relays: 1,
        }
    );
}

#[tokio::test]
async fn relay_exceeding_raw_serialized_bytes_never_counts_as_complete() {
    let developer = Keys::generate();
    let content = "x".repeat(MAX_RELAY_QUERY_BYTES);
    let oversized = signed_listing_with_content(&developer, "game-1", 1, &content);
    let relay_complete = LocalRelay::run(Vec::new()).await;
    let relay_overflow = LocalRelay::run(vec![oversized]).await;
    let relay_client =
        connect_client(&[relay_complete.url.clone(), relay_overflow.url.clone()]).await;

    let lookup = relay_client
        .lookup_listing(developer.public_key(), "game-1")
        .await;

    assert_eq!(
        lookup,
        RelayLookup::Unavailable {
            successful_relays: 1,
            failed_relays: 1,
        }
    );
}

#[tokio::test]
async fn relay_timeouts_are_fanned_out_concurrently() {
    let relay_a = LocalRelay::run_without_eose(Vec::new()).await;
    let relay_b = LocalRelay::run_without_eose(Vec::new()).await;
    let relay_c = LocalRelay::run_without_eose(Vec::new()).await;
    let relay_client = connect_client(&[
        relay_a.url.clone(),
        relay_b.url.clone(),
        relay_c.url.clone(),
    ])
    .await;

    let lookup = tokio::time::timeout(
        Duration::from_millis(250),
        relay_client.lookup_listing(Keys::generate().public_key(), "missing"),
    )
    .await
    .expect("concurrent relay timeouts finish within a short aggregate bound");

    assert_eq!(
        lookup,
        RelayLookup::Unavailable {
            successful_relays: 0,
            failed_relays: 3,
        }
    );
}

#[tokio::test]
async fn lookup_listing_chooses_latest_valid_matching_event() {
    let developer = Keys::generate();
    let other_developer = Keys::generate();
    let older = signed_listing(&developer, "game-1", 1_700_000_000);
    let newer = signed_listing(&developer, "game-1", 1_800_000_000);
    let wrong_author = signed_listing(&other_developer, "game-1", 1_900_000_000);
    let wrong_d = signed_listing(&developer, "other-game", 2_000_000_000);
    let relay_a = LocalRelay::run(vec![older, wrong_author]).await;
    let relay_b = LocalRelay::run(vec![newer.clone(), wrong_d]).await;
    let relay_client = connect_client(&[relay_a.url.clone(), relay_b.url.clone()]).await;

    let lookup = relay_client
        .lookup_listing(developer.public_key(), "game-1")
        .await;

    assert_eq!(lookup, RelayLookup::Found(newer));
}

#[tokio::test]
async fn lookup_event_returns_exact_immutable_event_after_one_completion() {
    let event = signed_listing(&Keys::generate(), "game-1", 1_700_000_000);
    let relay = LocalRelay::run(vec![event.clone()]).await;
    let relay_failed = LocalRelay::run_disconnect_before_eose(Vec::new()).await;
    let relay_client = connect_client(&[relay.url.clone(), relay_failed.url.clone()]).await;

    let lookup = relay_client.lookup_event(event.id).await;

    assert_eq!(lookup, RelayLookup::Found(event));
}

#[tokio::test]
async fn duplicate_normalized_relay_urls_count_once() {
    let relay = LocalRelay::run(Vec::new()).await;
    let relay_client = connect_client(&[relay.url.clone(), format!("{}/", relay.url)]).await;

    let lookup = relay_client
        .lookup_listing(Keys::generate().public_key(), "missing")
        .await;

    assert_eq!(
        lookup,
        RelayLookup::Unavailable {
            successful_relays: 1,
            failed_relays: 0,
        }
    );
}

#[tokio::test]
async fn collection_lookup_deduplicates_and_sorts_by_event_id() {
    let buyer = Keys::generate();
    let coordinate = "30402:developer:game-1";
    let first = signed_receipt(&buyer, coordinate, 1_700_000_000);
    let second = signed_receipt(&buyer, coordinate, 1_800_000_000);
    let relay_a = LocalRelay::run(vec![second.clone(), first.clone()]).await;
    let relay_b = LocalRelay::run(vec![first.clone(), second.clone()]).await;
    let relay_client = connect_client(&[relay_a.url.clone(), relay_b.url.clone()]).await;

    let RelayLookup::Found(events) = relay_client
        .lookup_receipts(buyer.public_key(), coordinate)
        .await
    else {
        panic!("receipts should be found");
    };

    let mut expected = vec![first, second];
    expected.sort_by_key(|event| event.id);
    assert_eq!(events, expected);
}
