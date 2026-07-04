mod config;
mod error;
mod relay;
mod routes;
mod state;
mod storage;

use nostr_sdk::Keys;
use tracing_subscriber::EnvFilter;

use crate::config::Config;
use crate::relay::RelayClient;
use crate::state::AppState;
use crate::storage::Storage;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config = Config::from_env();

    tokio::fs::create_dir_all(&config.files_dir).await?;
    if let Some(parent) = std::path::Path::new(&config.db_path).parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let fulfillment_keys = Keys::parse(&config.fulfillment_nsec)
        .map_err(|e| anyhow::anyhow!("failed to parse ADP_FULFILLMENT_NSEC: {e}"))?;
    tracing::info!(
        pubkey = %fulfillment_keys.public_key().to_hex(),
        "loaded fulfillment key — this MUST be declared in a fulfillment_pubkey tag \
         on any listing this server is meant to fulfill purchases for"
    );

    let storage = Storage::connect(&config.db_path).await?;
    let relay = RelayClient::connect(
        fulfillment_keys.clone(),
        &config.relays,
        config.server_url.clone(),
    )
    .await?;

    tracing::info!(relays = ?config.relays, "connected to relays");

    let bind_addr = config.bind_addr.clone();

    let state = AppState {
        config: std::sync::Arc::new(config),
        storage,
        relay: std::sync::Arc::new(relay),
        fulfillment_keys,
    };

    let app = routes::router(state);

    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    tracing::info!(addr = %bind_addr, "ADP server listening");
    axum::serve(listener, app).await?;

    Ok(())
}
