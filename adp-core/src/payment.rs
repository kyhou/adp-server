use std::str::FromStr;

use lightning_invoice::Bolt11Invoice;
use nostr::{Event, PublicKey};
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
/// Also requires the zap receipt signer to match the listing's resolved LSP
/// `nostrPubkey` per NIP-57 / ADP-01 LSP verification.
pub fn verify_zap_receipt(
    zap_receipt: &Event,
    buyer_pubkey_hex: &str,
    game_coordinate: &str,
    lsp_pubkey: &PublicKey,
) -> Result<(), AdpError> {
    if zap_receipt.verify().is_err() {
        return Err(AdpError::InvalidSignature);
    }
    if zap_receipt.kind.as_u16() != ZAP_RECEIPT_KIND {
        return Err(AdpError::InvalidPaymentProof(
            "expected kind:9735 zap receipt".into(),
        ));
    }
    if &zap_receipt.pubkey != lsp_pubkey {
        return Err(AdpError::InvalidPaymentProof(
            "zap receipt was not signed by the listing's resolved LSP".into(),
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
