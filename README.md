# adp-server

A reference implementation of **ADP-01: Arcadestr Distribution Protocol** — see
`ADP-01.md` for the protocol spec this code implements.

See [docs/README.md](docs/README.md) for documentation status, examples, and what
belongs in version control.

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

Keep operator credentials outside the checkout and any sync folder. Use an
owner-only directory (`0700`) and a private environment file (`0600`), for
example `~/.config/adp-server/server.env` if that directory is not synchronized.
For a new instance, use `.env.example` as a template; do not replace an existing
instance's keys or database paths. Both the server and `adp-refunds` read their
environment rather than discovering an `.env` file.

```bash
# From the repository directory, for a trusted, shell-compatible file only:
(
  set -a
  . "$HOME/.config/adp-server/server.env" || exit 1
  set +a
  exec cargo run -p adp-server
)
```

With a service manager, load its supported environment-file format instead of
sourcing it in a shell. Give `adp-refunds` the intended server origin and
matching operator key; it does not need the database master key.

The SQLite DB and uploaded files are created under `./data/` (gitignored).

### If credentials were synchronized

On each Syncthing device, check the actual folder root and add an anchored
ignore for the secret path to that root's `.stignore`. For a `~/Sync` root
containing this checkout at `Projetos/adp-server`, use:

```text
/Projetos/adp-server/.env
!/Projetos/adp-server/.env.example
/Projetos/adp-server/.env.*
```

The exception is only for the committed, secret-free template. Ignore rules
are local, not shared by Syncthing; an ignore does not erase existing copies.
Inventory the file and variants on the receiving devices, in Syncthing version
stores, and in backups without copying their contents to logs or reports. Do
not remove or rename the old synchronized file before coordinating treatment
with the other devices; leave it unused pending approved disposition.

- **`ADP_OPERATOR_NSEC`:** A copied key can impersonate the operator, including
  signing announcements and fulfillment attestations and authenticating operator
  refund actions. Treat it as potentially compromised. A new key means a new
  operator identity, not a replacement that can authenticate for the old one.
  Before a cutover, review existing terms, authorization and receipt chains,
  and open refund obligations. Verify old receipts and downloads and a new
  fulfillment path with preserved signed evidence; do not invalidate historical
  authorizations blindly.
- **`ADP_MASTER_KEY`:** A copied key can decrypt stored fulfillment private keys
  if the corresponding database or backup is also obtained. Do not change only
  the environment value: existing rows would become unreadable. Plan a tested
  backup/restore and re-encryption of fulfillment keys on a disposable database
  copy before retiring the old key. Re-encryption cannot protect prior copies;
  if ciphertext was also exposed, review rotation of the affected fulfillment
  identities without breaking historical receipts.

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

For listings with settlement terms, `/purchase/confirm` only recovers a
receipt from an already settled quote. Payment proofs belong on the original
settlement attempt. Release builds reject standalone BOLT11/preimage pairs
as legacy payment evidence: those pairs do not establish payment to the
merchant. If legacy paid sales are allowed, the legacy route requires a
verified zap receipt bound to the buyer, listing, and merchant's LNURL provider.

## Design notes worth knowing before extending this

- **Ownership and distribution authorization are checked independently** in
  `routes/game.rs` — a valid receipt alone is never sufficient to serve a file;
  the server must also be named in the listing's `server` tags. See ADP-01.md
  "Trust Model" for why these are deliberately not conflated.
- **Authorization freshness**: current listings are fetched from at least two relays
  for new delegated operations. The listing enables exact `kind:30406` root/key
  pairs; validity and capabilities come only from the developer-signed lifecycle.
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
  kind `30406` authorization lifecycle with the required capability may issue grants
  and receipts. Delegated credentials retain the immutable root ID for historical
  verification after listing removal.
- **Claim idempotency is server-local**: one stored grant is reused for a repeated
  `(buyer, game coordinate, campaign)` claim; independent servers may issue their
  own valid grants.
