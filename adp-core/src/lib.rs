//! adp-core: protocol logic for ADP-01 (Arcadestr Distribution Protocol).
//!
//! This crate is transport-agnostic — it has no knowledge of HTTP, axum, or SQLite.
//! It implements:
//!   - parsing ADP's `kind:30402` listing tag extensions (`listing`)
//!   - fulfillment key delegation and its validity-interval check (`delegation`)
//!   - NIP-102 `kind:1020` receipt construction and verification (`receipt`)
//!   - NIP-98 HTTP Auth token verification (`nip98`)
//!   - payment proof verification for zap receipts and bolt11/preimage pairs (`payment`)
//!
//! See `ADP-01.md` (the protocol spec) for the normative behavior this crate implements.

pub mod delegation;
pub mod error;
pub mod listing;
pub mod nip98;
pub mod payment;
pub mod provisioning;
pub mod receipt;

pub use delegation::{verify_signer_authorization, SignerAuthorization};
pub use error::AdpError;
pub use listing::{AdpListing, FulfillmentDelegation};
pub use nip98::{decode_authorization_header, verify_nip98, Nip98Verified};
pub use payment::{verify_bolt11_preimage, verify_zap_receipt};
pub use provisioning::{
    build_acceptance_event_builder, build_attestation_event_builder, parse_acceptance_event,
    parse_attestation_event, ParsedAcceptance, ParsedAttestation, ACCEPTANCE_KIND,
    ATTESTATION_KIND,
};
pub use receipt::{
    build_receipt_event_builder, encrypt_receipt_content, latest_status_in_chain, new_order_id,
    parse_receipt_event, BuildReceiptParams, ParsedReceipt, ReceiptContent, ReceiptItem,
    RECEIPT_KIND,
};
