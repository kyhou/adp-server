use bitcoin::hashes::{sha256, Hash};
use bitcoin::secp256k1::{Secp256k1, SecretKey};
use lightning_invoice::{Currency, InvoiceBuilder, PaymentSecret};
use nostr::{EventBuilder, Keys, Kind, Tag, Timestamp};

use adp_core::verify_zap_receipt;

fn tag(values: &[&str]) -> Tag {
    Tag::parse(values.iter().copied()).unwrap()
}

fn invoice() -> String {
    let payment_hash = sha256::Hash::hash(&[42u8; 32]);
    let private_key = SecretKey::from_slice(&[1u8; 32]).unwrap();
    let secp = Secp256k1::new();
    InvoiceBuilder::new(Currency::Bitcoin)
        .amount_milli_satoshis(1_000)
        .description("ADP proof".into())
        .payment_hash(payment_hash)
        .payment_secret(PaymentSecret([2u8; 32]))
        .duration_since_epoch(std::time::Duration::from_secs(100))
        .min_final_cltv_expiry_delta(144)
        .build_signed(|hash| secp.sign_ecdsa_recoverable(hash, &private_key))
        .unwrap()
        .to_string()
}

#[test]
fn zap_validation_accepts_uppercase_buyer_binding_and_public_invoice() {
    let lsp = Keys::generate();
    let buyer = Keys::generate();
    let coordinate = format!("30402:{}:game", Keys::generate().public_key());
    let zap = EventBuilder::new(Kind::Custom(9735), "")
        .tags(vec![
            tag(&["P", &buyer.public_key().to_hex()]),
            tag(&["a", &coordinate]),
            tag(&["bolt11", &invoice()]),
        ])
        .custom_created_at(Timestamp::from_secs(100))
        .sign_with_keys(&lsp)
        .unwrap();
    let binding = verify_zap_receipt(
        &zap,
        &buyer.public_key().to_hex(),
        &coordinate,
        &lsp.public_key(),
    )
    .unwrap();
    assert_eq!(binding.amount_msat, 1_000);
}

#[test]
fn zap_validation_rejects_lsp_mismatch_before_accepting_proof() {
    let actual_lsp = Keys::generate();
    let declared_lsp = Keys::generate();
    let buyer = Keys::generate();
    let coordinate = format!("30402:{}:game", Keys::generate().public_key());
    let zap = EventBuilder::new(Kind::Custom(9735), "")
        .tags(vec![
            tag(&["P", &buyer.public_key().to_hex()]),
            tag(&["a", &coordinate]),
            tag(&["bolt11", &invoice()]),
        ])
        .sign_with_keys(&actual_lsp)
        .unwrap();
    assert!(verify_zap_receipt(
        &zap,
        &buyer.public_key().to_hex(),
        &coordinate,
        &declared_lsp.public_key(),
    )
    .is_err());
}
