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

pub mod campaign;
pub mod chain;
pub mod delegation;
pub mod entitlement;
pub mod entitlement_adp;
pub mod error;
pub mod kinds;
pub mod listing;
pub mod nip98;
pub mod payment;
pub mod provisioning;
pub mod receipt;

pub use campaign::{
    CampaignEvent, CampaignMode, CampaignStatus, CampaignTerms, CampaignTransition,
    ResolvedCampaign,
};
pub use chain::ChainResolutionError;
pub use delegation::{verify_signer_authorization, SignerAuthorization};
pub use entitlement::{GrantStatus, ParsedEntitlementGrant, ResolvedEntitlementGrant};
pub use error::AdpError;
pub use kinds::{ADP_VERSION, EXPERIMENTAL_ADP_CAMPAIGN_KIND, EXPERIMENTAL_ENTITLEMENT_GRANT_KIND};
pub use listing::{AcquisitionPolicy, AdpListing, CampaignPointer, FulfillmentDelegation};
pub use nip98::{decode_authorization_header, verify_nip98, Nip98Verified};
pub use payment::{verify_bolt11_preimage, verify_zap_receipt};
pub use provisioning::{
    build_acceptance_event_builder, build_attestation_event_builder,
    build_authorization_revocation_event_builder, build_authorization_root_event_builder,
    parse_acceptance_event, parse_attestation_event, parse_authorization_event,
    resolve_authorization, AuthorizationEvent, AuthorizationTerms, AuthorizationTransition,
    ParsedAcceptance, ParsedAttestation, ResolvedAuthorization, ACCEPTANCE_KIND, ATTESTATION_KIND,
};
pub use receipt::{
    build_receipt_event_builder, encrypt_receipt_content, latest_status_in_chain, new_order_id,
    parse_receipt_event, BuildReceiptParams, ParsedReceipt, ReceiptContent, ReceiptItem,
    RECEIPT_KIND,
};
