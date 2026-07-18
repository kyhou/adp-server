# adp-server

A reference implementation of **ADP-01: Arcadestr Distribution Protocol** — see
`ADP-01.md` for the protocol spec this code implements.

Two-crate workspace, matching the pattern already used elsewhere in this project:

- **`adp-core`** — protocol logic only. No HTTP, no SQL. Listing policy, campaign,
  authorization lifecycle, Entitlement Grant, NIP-102 receipt, NIP-98 auth, and
  payment-proof parsing and verification
  (bolt11+preimage and partial zap receipt validation). Fully unit-testable without
  a network or a database — see `adp-core/tests/`.
- **`adp-server`** — the HTTP binary. axum routes, SQLite persistence (sqlx), and a
  thin `nostr-sdk` relay client wrapper. Wires `adp-core`'s pure functions into the
  endpoints defined by the spec.

## Build status

This repository is compiler-checked against the locked Rust dependencies in
`Cargo.lock`, including `nostr`/`nostr-sdk` 0.44. The current verification gate is:

```bash
cargo check -p adp-core --all-targets
cargo check -p adp-server --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Use that sequence before pushing behavior changes. `adp-core` has pure protocol
regression coverage; `adp-server` currently has a small route-level unit test plus
compile coverage for the HTTP, SQL, and relay integration surface.

## Known simplifications (intentionally out of scope for this draft)

- **Download token → buyer resolution requires the token to already be scoped to
  a `game_coordinate`** (`resolve_download_token`). This matches the spec's
  Path A (the token was issued for a specific coordinate), so it's not a gap
  against ADP-01 itself — just noting the schema choice.
- **Multi-tenant fulfillment custody**: hosted operators provision distinct
  fulfillment keys per `(developer, scope)` via `POST /provision`; private keys are
  encrypted at rest with `ADP_MASTER_KEY`. This env-var master key is a
  reference-implementation baseline, not a full production custody design. An
  operator running at real scale should evaluate an HSM or KMS.
- **Local LNURL testing**: `ADP_ALLOW_INSECURE_LNURL=true` permits `http://`
  LNURL resolution for loopback domains only (`localhost`, `127.0.0.1`, `::1`);
  non-loopback domains always use `https://`.

## Running

```bash
cp .env.example .env
# edit .env — ADP_OPERATOR_NSEC and ADP_MASTER_KEY are required. Generate a
# dedicated operator key with `nak key generate` or any nostr key tool. Generate
# ADP_MASTER_KEY as 32 random bytes encoded in hex.

set -a; source .env; set +a
cargo run -p adp-server
```

The SQLite DB and uploaded files are created under `./data/` (gitignored).

## Endpoints

| Method | Path | Auth | Spec section |
|---|---|---|---|
| `GET`  | `/.well-known/adp` | none | ADP-01.md §`GET /.well-known/adp` |
| `POST` | `/provision` | NIP-98 (developer) | §`POST /provision` |
| `POST` | `/provision/revoke` | NIP-98 (developer) | §`POST /provision/revoke` |
| `POST` | `/upload` | NIP-98 (developer or delegated fulfillment key) | §`POST /upload` |
| `POST` | `/purchase/confirm` | NIP-98 (buyer) | §`POST /purchase/confirm` |
| `POST` | `/entitlement/claim` | NIP-98 (buyer) | Free-acquisition amendment v3 |
| `GET`  | `/game/{game_coordinate}` | NIP-98 or `?token=` | §`GET /game/:game_coordinate` |

## Design notes worth knowing before extending this

- **Ownership and distribution authorization are checked independently** in
  `routes/game.rs` — a valid receipt alone is never sufficient to serve a file;
  the server must also be named in the listing's `server` tags. See ADP-01.md
  "Trust Model" for why these are deliberately not conflated.
- **Delegation freshness**: `relay.rs::fetch_listing` is called fresh on every
  request that needs authorization-sensitive listing state. Caller-supplied listing
  events identify the coordinate, but fresh relay state is used for delegation and
  distribution checks. If you add a cache later for performance, do not use it for
  delegation checks without re-reading ADP-01.md's freshness requirement first.
- **Status chain traversal** (`adp_core::latest_status_in_chain`) walks `e`-tag
  references rather than trusting `created_at` ordering, per NIP-102's
  append-only chain model.
- **Idempotency / replay protection** live at the database layer (`UNIQUE`
  constraints on `order_id` and `payment_proof_event_id` in
  `migrations/0001_init.sql`), not just in application logic, so a bug in the
  handler can't silently double-issue a receipt for the same order.
- **Explicit acquisition only**: a zero price never authorizes a download.
  `acquisition: public` and active half-open `timed-access` are the only ungated
  listing policies. Durable grants and receipts take precedence over current policy.
- **Provisional event kinds**: Entitlement Grants use development kind `1030` and
  ADP campaigns use `1031`. `/.well-known/adp` advertises both as provisional.
- **Authority separation**: only the publisher controls campaigns and revokes
  grants. A currently listed fulfillment key with an active publisher-signed
  kind `30406` authorization lifecycle may issue grants and receipts.
- **Claim idempotency is server-local**: one stored grant is reused for a repeated
  `(buyer, game coordinate, campaign)` claim; independent servers may issue their
  own valid grants.
