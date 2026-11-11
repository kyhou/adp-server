use base64::{engine::general_purpose::STANDARD, Engine};
use nostr::{Event, PublicKey, Timestamp};

use crate::error::AdpError;

const NIP98_KIND: u16 = 27235;
const MAX_CLOCK_SKEW_SECS: u64 = 60;

pub struct Nip98Verified {
    pub pubkey: PublicKey,
}

/// Decodes an `Authorization: Nostr <base64>` header value into the underlying
/// `kind:27235` event. Does not verify the event — call `verify_nip98` next.
pub fn decode_authorization_header(header_value: &str) -> Result<Event, AdpError> {
    let b64 = header_value
        .strip_prefix("Nostr ")
        .ok_or_else(|| AdpError::Nip98("Authorization header must start with 'Nostr '".into()))?;
    let json = STANDARD
        .decode(b64.trim())
        .map_err(|_| AdpError::Nip98("invalid base64 in Authorization header".into()))?;
    let event: Event = serde_json::from_slice(&json)
        .map_err(|_| AdpError::Nip98("invalid event JSON in Authorization header".into()))?;
    Ok(event)
}

/// Verifies a NIP-98 HTTP Auth token: signature, kind, clock skew, and that the
/// `u` (URL) and `method` tags match the current request exactly. The `u` tag MUST
/// match the full request path — a token minted for one endpoint MUST NOT be
/// accepted on another. See ADP-01.md "Security Considerations".
pub fn verify_nip98(
    token_event: &Event,
    expected_method: &str,
    expected_url: &str,
) -> Result<Nip98Verified, AdpError> {
    if token_event.verify().is_err() {
        return Err(AdpError::Nip98("invalid event signature".into()));
    }
    if token_event.kind.as_u16() != NIP98_KIND {
        return Err(AdpError::Nip98("expected kind:27235".into()));
    }

    let now = Timestamp::now().as_secs();
    let created_at = token_event.created_at.as_secs();
    let skew = now.abs_diff(created_at);
    if skew > MAX_CLOCK_SKEW_SECS {
        return Err(AdpError::Nip98(format!(
            "token timestamp outside allowed clock skew ({skew}s)"
        )));
    }

    let u_tag = token_event
        .tags
        .iter()
        .find_map(|t| {
            let v = t.clone().to_vec();
            (v.first().map(|k| k == "u").unwrap_or(false))
                .then(|| v.get(1).cloned())
                .flatten()
        })
        .ok_or_else(|| AdpError::Nip98("missing u tag".into()))?;

    if u_tag != expected_url {
        return Err(AdpError::Nip98(
            "u tag does not match the exact requested URL".into(),
        ));
    }

    let method_tag = token_event
        .tags
        .iter()
        .find_map(|t| {
            let v = t.clone().to_vec();
            (v.first().map(|k| k == "method").unwrap_or(false))
                .then(|| v.get(1).cloned())
                .flatten()
        })
        .ok_or_else(|| AdpError::Nip98("missing method tag".into()))?;

    if !method_tag.eq_ignore_ascii_case(expected_method) {
        return Err(AdpError::Nip98(
            "method tag does not match the request method".into(),
        ));
    }

    Ok(Nip98Verified {
        pubkey: token_event.pubkey,
    })
}
