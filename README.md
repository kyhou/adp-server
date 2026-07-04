# adp-server

A reference implementation of **ADP-01: Arcadestr Distribution Protocol** — see
`ADP-01.md` for the protocol spec this code implements.

Two-crate workspace, matching the pattern already used elsewhere in this project:

- **`adp-core`** — protocol logic only. No HTTP, no SQL. Listing tag parsing,
  fulfillment key delegation and its validity-interval check, NIP-102 receipt
  construction/parsing, NIP-98 auth verification, and payment proof verification
  (bolt11+preimage and zap receipts). Fully unit-testable without a network or a
  database — see `adp-core/tests/delegation_tests.rs`.
- **`adp-server`** — the HTTP binary. axum routes, SQLite persistence (sqlx), and a
  thin `nostr-sdk` relay client wrapper. Wires `adp-core`'s pure functions into the
  four endpoints defined by the spec.

## ⚠️ Compilation status — please read

This project was written in a sandboxed environment **without a Rust toolchain
available** (no `rustc`/`cargo`, and no access to `rustup`'s install domain from
that sandbox). Every file here was written directly against the documented
`nostr`/`nostr-sdk` 0.44 API surface, but **none of it has been run through
`cargo check`**. Treat this as a structurally complete first draft, not a verified
build.

The parts most likely to need small adjustments once you run `cargo check` locally:

- **Tag construction/inspection** (`adp-core/src/*.rs`): built on `Tag::parse(Vec<&str>)`
  and `Tag::as_vec()`, which are the most version-stable, least typed parts of the
  `nostr` tag API — chosen deliberately to minimize API-drift risk, but not
  compiler-verified here.
- **`nostr::nips::nip44::encrypt`** signature in `adp-core/src/receipt.rs`
  (`encrypt_receipt_content`) — double-check `Keys::secret_key()`'s exact return
  type (`&SecretKey` vs `Result<&SecretKey, _>`) against your locked `nostr` version.
- **`Filter` builder methods** in `adp-server/src/relay.rs` (`.identifier()`,
  `.pubkey()`, `.custom_tag(SingleLetterTag::lowercase(Alphabet::A), ...)`) — the
  general shape is right, exact method names may have shifted between 0.44.x patches.
- **`EventBuilder::new(kind, content).tags(tags)`** builder chain and
  `.custom_created_at(...)` (used only in tests, to pin a receipt's `created_at` for
  delegation-interval assertions).

None of this affects the *design* — only exact call sites. Run:

```bash
cargo check --workspace
cargo clippy --workspace -- -D warnings
cargo test -p adp-core
```

and fix what the compiler flags. Given this codebase's existing workflow (opencode
implementing against real compiler feedback, reviewed gate-by-gate), that's the
natural next step rather than something to work around by hand.

## Known simplifications (intentionally out of scope for this draft)

- **NIP-57 LSP verification**: `verify_zap_receipt` checks the receipt's own
  signature and its `p`/`a` tag references, but does not resolve the developer's
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
| `GET`  | `/game/:game_coordinate` | NIP-98 or `?token=` | §`GET /game/:game_coordinate` |

## Design notes worth knowing before extending this

- **Ownership and distribution authorization are checked independently** in
  `routes/game.rs` — a valid receipt alone is never sufficient to serve a file;
  the server must also be named in the listing's `server` tags. See ADP-01.md
  "Trust Model" for why these are deliberately not conflated.
- **Delegation freshness**: `relay.rs::fetch_listing` is called fresh on every
  request that needs it — nothing in this server caches listings. If you add a
  cache later for performance, do not use it for delegation checks without
  re-reading ADP-01.md's freshness requirement first.
- **Status chain traversal** (`adp_core::latest_status_in_chain`) walks `e`-tag
  references rather than trusting `created_at` ordering, per NIP-102's
  append-only chain model.
- **Idempotency / replay protection** live at the database layer (`UNIQUE`
  constraints on `order_id` and `payment_proof_event_id` in
  `migrations/0001_init.sql`), not just in application logic, so a bug in the
  handler can't silently double-issue a receipt for the same order.
