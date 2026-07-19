use axum::extract::State;
use axum::Json;
use serde::Serialize;

use adp_core::{ADP_VERSION, EXPERIMENTAL_ADP_CAMPAIGN_KIND, EXPERIMENTAL_ENTITLEMENT_GRANT_KIND};

use crate::state::AppState;

#[derive(Serialize)]
pub struct WellKnownResponse {
    adp_version: &'static str,
    pubkey: String,
    name: String,
    url: String,
    event_kinds: EventKinds,
    event_kind_status: &'static str,
}

#[derive(Serialize)]
struct EventKinds {
    entitlement_grant: u16,
    adp_campaign: u16,
}

/// Public, unauthenticated. See ADP-01.md `GET /.well-known/adp`.
///
pub async fn get_well_known(State(state): State<AppState>) -> Json<WellKnownResponse> {
    Json(WellKnownResponse {
        adp_version: ADP_VERSION,
        pubkey: state.operator_keys.public_key().to_hex(),
        name: "ADP Reference Server".to_string(),
        url: state.config.server_url.clone(),
        event_kinds: EventKinds {
            entitlement_grant: EXPERIMENTAL_ENTITLEMENT_GRANT_KIND,
            adp_campaign: EXPERIMENTAL_ADP_CAMPAIGN_KIND,
        },
        event_kind_status: "provisional",
    })
}
