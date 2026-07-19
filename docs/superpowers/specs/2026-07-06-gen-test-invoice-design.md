# Test Invoice Example Design

## Goal

Add a small `adp-server` Cargo example that prints a synthetic BOLT11 invoice and its matching 32-byte preimage for local testing of `verify_bolt11_preimage`.

## Approach

The example lives at `adp-server/examples/gen_test_invoice.rs` so it runs with `cargo run -p adp-server --example gen_test_invoice`. It generates a random preimage, hashes it with SHA-256 for the invoice payment hash, signs the invoice with a randomly generated secp256k1 key, and prints both values.

## Dependencies

The example reuses existing workspace dependencies for `lightning-invoice`, `sha2`, and `hex`. `adp-server` needs example-only access to `rand` and `secp256k1`, so they are added as dev-dependencies.

## Verification

Run `cargo run -p adp-server --example gen_test_invoice` and confirm it prints `bolt11:` and `preimage:` lines.
