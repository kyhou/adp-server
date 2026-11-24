# ADP-01 implementation gaps (adp-server)

**Status:** tracked. Close an item only when code changes **and** a targeted test
passes. Synchronizing `ADP-01.md` resolves nothing here.

This record compares the reference server with the consolidated ADP-01 (`0.3.0`).
Client-side gaps are tracked in the Arcadestr repository's
`docs/protocol-implementation-gaps.md`.

## Open

None. Every tracked server gap is closed below.

## Closed

| # | Resolution | Tests |
|---|---|---|
| S1 | `GET /game` serves anonymous requests only when the current signed listing is `public`, or `timed-access` and active now. Distribution authorization and stored-file integrity still run first for every basis. Credential relays are not consulted for anonymous requests. | `download_authorization_tests.rs`: `anonymous_download_is_allowed_by_explicit_public_policy`, `…_during_active_timed_access`, `…_refused_after_timed_access_ends`, `…_refused_before_timed_access_starts`, `…_of_a_zero_price_gated_listing_is_refused`, `…_with_malformed_policy_is_refused`, `anonymous_public_access_still_requires_distribution_authorization` (451), `anonymous_public_access_still_verifies_the_stored_file` (500; also fails if the hash check is removed) |
| S2 | An invalid or expired download token no longer returns `403`: it is treated as no credential, so a request with no other authentication gets `401`, and public policy still applies. An invalid NIP-98 token returns `401` and is never downgraded to anonymous. | `an_invalid_download_token_without_authentication_returns_401`, `an_invalid_download_token_does_not_block_public_access`, `an_invalid_nip98_token_is_rejected_even_when_policy_is_public` |
| S3 | `/purchase/confirm` keys replay protection on the canonical lowercase `payment_hash` for every proof format. It reserves the signed receipt and key in one transaction **before** publishing, so concurrent submissions cannot both issue a receipt. A reservation is never released. If publication fails, the process stops, or the response is lost, a retry by the **same buyer for the same coordinate** re-publishes the exact stored receipt (same event ID) and gets a fresh token. A different buyer gets `409`. **Spec clarification suggested:** ADP-01 §Replay protection says reuse is rejected "even for a different buyer"; returning the original receipt to its own buyer follows NIP-102 idempotency and ADP-01's "publication retries reuse that exact root", and issues no new credential. | `route_integration_tests.rs`: `purchase_replay_is_keyed_by_payment_hash_across_buyers`, `concurrent_submissions_of_one_payment_publish_one_receipt`, `concurrent_retries_by_one_buyer_yield_one_receipt`, `a_retry_after_failed_publication_republishes_the_same_receipt`, `a_retry_after_a_lost_response_returns_the_same_receipt`, `a_reservation_left_by_an_interrupted_request_is_recoverable` |
| S4 | `disputed` receipt tips retain ownership; `refunded` does not. | `gate7_delegated_portable_receipts_preserve_history_authority_and_status` (`disputed` case) |
| S5 | A developer-signed grant without `source_event` is verified by `adp_core::verify_adp_developer_grant`: developer signature, no `authorization`, developer-only status updates. Delegated grants still require a campaign. | `adp-core/tests/entitlement_tests.rs` (five campaignless tests); `download_authorization_tests.rs`: `developer_grant_without_campaign_permits_download`, `non_developer_grant_without_campaign_is_denied` |
| S6 | Failed `kind:30404` publication in `/provision` and `/provision/revoke` returns `503`. | `failed_attestation_publication_returns_503` |
| S7 | An authenticated uploader without `upload_build` authority gets `403`, and nothing is stored. | `an_uploader_without_upload_authority_gets_403` |
| S10 | New `adp_core::canonical_hex` helpers reject anything but lowercase. They are used for every ADP-defined hex input: listing `file_hash`, `fulfillment_authorization`, `adp_terms`, and `campaign`; grant `source_event` and `authorization`; receipt `authorization` and zap proof ID; `fulfillment_pubkey` in `kind:30404`/`30406`; and the claim `campaign_event_id` and revoke `fulfillment_pubkey` request bodies. Non-canonical listing values become malformed entries; a non-canonical `file_hash` is reported by upload as `400`. UUID and opaque identifiers (grant/campaign/authorization `d`, `order`, `scope`) are deliberately not checked. NIP-01's own `p`/`e` tags and the coordinate pubkey are outside this rule. | `adp-core/tests/canonical_hex_tests.rs` (9 tests, each with a lowercase control), `canonical_hex` unit tests, `required_file_hash_rejects_a_non_canonical_hash`, `an_uppercase_campaign_event_id_is_rejected`, `revoke_rejects_an_uppercase_fulfillment_pubkey`, `upload_rejects_a_listing_with_an_uppercase_file_hash`. All fail if the helper accepts uppercase. |
| S8 | Optional, **disabled by default** (`ADP_PUBLISHER_RELAYS=enabled`). Listing reads also query each publisher's NIP-65 write relays under [the outbound relay policy](outbound-relay-policy.md): `wss` only, public addresses only, one DNS resolution with the connection pinned to the validated IP while TLS verifies the hostname, no redirects, bounded attempts and pool size, no reconnection. Publisher relays add candidate events but never count toward the S9 freshness policy, and their failures are ignored. | `src/publisher_relays.rs` unit tests (11); `multi_relay_freshness_tests.rs` (9 publisher-relay tests over real TLS). Mutations caught: accepting non-public resolved addresses, resolving twice before dialing, counting publisher relays toward freshness. |
| S9 | Documented server freshness policy (`FRESHNESS_MIN_COMPLETED_RELAYS` in `adp-server/src/relay.rs`): every configured relay is queried concurrently; the newest valid listing among completed responses is used only if at least 2 configured relays completed; otherwise `503`. No listing cache exists. `Relay::lookup_current_listing` applies it and is used by every freshness-sensitive route (`/purchase/confirm`, `/entitlement/claim`, `/upload`, `GET /game`, settlement quote creation). `lookup_listing` keeps the existence rule: one valid listing from one relay. **Behavior change:** these routes previously accepted a listing when only one relay answered; they now return `503`. | `multi_relay_freshness_tests.rs`: `one_completed_relay_establishes_existence_but_not_freshness`, `two_completed_relays_give_the_newest_current_listing`; `relay::freshness_policy_tests` (3); `route_integration_tests.rs`: `freshness_sensitive_routes_return_503_without_current_listing_evidence` (fails if routes use the existence lookup) |
| S11 | Receipt status updates signed by a fulfillment key are no longer chain members (ADP-01 defines no status-update capability), so they cannot change the resolved status. Developer-signed transitions are unchanged. Found during this work; it was not on the original list. | `adp-core/tests/receipt_authorization_tests.rs`: `a_fulfillment_key_status_update_does_not_alter_the_resolved_status`; `delegated-refund` case in the gate7 download test |

Every test in the Closed table was also run against the previous code, where it fails (except the boundary assertions noted).

## Verified conformant (no change needed)

- **Terminal states:** campaign chains skip every successor once cancelled (`adp-core/src/campaign.rs`). Grant chains admit only `granted → revoked` (`entitlement.rs`). `kind:30406` admits one cancellation and nothing after it (`provisioning.rs`).
- **Acquisition tags:** duplicate, unknown-mode, malformed, and `starts >= ends` produce `AcquisitionPolicy::Invalid`, which grants no access without invalidating the listing. Covered by `adp-core/tests/listing_regression_tests.rs`.

## Outside ADP-01

- The `/settlement/*` endpoints, `/health`, and `/healthz` are implementation-specific and are not ADP-01 endpoints.
- `already_claimed` in `/entitlement/claim` responses is an implementation extension.
- Release builds reject standalone BOLT11/preimage evidence for legacy sales (see `README.md`). That is a deployment policy stricter than ADP-01, not a conformance gap.
