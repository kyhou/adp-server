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

/// Verifies current-listing authority while issuing a new receipt or accepting an upload.
///
/// `listing` MUST be fetched fresh from relays (not a local cache). This helper is not
/// portable historical verification: ownership checks must resolve the receipt chain and
/// its immutable kind 30406 authorization root with `resolve_receipt_chain`.
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
    let listing_created_at = listing.event.created_at.as_secs();
    let mut revocation_hit: Option<(u64, u64)> = None;

    for delegation in &listing.fulfillment_delegations {
        if delegation.pubkey != receipt.pubkey {
            continue;
        }
        if delegation.authorizes_at(receipt_created_at, listing_created_at) {
            return Ok(SignerAuthorization::Delegated {
                fulfillment_pubkey: delegation.pubkey,
                valid_from: delegation.valid_from,
            });
        }
        if delegation.valid_from > receipt_created_at {
            continue;
        }
        if let Some(revoked_at) = delegation.revoked_at {
            let effective_revoked_at = revoked_at.max(listing_created_at);
            if effective_revoked_at <= receipt_created_at {
                // Same key, but this specific interval had already ended by the time
                // the receipt claims to have been signed. Keep looking — an earlier
                // tag with a different interval for the same pubkey could still match
                // (e.g. re-delegated after a temporary revocation) — but remember this
                // as a candidate error if nothing else matches.
                revocation_hit = Some((effective_revoked_at, receipt_created_at));
                continue;
            }
        }
    }

    if let Some((revoked_at, created_at)) = revocation_hit {
        return Err(AdpError::RevokedAtSigningTime {
            revoked_at,
            created_at,
        });
    }

    Err(AdpError::UnauthorizedSigner(receipt.pubkey.to_hex()))
}
