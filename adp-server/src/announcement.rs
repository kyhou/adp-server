use nostr_sdk::{Event, EventBuilder, Keys, Kind, Tag};

use adp_core::ADP_VERSION;

pub fn build_server_announcement_event(
    operator_keys: &Keys,
    slug: &str,
    name: &str,
    url: &str,
    contact: &str,
) -> Result<Event, String> {
    let settlement = crate::config::SettlementConfig::from_env();
    let mut tags = vec![
        Tag::parse(vec!["d", slug]).map_err(|e| e.to_string())?,
        Tag::parse(vec!["name", name]).map_err(|e| e.to_string())?,
        Tag::parse(vec!["url", url]).map_err(|e| e.to_string())?,
        Tag::parse(vec!["supported_adp", ADP_VERSION]).map_err(|e| e.to_string())?,
        Tag::parse(vec!["contact", contact]).map_err(|e| e.to_string())?,
        Tag::parse(vec![
            "settlement",
            "1",
            if settlement.split_enabled() {
                "split"
            } else {
                "legacy-only"
            },
        ])
        .map_err(|e| e.to_string())?,
        Tag::parse(vec!["experimental_kinds", "1032", "1033", "1034"])
            .map_err(|e| e.to_string())?,
        Tag::parse(vec![
            "legacy_paid_sales",
            match settlement.legacy_paid_sales {
                crate::config::LegacyPaidSalesPolicy::AcceptFeeFree => "accept_fee_free",
                crate::config::LegacyPaidSalesPolicy::Decline => "decline",
            },
        ])
        .map_err(|e| e.to_string())?,
    ];
    if let Some(terms_event_id) = settlement.terms_event_id {
        tags.push(Tag::parse(vec!["terms", &terms_event_id]).map_err(|e| e.to_string())?);
    }
    EventBuilder::new(Kind::Custom(30403), "")
        .tags(tags)
        .sign_with_keys(operator_keys)
        .map_err(|e| e.to_string())
}
