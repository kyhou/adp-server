use lightning_invoice::{Currency, InvoiceBuilder, PaymentSecret};
use rand::RngCore;
use secp256k1::hashes::{sha256, Hash as _};
use secp256k1::{Secp256k1, SecretKey};
use sha2::{Digest, Sha256};
use std::time::Duration;

fn main() {
    let mut preimage = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut preimage);
    let payment_hash = Sha256::digest(preimage);
    let payment_hash = sha256::Hash::from_slice(&payment_hash).unwrap();

    let secp = Secp256k1::new();
    let mut sk_bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut sk_bytes);
    let secret_key = SecretKey::from_slice(&sk_bytes).unwrap();

    let invoice = InvoiceBuilder::new(Currency::Bitcoin)
        .description("adp-server test invoice".to_string())
        .payment_hash(payment_hash.into())
        .payment_secret(PaymentSecret([0u8; 32]))
        .amount_milli_satoshis(1_000_000)
        .duration_since_epoch(Duration::from_secs(1_700_000_000))
        .min_final_cltv_expiry_delta(144)
        .build_signed(|hash| secp.sign_ecdsa_recoverable(hash, &secret_key))
        .unwrap();

    println!("bolt11: {invoice}");
    println!("preimage: {}", hex::encode(preimage));
}
