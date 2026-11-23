//! ADP-01 §Canonical Hex Encoding.
//!
//! Every hex value that ADP-01 defines (digests, x-only public keys, event IDs,
//! payment hashes) has exactly one valid spelling: lowercase. A value arriving
//! from another participant is accepted as canonical or rejected, never
//! normalized. `EventId::from_hex` and `PublicKey::from_hex` accept uppercase,
//! so ADP fields must be parsed through these helpers instead.
//!
//! The rule covers only fields defined as hexadecimal. UUIDs and opaque
//! identifiers (campaign, grant and authorization `d` tags, order IDs, scopes)
//! are not hex fields and must not be checked here.
use nostr::{EventId, PublicKey};

/// Length of a SHA-256 digest, event ID, or x-only public key in hex.
pub const HEX_32_BYTES: usize = 64;

/// Whether `value` is exactly `len` lowercase hexadecimal characters.
pub fn is_canonical_hex(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Whether `value` is a canonical SHA-256 digest (64 lowercase hex characters).
pub fn is_canonical_sha256(value: &str) -> bool {
    is_canonical_hex(value, HEX_32_BYTES)
}

/// Parses an event ID only from its canonical lowercase form.
pub fn canonical_event_id(value: &str) -> Option<EventId> {
    is_canonical_hex(value, HEX_32_BYTES)
        .then(|| EventId::from_hex(value).ok())
        .flatten()
}

/// Parses an x-only public key only from its canonical lowercase form.
pub fn canonical_public_key(value: &str) -> Option<PublicKey> {
    is_canonical_hex(value, HEX_32_BYTES)
        .then(|| PublicKey::from_hex(value).ok())
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "4bb06f8e4e3a7715d201d573d0aa423762e55dabd61a2c02278fa56cc6d294e0";

    #[test]
    fn lowercase_is_accepted() {
        assert!(is_canonical_sha256(ID));
        assert!(canonical_event_id(ID).is_some());
    }

    #[test]
    fn uppercase_and_mixed_case_are_rejected_not_normalized() {
        assert!(!is_canonical_sha256(&ID.to_uppercase()));
        assert!(canonical_event_id(&ID.to_uppercase()).is_none());
        let mixed = format!("{}{}", &ID[..63], "E");
        assert!(canonical_event_id(&mixed).is_none());
        // The library parser alone would have accepted it.
        assert!(EventId::from_hex(&ID.to_uppercase()).is_ok());
    }

    #[test]
    fn wrong_length_and_non_hex_are_rejected() {
        assert!(!is_canonical_sha256(&ID[..63]));
        assert!(!is_canonical_sha256(&format!("{ID}0")));
        assert!(!is_canonical_sha256(&format!("{}g", &ID[..63])));
        assert!(!is_canonical_sha256(&format!("0x{}", &ID[..62])));
    }

    #[test]
    fn a_uuid_is_not_a_hex_field() {
        // UUIDs are opaque identifiers; callers must not check them here.
        assert!(!is_canonical_sha256("9c2f1e4a-3b7d-4f21-9a6e-2d8c1f0a5b3e"));
    }
}
