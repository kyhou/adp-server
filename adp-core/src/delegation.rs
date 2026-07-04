use nostr::{Event, PublicKey};

use crate::error::AdpError;
use crate::listing::AdpListing;

/// How a receipt's signer was authorized to sign it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignerAuthorization {
    /// Signed directly by the listing's own (developer) key.
    DirectDeveloper,
    /// Signed by a key the listing delegated via an active `fulfillment_pubkey` tag.
    Delegated {
        fulfillment_pubkey: PublicKey,
        valid_from: u64,
    },
}

/// Verifies that `receipt.pubkey` was authorized to sign `receipt` at `receipt.created_at`,
/// per ADP-01's extension to NIP-102 Proof of Ownership condition #1.
///
/// `listing` MUST be fetched fresh from relays (not a local cache) before calling this
/// for any security-relevant decision (e.g. unlocking a download) — a stale listing may
/// not yet reflect a revocation. See ADP-01.md "Fulfillment Key Delegation" §freshness.
///
/// This function does NOT verify `receipt`'s own signature — callers must do that
/// separately (e.g. `receipt.verify()`) before relying on `receipt.pubkey`.
pub fn verify_signer_authorization(
    receipt: &Event,
    listing: &AdpListing,
) -> Result<SignerAuthorization, AdpError> {
    if receipt.pubkey == listing.developer_pubkey {
        return Ok(SignerAuthorization::DirectDeveloper);
    }

    let receipt_created_at = receipt.created_at.as_secs();
    let mut revocation_hit: Option<(u64, u64)> = None;

    for delegation in &listing.fulfillment_delegations {
        if delegation.pubkey != receipt.pubkey {
            continue;
        }
        if delegation.valid_from > receipt_created_at {
            continue;
        }
        if let Some(revoked_at) = delegation.revoked_at {
            if revoked_at <= receipt_created_at {
                // Same key, but this specific interval had already ended by the time
                // the receipt claims to have been signed. Keep looking — an earlier
                // tag with a different interval for the same pubkey could still match
                // (e.g. re-delegated after a temporary revocation) — but remember this
                // as a candidate error if nothing else matches.
                revocation_hit = Some((revoked_at, receipt_created_at));
                continue;
            }
        }
        return Ok(SignerAuthorization::Delegated {
            fulfillment_pubkey: delegation.pubkey,
            valid_from: delegation.valid_from,
        });
    }

    if let Some((revoked_at, created_at)) = revocation_hit {
        return Err(AdpError::RevokedAtSigningTime {
            revoked_at,
            created_at,
        });
    }

    Err(AdpError::UnauthorizedSigner(receipt.pubkey.to_hex()))
}
