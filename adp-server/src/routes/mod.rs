pub(crate) mod entitlement;
pub(crate) mod game;
mod health;
mod provision;
mod purchase;
mod settlement;
mod upload;
mod well_known;

use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};
use axum::Router;
use tower_http::trace::TraceLayer;

use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    router_with_settlement_clock(state, crate::settlement_clock::SettlementClock::default())
}

/// In-process injection only, used by isolated regtest scenarios.
pub fn router_with_settlement_clock(
    state: AppState,
    clock: crate::settlement_clock::SettlementClock,
) -> Router {
    Router::new()
        .route("/health", get(health::health_handler))
        .route("/healthz", get(health::health_handler))
        .route("/.well-known/adp", get(well_known::get_well_known))
        .route("/provision", post(provision::provision_handler))
        .route("/provision/revoke", post(provision::revoke_handler))
        .route(
            "/upload",
            post(upload::upload_handler).layer(DefaultBodyLimit::max(
                usize::try_from(
                    state
                        .config
                        .max_upload_bytes
                        .saturating_add(2 * 1024 * 1024),
                )
                .unwrap_or(usize::MAX),
            )),
        )
        .route("/purchase/confirm", post(purchase::confirm_handler))
        .route("/settlement/quote", post(settlement::create_quote_handler))
        .route("/settlement/:quote_id", get(settlement::status_handler))
        .route(
            "/settlement/:quote_id/status",
            post(settlement::status_post_handler),
        )
        .route(
            "/settlement/:quote_id/next",
            post(settlement::next_invoice_handler),
        )
        .route(
            "/settlement/:quote_id/proof",
            post(settlement::proof_handler),
        )
        .route(
            "/settlement/:quote_id/refund-destination",
            post(settlement::replace_refund_destination_handler),
        )
        .route(
            "/settlement/:quote_id/recovery",
            post(settlement::prepare_recovery_handler),
        )
        .route(
            "/settlement/:quote_id/recovery/submit",
            post(settlement::submit_recovery_handler),
        )
        .route(
            "/settlement/:quote_id/refund/:leg_id/prepare",
            post(settlement::prepare_refund_handler),
        )
        .route(
            "/settlement/:quote_id/refund/:leg_id/confirm",
            post(settlement::confirm_refund_handler),
        )
        .route("/entitlement/claim", post(entitlement::claim_handler))
        .route("/game/:game_coordinate", get(game::download_handler))
        .layer(DefaultBodyLimit::max(1024 * 1024 * 1024))
        .layer(TraceLayer::new_for_http())
        .layer(axum::Extension(clock))
        .with_state(state)
}
