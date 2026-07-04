use axum::extract::State;
use axum::Json;
use serde::Serialize;

use crate::state::AppState;

#[derive(Serialize)]
pub struct WellKnownResponse {
    adp_version: &'static str,
    pubkey: String,
    name: String,
    url: String,
}

/// Public, unauthenticated. See ADP-01.md `GET /.well-known/adp`.
///
/// NOTE: this reference implementation uses the fulfillment key's own pubkey as the
/// server's announced identity. A deployment that wants a distinct long-lived server
/// identity separate from a (potentially rotated) fulfillment key should introduce a
/// separate server identity keypair here instead.
pub async fn get_well_known(State(state): State<AppState>) -> Json<WellKnownResponse> {
    Json(WellKnownResponse {
        adp_version: "0.2.0",
        pubkey: state.fulfillment_keys.public_key().to_hex(),
        name: "ADP Reference Server".to_string(),
        url: state.config.server_url.clone(),
    })
}
