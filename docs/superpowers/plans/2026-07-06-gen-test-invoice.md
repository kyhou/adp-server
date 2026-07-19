# Test Invoice Example Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an `adp-server` example binary that generates a synthetic BOLT11 invoice and matching preimage.

**Architecture:** Keep the utility as a Cargo example under `adp-server/examples/`. The example is self-contained and depends only on invoice construction, random byte generation, hashing, signing, and hex encoding.

**Tech Stack:** Rust 2021, Cargo examples, `lightning-invoice 0.32`, `sha2 0.10`, `hex 0.4`, `rand`, `secp256k1`.

---

### Task 1: Add And Verify Example Binary

**Files:**
- Create: `adp-server/examples/gen_test_invoice.rs`
- Modify: `adp-server/Cargo.toml`

- [ ] **Step 1: Run the failing command**

Run: `cargo run -p adp-server --example gen_test_invoice`

Expected: FAIL because Cargo cannot find the `gen_test_invoice` example.

- [ ] **Step 2: Add example-only dependencies**

Add these dev-dependencies to `adp-server/Cargo.toml`:

```toml
rand = "0.8"
secp256k1 = "0.29"
```

- [ ] **Step 3: Add the example source**

Create `adp-server/examples/gen_test_invoice.rs` with code that:

```rust
use lightning_invoice::{Currency, InvoiceBuilder, PaymentSecret};
use rand::RngCore;
use secp256k1::{Secp256k1, SecretKey};
use sha2::{Digest, Sha256};

fn main() {
    let mut preimage = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut preimage);
    let payment_hash = Sha256::digest(preimage);

    let secp = Secp256k1::new();
    let mut sk_bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut sk_bytes);
    let secret_key = SecretKey::from_slice(&sk_bytes).unwrap();

    let invoice = InvoiceBuilder::new(Currency::Bitcoin)
        .description("adp-server test invoice".to_string())
        .payment_hash(payment_hash.into())
        .payment_secret(PaymentSecret([0u8; 32]))
        .amount_milli_satoshis(1_000_000)
        .current_timestamp()
        .min_final_cltv_expiry_delta(144)
        .build_signed(|hash| secp.sign_ecdsa_recoverable(hash, &secret_key))
        .unwrap();

    println!("bolt11: {invoice}");
    println!("preimage: {}", hex::encode(preimage));
}
```

- [ ] **Step 4: Run the example**

Run: `cargo run -p adp-server --example gen_test_invoice`
Expected: PASS and print `bolt11:` plus `preimage:`.

- [ ] **Step 5: Fix API mismatches if needed**

If compilation fails due to `lightning-invoice` or `secp256k1` API differences, adjust only the example/dependency versions needed to compile and rerun Step 4.
