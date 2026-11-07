use axum::extract::State;
use axum::Json;
use serde::Serialize;

use adp_core::{ADP_VERSION, EXPERIMENTAL_ADP_CAMPAIGN_KIND, EXPERIMENTAL_ENTITLEMENT_GRANT_KIND};

use crate::state::AppState;

#[derive(Serialize)]
struct SettlementDiscovery {
    version: u16,
    split_enabled: bool,
    terms_event_id: Option<String>,
    fee: adp_core::FeePolicy,
    legacy_paid_sales: crate::config::LegacyPaidSalesPolicy,
}

#[derive(Serialize)]
pub struct WellKnownResponse {
    adp_version: &'static str,
    pubkey: String,
    name: String,
    url: String,
    event_kinds: EventKinds,
    event_kind_status: &'static str,
    max_upload_bytes: u64,
    settlement: SettlementDiscovery,
}

#[derive(Serialize)]
struct EventKinds {
    entitlement_grant: u16,
    adp_campaign: u16,
    commercial_terms: u16,
    private_quote: u16,
    recovery_evidence: u16,
}

/// Public, unauthenticated. See ADP-01.md `GET /.well-known/adp`.
///
pub async fn get_well_known(State(state): State<AppState>) -> Json<WellKnownResponse> {
    let settlement = crate::config::SettlementConfig::from_env();
    Json(WellKnownResponse {
        adp_version: ADP_VERSION,
        pubkey: state.operator_keys.public_key().to_hex(),
        name: "ADP Reference Server".to_string(),
        url: state.config.server_url.clone(),
        event_kinds: EventKinds {
            entitlement_grant: EXPERIMENTAL_ENTITLEMENT_GRANT_KIND,
            adp_campaign: EXPERIMENTAL_ADP_CAMPAIGN_KIND,
            commercial_terms: adp_core::TERMS_KIND,
            private_quote: adp_core::QUOTE_KIND,
            recovery_evidence: adp_core::RECOVERY_KIND,
        },
        event_kind_status: "provisional",
        max_upload_bytes: state.config.max_upload_bytes,
        settlement: SettlementDiscovery {
            version: settlement.version,
            split_enabled: settlement.split_enabled(),
            terms_event_id: settlement.terms_event_id,
            fee: settlement.fee,
            legacy_paid_sales: settlement.legacy_paid_sales,
        },
    })
}
