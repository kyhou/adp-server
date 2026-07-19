pub(crate) mod entitlement;
pub(crate) mod game;
mod health;
mod provision;
mod purchase;
mod upload;
mod well_known;

use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};
use axum::Router;
use tower_http::trace::TraceLayer;

use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health::health_handler))
        .route("/healthz", get(health::health_handler))
        .route("/.well-known/adp", get(well_known::get_well_known))
        .route("/provision", post(provision::provision_handler))
        .route("/provision/revoke", post(provision::revoke_handler))
        .route("/upload", post(upload::upload_handler))
        .route("/purchase/confirm", post(purchase::confirm_handler))
        .route("/entitlement/claim", post(entitlement::claim_handler))
        .route("/game/:game_coordinate", get(game::download_handler))
        .layer(DefaultBodyLimit::max(1024 * 1024 * 1024))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
