use adp_server::announcement::build_server_announcement_event;
use adp_server::config::Config;
use adp_server::integrity::spawn_file_hash_reverification;
use adp_server::keystore::KeyStore;
use adp_server::relay::{Relay, RelayClient};
use adp_server::routes;
use adp_server::state::AppState;
use adp_server::storage::Storage;
use nostr_sdk::Keys;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config = Config::from_env();
    if config.relays.len() < 2 {
        anyhow::bail!("ADP_RELAYS must configure at least 2 relays for ADP-01 listing freshness");
    }

    tokio::fs::create_dir_all(&config.files_dir).await?;
    if let Some(parent) = std::path::Path::new(&config.db_path).parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let operator_keys = Keys::parse(&config.operator_nsec)
        .map_err(|e| anyhow::anyhow!("failed to parse ADP_OPERATOR_NSEC: {e}"))?;
    tracing::info!(
        pubkey = %operator_keys.public_key().to_hex(),
        "loaded operator identity key for announcements and provisioning attestations"
    );

    let storage = Storage::connect(&config.db_path).await?;
    let keystore = KeyStore::new(storage.pool(), config.master_key);
    let relay = RelayClient::connect(
        operator_keys.clone(),
        &config.relays,
        config.server_url.clone(),
    )
    .await?;

    tracing::info!(relays = ?config.relays, "connected to relays");

    let announcement = build_server_announcement_event(
        &operator_keys,
        "adp-reference-server",
        "ADP Reference Server",
        &config.server_url,
        "",
    )
    .map_err(|e| anyhow::anyhow!("failed to build server announcement: {e}"))?;
    relay
        .publish(&announcement)
        .await
        .map_err(|e| anyhow::anyhow!("failed to publish server announcement: {e}"))?;
    tracing::info!(event_id = %announcement.id.to_hex(), "published server announcement");

    let bind_addr = config.bind_addr.clone();
    let reverify_interval = std::time::Duration::from_secs(config.file_reverify_interval_secs);

    let state = AppState {
        config: std::sync::Arc::new(config),
        storage,
        relay: std::sync::Arc::new(relay),
        keystore: std::sync::Arc::new(keystore),
        operator_keys,
    };

    spawn_file_hash_reverification(state.storage.clone(), reverify_interval);

    let app = routes::router(state);

    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    tracing::info!(addr = %bind_addr, "ADP server listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("shutdown signal received");
}
