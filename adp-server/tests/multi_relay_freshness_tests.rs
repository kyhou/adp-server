use std::sync::Arc;

use adp_server::relay::{Relay, RelayClient};
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
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind local relay");
        let addr = listener.local_addr().expect("local relay addr");
        let events = Arc::new(Mutex::new(events));
        let task = tokio::spawn(run_relay(listener, events));

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

async fn run_relay(listener: TcpListener, events: Arc<Mutex<Vec<Event>>>) {
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
                        let eose = json!(["EOSE", subscription_id]);
                        if socket
                            .send(Message::Text(eose.to_string().into()))
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                    "EVENT" => {
                        let Some(event_value) = parts.get(1) else {
                            continue;
                        };
                        let Ok(event) = serde_json::from_value::<Event>(event_value.clone())
                        else {
                            continue;
                        };
                        let event_id = event.id.to_hex();
                        events.lock().await.push(event);
                        let ok = json!(["OK", event_id, true, ""]);
                        if socket.send(Message::Text(ok.to_string().into())).await.is_err() {
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
