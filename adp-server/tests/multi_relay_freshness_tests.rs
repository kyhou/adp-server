use std::sync::Arc;

use adp_server::relay::{Relay, RelayClient, RelayLookup};
use futures_util::{SinkExt, StreamExt};
use nostr_sdk::{Event, EventBuilder, Keys, Kind, Tag, Timestamp};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_tungstenite::{accept_async, tungstenite::Message};

struct LocalRelay {
    url: String,
    task: JoinHandle<()>,
}

impl LocalRelay {
    async fn run(events: Vec<Event>) -> Self {
        Self::run_with_eose(events, true).await
    }

    async fn run_without_eose(events: Vec<Event>) -> Self {
        Self::run_with_eose(events, false).await
    }

    async fn run_with_eose(events: Vec<Event>, send_eose: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind local relay");
        let addr = listener.local_addr().expect("local relay addr");
        let events = Arc::new(Mutex::new(events));
        let task = tokio::spawn(run_relay(listener, events, send_eose));

        Self {
            url: format!("ws://{addr}"),
            task,
        }
    }
}

impl Drop for LocalRelay {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn run_relay(listener: TcpListener, events: Arc<Mutex<Vec<Event>>>, send_eose: bool) {
    while let Ok((stream, _)) = listener.accept().await {
        let events = events.clone();
        tokio::spawn(async move {
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
                        let Some(subscription_id) = parts.get(1).and_then(|part| part.as_str())
                        else {
                            continue;
                        };
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
                    "CLOSE" => {}
                    _ => {}
                }
            }
        });
    }
}

fn signed_listing(developer: &Keys, d_tag: &str, created_at: u64) -> Event {
    EventBuilder::new(Kind::Custom(30402), "")
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

#[tokio::test]
async fn sanity_check_connect_tolerates_dead_relay() {
    let result = RelayClient::connect(
        Keys::generate(),
        &["ws://127.0.0.1:1".to_string()],
        "http://localhost:8080".to_string(),
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

    let relay_client = RelayClient::connect(
        Keys::generate(),
        &[relay_a.url.clone(), relay_b.url.clone()],
        "http://localhost:8080".to_string(),
    )
    .await
    .expect("connect relay client");

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
async fn fetch_listing_succeeds_when_one_configured_relay_is_unreachable() {
    let developer = Keys::generate();
    let listing = signed_listing(&developer, "game-1", 1_700_000_000);
    let relay_a = LocalRelay::run(vec![listing]).await;

    let relay_client = RelayClient::connect(
        Keys::generate(),
        &[relay_a.url.clone(), "ws://127.0.0.1:1".to_string()],
        "http://localhost:8080".to_string(),
    )
    .await
    .expect("connect relay client");

    let fetched = relay_client
        .fetch_listing(developer.public_key(), "game-1")
        .await
        .expect("fetch listing");

    assert!(
        fetched.is_some(),
        "should still find the listing via the one live relay"
    );
}

#[tokio::test]
async fn lookup_listing_requires_two_successful_relay_completions() {
    let developer = Keys::generate();
    let listing = signed_listing(&developer, "game-1", 1_700_000_000);
    let relay = LocalRelay::run(vec![listing]).await;
    let relay_client = RelayClient::connect(
        Keys::generate(),
        &[relay.url.clone(), "ws://127.0.0.1:1".to_string()],
        "http://localhost:8080".to_string(),
    )
    .await
    .expect("connect relay client");

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
async fn lookup_listing_confirms_absence_with_two_independent_completions() {
    let developer = Keys::generate();
    let relay_a = LocalRelay::run(Vec::new()).await;
    let relay_b = LocalRelay::run(Vec::new()).await;
    let relay_client = RelayClient::connect(
        Keys::generate(),
        &[relay_a.url.clone(), relay_b.url.clone()],
        "http://localhost:8080".to_string(),
    )
    .await
    .expect("connect relay client");

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
    let relay_client = RelayClient::connect(
        Keys::generate(),
        &[relay_complete.url.clone(), relay_timeout.url.clone()],
        "http://localhost:8080".to_string(),
    )
    .await
    .expect("connect relay client");

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
async fn relay_timeouts_are_fanned_out_concurrently() {
    let relay_a = LocalRelay::run_without_eose(Vec::new()).await;
    let relay_b = LocalRelay::run_without_eose(Vec::new()).await;
    let relay_client = RelayClient::connect(
        Keys::generate(),
        &[relay_a.url.clone(), relay_b.url.clone()],
        "http://localhost:8080".to_string(),
    )
    .await
    .expect("connect relay client");
    let started = tokio::time::Instant::now();

    let lookup = relay_client
        .lookup_listing(Keys::generate().public_key(), "missing")
        .await;

    assert_eq!(
        lookup,
        RelayLookup::Unavailable {
            successful_relays: 0,
            failed_relays: 2,
        }
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(15),
        "two 10-second timeouts must not run sequentially"
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
    let relay_client = RelayClient::connect(
        Keys::generate(),
        &[relay_a.url.clone(), relay_b.url.clone()],
        "http://localhost:8080".to_string(),
    )
    .await
    .expect("connect relay client");

    let lookup = relay_client
        .lookup_listing(developer.public_key(), "game-1")
        .await;

    assert_eq!(lookup, RelayLookup::Found(newer));
}

#[tokio::test]
async fn lookup_event_returns_exact_immutable_event_after_one_completion() {
    let event = signed_listing(&Keys::generate(), "game-1", 1_700_000_000);
    let relay = LocalRelay::run(vec![event.clone()]).await;
    let relay_client = RelayClient::connect(
        Keys::generate(),
        &[relay.url.clone(), "ws://127.0.0.1:1".to_string()],
        "http://localhost:8080".to_string(),
    )
    .await
    .expect("connect relay client");

    let lookup = relay_client.lookup_event(event.id).await;

    assert_eq!(lookup, RelayLookup::Found(event));
}

#[tokio::test]
async fn duplicate_normalized_relay_urls_count_once() {
    let relay = LocalRelay::run(Vec::new()).await;
    let relay_client = RelayClient::connect(
        Keys::generate(),
        &[relay.url.clone(), format!("{}/", relay.url)],
        "http://localhost:8080".to_string(),
    )
    .await
    .expect("connect relay client");

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
    let relay_client = RelayClient::connect(
        Keys::generate(),
        &[relay_a.url.clone(), relay_b.url.clone()],
        "http://localhost:8080".to_string(),
    )
    .await
    .expect("connect relay client");

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
