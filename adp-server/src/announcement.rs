use nostr_sdk::{Event, EventBuilder, Keys, Kind, Tag};

pub fn build_server_announcement_event(
    operator_keys: &Keys,
    slug: &str,
    name: &str,
    url: &str,
    contact: &str,
) -> Result<Event, String> {
    let tags = vec![
        Tag::parse(vec!["d", slug]).map_err(|e| e.to_string())?,
        Tag::parse(vec!["name", name]).map_err(|e| e.to_string())?,
        Tag::parse(vec!["url", url]).map_err(|e| e.to_string())?,
        Tag::parse(vec!["supported_adp", "0.2.0"]).map_err(|e| e.to_string())?,
        Tag::parse(vec!["contact", contact]).map_err(|e| e.to_string())?,
    ];
    EventBuilder::new(Kind::Custom(30403), "")
        .tags(tags)
        .sign_with_keys(operator_keys)
        .map_err(|e| e.to_string())
}
