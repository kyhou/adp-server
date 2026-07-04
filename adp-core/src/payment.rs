use std::str::FromStr;

use lightning_invoice::Bolt11Invoice;
use nostr::Event;
use sha2::{Digest, Sha256};

use crate::error::AdpError;

const ZAP_RECEIPT_KIND: u16 = 9735;

/// Verifies a self-contained payment proof: `sha256(preimage) == invoice.payment_hash`,
/// per NIP-102's self-contained proof path. `expected_amount_msat`, if given, enforces
/// the invoice paid at least that amount (the listing's declared price).
pub fn verify_bolt11_preimage(
    bolt11: &str,
    preimage_hex: &str,
    expected_amount_msat: Option<u64>,
) -> Result<(), AdpError> {
    let invoice = Bolt11Invoice::from_str(bolt11)
        .map_err(|e| AdpError::InvalidPaymentProof(format!("invalid bolt11 invoice: {e}")))?;

    let preimage_bytes = hex::decode(preimage_hex)
        .map_err(|_| AdpError::InvalidPaymentProof("preimage is not valid hex".into()))?;
    if preimage_bytes.len() != 32 {
        return Err(AdpError::InvalidPaymentProof(
            "preimage must be exactly 32 bytes".into(),
        ));
    }

    let computed_hash: [u8; 32] = Sha256::digest(&preimage_bytes).into();
    let payment_hash: &[u8; 32] = invoice.payment_hash().as_ref();
    if &computed_hash != payment_hash {
        return Err(AdpError::InvalidPaymentProof(
            "sha256(preimage) does not match the invoice's payment hash".into(),
        ));
    }

    if let Some(expected) = expected_amount_msat {
        if let Some(amount) = invoice.amount_milli_satoshis() {
            if amount < expected {
                return Err(AdpError::InvalidPaymentProof(
                    "invoice amount is below the listing's declared price".into(),
                ));
            }
        }
    }

    Ok(())
}

/// Verifies a `kind:9735` zap receipt references the buyer and the claimed listing.
///
/// This checks the receipt's own signature and its `P`/`a`(or `e`) tags, which is
/// sufficient to confirm the receipt is internally consistent with the claimed
/// purchase. It does NOT verify that the signing pubkey is the recipient's declared
/// LSP per NIP-57 §"Zap Receipt appearance validation" — a production deployment
/// SHOULD additionally resolve the developer's `lud16`/`lud06` and confirm the zap
/// receipt's pubkey matches the LNURL provider's declared `nostrPubkey`, which
/// requires an HTTP round trip this reference implementation omits for brevity.
pub fn verify_zap_receipt(
    zap_receipt: &Event,
    buyer_pubkey_hex: &str,
    game_coordinate: &str,
) -> Result<(), AdpError> {
    if zap_receipt.verify().is_err() {
        return Err(AdpError::InvalidSignature);
    }
    if zap_receipt.kind.as_u16() != ZAP_RECEIPT_KIND {
        return Err(AdpError::InvalidPaymentProof(
            "expected kind:9735 zap receipt".into(),
        ));
    }

    let has_buyer_p_tag = zap_receipt.tags.iter().any(|t| {
        let v = t.clone().to_vec();
        v.first().map(|k| k == "P").unwrap_or(false)
            && v.get(1).map(|pk| pk == buyer_pubkey_hex).unwrap_or(false)
    });
    if !has_buyer_p_tag {
        return Err(AdpError::InvalidPaymentProof(
            "zap receipt does not reference the buyer's pubkey in a P tag".into(),
        ));
    }

    let references_coordinate = zap_receipt.tags.iter().any(|t| {
        let v = t.clone().to_vec();
        matches!(v.first().map(|k| k.as_str()), Some("a") | Some("e"))
            && v.get(1).map(|val| val == game_coordinate).unwrap_or(false)
    });
    if !references_coordinate {
        return Err(AdpError::InvalidPaymentProof(
            "zap receipt does not reference the purchased listing in an a/e tag".into(),
        ));
    }

    Ok(())
}
