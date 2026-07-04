# adp-server

A reference implementation of **ADP-01: Arcadestr Distribution Protocol** — see
`ADP-01.md` for the protocol spec this code implements.

Two-crate workspace, matching the pattern already used elsewhere in this project:

- **`adp-core`** — protocol logic only. No HTTP, no SQL. Listing tag parsing,
  fulfillment key delegation and its validity-interval check, NIP-102 receipt
  construction/parsing, NIP-98 auth verification, and payment proof verification
  (bolt11+preimage and partial zap receipt validation). Fully unit-testable without
  a network or a database — see `adp-core/tests/`.
- **`adp-server`** — the HTTP binary. axum routes, SQLite persistence (sqlx), and a
  thin `nostr-sdk` relay client wrapper. Wires `adp-core`'s pure functions into the
  four endpoints defined by the spec.

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

- **NIP-57 LSP verification**: `verify_zap_receipt` checks the receipt's own
  signature and its `P`/`a` tag references, but does not resolve the developer's
  `lud16`/`lud06` to confirm the zap receipt's signing pubkey matches the LNURL
  provider's declared `nostrPubkey`. Flagged inline in `adp-core/src/payment.rs`.
- **Listing price resolution**: `/purchase/confirm` hardcodes `amount: "0"`,
  `currency: "SATS"` on the receipt rather than reading the listing's NIP-99 price
  tags. Marked with a `NOTE` in `adp-server/src/routes/purchase.rs`.
- **Download token → buyer resolution requires the token to already be scoped to
  a `game_coordinate`** (`resolve_download_token`). This matches the spec's
  Path A (the token was issued for a specific coordinate), so it's not a gap
  against ADP-01 itself — just noting the schema choice.
- **Server identity**: `GET /.well-known/adp` announces the fulfillment key's own
  pubkey as the server's identity. A deployment wanting a server identity distinct
  from a (rotatable) fulfillment key should add a separate identity keypair.

## Running

```bash
cp .env.example .env
# edit .env — ADP_FULFILLMENT_NSEC is required; generate a DEDICATED key,
# e.g. via `nak key generate` or any nostr key tool. Never reuse a developer's
# main identity key here — see ADP-01.md "Security Considerations".

set -a; source .env; set +a
cargo run -p adp-server
```

The SQLite DB and uploaded files are created under `./data/` (gitignored).

## Endpoints

| Method | Path | Auth | Spec section |
|---|---|---|---|
| `GET`  | `/.well-known/adp` | none | ADP-01.md §`GET /.well-known/adp` |
| `POST` | `/upload` | NIP-98 (developer or delegated fulfillment key) | §`POST /upload` |
| `POST` | `/purchase/confirm` | NIP-98 (buyer) | §`POST /purchase/confirm` |
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
