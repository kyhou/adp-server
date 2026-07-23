use std::str::FromStr;

use lightning_invoice::Bolt11Invoice;
use nostr::{Event, PublicKey};
use sha2::{Digest, Sha256};

use crate::error::AdpError;

const ZAP_RECEIPT_KIND: u16 = 9735;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedPaymentProof {
    pub payment_hash: String,
    pub amount_msat: u64,
    pub settled_at: Option<u64>,
}

fn invoice_binding(invoice: &Bolt11Invoice) -> Result<(String, u64), AdpError> {
    let payment_hash: &[u8; 32] = invoice.payment_hash().as_ref();
    let amount_msat = invoice
        .amount_milli_satoshis()
        .ok_or_else(|| AdpError::InvalidPaymentProof("lightning invoice has no amount".into()))?;
    Ok((hex::encode(payment_hash), amount_msat))
}

pub fn verify_bolt11_preimage(
    bolt11: &str,
    preimage_hex: &str,
    expected_amount_msat: Option<u64>,
) -> Result<VerifiedPaymentProof, AdpError> {
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
    let (payment_hash, amount_msat) = invoice_binding(&invoice)?;
    if expected_amount_msat.is_some_and(|expected| amount_msat < expected) {
        return Err(AdpError::InvalidPaymentProof(
            "invoice amount is below the listing's declared price".into(),
        ));
    }
    Ok(VerifiedPaymentProof {
        payment_hash,
        amount_msat,
        settled_at: None,
    })
}

pub fn verify_zap_receipt(
    zap_receipt: &Event,
    buyer_pubkey_hex: &str,
    game_coordinate: &str,
    lsp_pubkey: &PublicKey,
) -> Result<VerifiedPaymentProof, AdpError> {
    zap_receipt
        .verify()
        .map_err(|_| AdpError::InvalidSignature)?;
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
    let has_buyer = zap_receipt.tags.iter().any(|tag| {
        let values = tag.clone().to_vec();
        values.len() >= 2 && values[0] == "P" && values[1] == buyer_pubkey_hex
    });
    if !has_buyer {
        return Err(AdpError::InvalidPaymentProof(
            "zap receipt does not reference the buyer's pubkey in a P tag".into(),
        ));
    }
    let has_coordinate = zap_receipt.tags.iter().any(|tag| {
        let values = tag.clone().to_vec();
        values.len() >= 2 && matches!(values[0].as_str(), "a" | "e") && values[1] == game_coordinate
    });
    if !has_coordinate {
        return Err(AdpError::InvalidPaymentProof(
            "zap receipt does not reference the purchased listing".into(),
        ));
    }
    let invoices = zap_receipt
        .tags
        .iter()
        .filter_map(|tag| {
            let values = tag.clone().to_vec();
            (values.len() == 2 && values[0] == "bolt11").then(|| values[1].clone())
        })
        .collect::<Vec<_>>();
    if invoices.len() != 1 {
        return Err(AdpError::InvalidPaymentProof(
            "zap receipt must contain exactly one bolt11 tag".into(),
        ));
    }
    let invoice = Bolt11Invoice::from_str(&invoices[0])
        .map_err(|e| AdpError::InvalidPaymentProof(format!("invalid zap bolt11 invoice: {e}")))?;
    let (payment_hash, amount_msat) = invoice_binding(&invoice)?;
    Ok(VerifiedPaymentProof {
        payment_hash,
        amount_msat,
        settled_at: Some(zap_receipt.created_at.as_secs()),
    })
}
