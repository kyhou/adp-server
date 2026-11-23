# ADP-01 implementation gaps (adp-server)

**Status:** tracked. Close an item only when code changes **and** a targeted test
passes. Synchronizing `ADP-01.md` resolves nothing here.

This record compares the reference server with the consolidated ADP-01 (`0.3.0`).
Client-side gaps are tracked in the Arcadestr repository's
`docs/protocol-implementation-gaps.md`.

## Open

| # | ADP-01 requirement | Current server behavior | Evidence |
|---|---|---|---|
| S1 | Public and active timed-access downloads are anonymous; a server MUST NOT require authentication for them (§`GET /game/:game_coordinate`). | Without `?token=`, the handler requires a NIP-98 header and returns `401` before evaluating public/timed policy. | `adp-server/src/routes/game.rs` (missing `Authorization` → `Unauthorized`) |
| S2 | Denial returns `401` when no valid token or NIP-98 was presented (§`GET /game`, §HTTP errors). | An invalid or expired `?token=` returns `403`. | `routes/game.rs` (`resolve_download_token` → `Forbidden`) |
| S3 | The replay identifier is the canonical `payment_hash` across all proof formats, enforced atomically (§Replay protection). | Replay keys are the zap receipt event ID or the raw preimage, so the same payment submitted in two formats has two different keys. Storage uses `UNIQUE(payment_proof_event_id)`. | `routes/purchase.rs` (`replay_keys`), `migrations/0001_init.sql` |
| S4 | `paid`, `fulfilled`, and `disputed` tips retain ownership; `refunded` does not (§Receipt status under ADP). | Ownership accepts only `paid` or `fulfilled`; `disputed` is denied. | `adp-server/src/ownership.rs` (tip status check) |
| S5 | Developer-signed grants may omit `source_event` (§Entitlement Grants). | ADP grant verification requires a campaign source and rejects its absence (`MissingCampaignSource`). | `adp-core/src/entitlement_adp.rs` |
| S6 | Failed relay publication of a required event returns `503` (§HTTP errors, `/provision`, `/provision/revoke`). | Provisioning errors map to `BadRequest`, `Forbidden`, or `Internal` (`500`); there is no `503` path. | `routes/provision.rs` |
| S7 | An uploader without authority receives `403` (§`POST /upload`). | The upload handler has no `Forbidden` path. Unauthorized uploaders surface through `Unauthorized` (`401`) or `BadRequest` (`400`). | `routes/upload.rs` |
| S8 | Implementations SHOULD prioritize the publisher's NIP-65 write relays (§Relay selection). This is an optimization, never an authorization requirement. | No NIP-65 (`kind:10002`) lookup exists; only configured relays are used. | No match for `10002`/NIP-65 in `adp-core` or `adp-server` |
| S9 | A documented freshness policy for freshness-sensitive operations; existence MUST NOT require two relays (§Relay availability). | `MIN_SUCCESSFUL_RELAYS = 2` governs listing usability and absence but is not documented as a freshness policy. Single-relay positive evidence for non-freshness-sensitive lookups still needs re-verification. | `adp-server/src/relay.rs` |
| S10 | Consumers MUST reject non-lowercase hex rather than normalizing it (§Canonical Hex Encoding). | Hex fields are parsed with `EventId::from_hex` / `PublicKey::from_hex`, which use `hex::decode_to_slice` and accept uppercase. `file_hash` is compared byte-for-byte, so uppercase fails closed, but it is not explicitly rejected as non-canonical. The `payment_hash` replay key is now enforced as lowercase (see S3). | `nostr-0.44` `EventId::from_hex`; `adp-core/src/listing.rs` (`file_hash` stored unvalidated) |

## Closed

None yet.

## Verified conformant (no change needed)

- **Terminal states:** campaign chains skip every successor once cancelled (`adp-core/src/campaign.rs`). Grant chains admit only `granted → revoked` (`entitlement.rs`). `kind:30406` admits one cancellation and nothing after it (`provisioning.rs`).
- **Acquisition tags:** duplicate, unknown-mode, malformed, and `starts >= ends` produce `AcquisitionPolicy::Invalid`, which grants no access without invalidating the listing. Covered by `adp-core/tests/listing_regression_tests.rs`.

## Outside ADP-01

- The `/settlement/*` endpoints, `/health`, and `/healthz` are implementation-specific and are not ADP-01 endpoints.
- `already_claimed` in `/entitlement/claim` responses is an implementation extension.
- Release builds reject standalone BOLT11/preimage evidence for legacy sales (see `README.md`). That is a deployment policy stricter than ADP-01, not a conformance gap.
