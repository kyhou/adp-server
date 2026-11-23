# ADP-01 implementation gaps (adp-server)

**Status:** open. Synchronizing `ADP-01.md` changed documentation only and does
**not** resolve any item here. Close an item only when code and tests change, and
link the change.

This record compares the reference server with the consolidated ADP-01 (`0.3.0`).
Each item was checked against the source cited when ADP-01 was synchronized.
Client-side gaps are tracked in the Arcadestr repository's
`docs/protocol-implementation-gaps.md`.

## Verified discrepancies

| # | ADP-01 requirement | Current server behavior | Evidence |
|---|---|---|---|
| S1 | Public and active timed-access downloads are anonymous; a server MUST NOT require authentication for them (§`GET /game/:game_coordinate`). | Without `?token=`, the handler requires a NIP-98 header and returns `401` before evaluating public/timed policy. | `adp-server/src/routes/game.rs` (missing `Authorization` → `Unauthorized`) |
| S2 | Denial returns `401` when no valid token or NIP-98 was presented (§`GET /game`, §HTTP errors). | An invalid or expired `?token=` returns `403`. | `routes/game.rs` (`resolve_download_token` → `Forbidden`) |
| S3 | The replay identifier is the canonical `payment_hash` across all proof formats, enforced atomically (§Replay protection). | Replay keys are the zap receipt event ID or the raw preimage, so the same payment submitted in two formats has two different keys. Storage uses `UNIQUE(payment_proof_event_id)`. | `routes/purchase.rs` (`replay_keys`), `migrations/0001_init.sql` |
| S4 | `paid`, `fulfilled`, and `disputed` tips retain ownership; `refunded` does not (§Receipt status under ADP). | Ownership accepts only `paid` or `fulfilled`; `disputed` is denied. | `adp-server/src/ownership.rs` (tip status check) |
| S5 | Developer-signed grants may omit `source_event` (§Entitlement Grants). | ADP grant verification requires a campaign source and rejects its absence (`MissingCampaignSource`). | `adp-core/src/entitlement_adp.rs` |
| S6 | Failed relay publication of a required event returns `503` (§HTTP errors, `/provision`, `/provision/revoke`). | Provisioning errors map to `BadRequest`, `Forbidden`, or `Internal` (`500`); there is no `503` path. | `routes/provision.rs` |
| S7 | An uploader without authority receives `403` (§`POST /upload`). | The upload handler has no `Forbidden` path. Unauthorized uploaders surface through `Unauthorized` (`401`) or `BadRequest` (`400`). | `routes/upload.rs` |
| S8 | Implementations SHOULD prioritize the publisher's NIP-65 write relays (§Relay selection). | No NIP-65 (`kind:10002`) lookup exists; only configured relays are used. | No match for `10002`/NIP-65 in `adp-core` or `adp-server` |
| S9 | A freshness policy for freshness-sensitive operations MUST be documented; existence MUST NOT require two relays (§Relay availability). | `MIN_SUCCESSFUL_RELAYS = 2` governs listing usability and absence, but is not documented as the server's freshness policy. Whether single-relay positive evidence is accepted for non-freshness-sensitive lookups needs re-verification: `docs/proposals/positive-listing-evidence.md` and the 2026-07-17 design doc describe different behavior. | `adp-server/src/relay.rs` |

## Not yet verified

These need targeted inspection and tests before being classified:

- `kind:30406` cancellation, campaign cancellation, and grant revocation rejected as terminal (any successor invalid).
- Malformed, duplicate, or unknown `acquisition` tags granting no access without invalidating the listing. The design doc describes an `Invalid` policy; confirm it matches ADP-01.
- Exact `valid_from` activation with no skew allowance. No skew constant was found in `adp-core`; confirm in the server's authorization paths.
- Canonical-hex rejection on every ADP-defined hex input, with UUID and opaque identifiers left unconstrained.
- The full status-code mapping of each endpoint against ADP-01 §HTTP errors.

## Outside ADP-01

- The `/settlement/*` endpoints, `/health`, and `/healthz` are implementation-specific and are not ADP-01 endpoints.
- The `already_claimed` field in `/entitlement/claim` responses is an implementation extension.
- Release builds reject standalone BOLT11/preimage evidence for legacy sales (see `README.md`). That is a deployment policy stricter than ADP-01; it is not a conformance gap.
