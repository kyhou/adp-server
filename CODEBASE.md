# ADP Server — Codebase Documentation

## Table of Contents

1. [Project Identity & Purpose](#1-project-identity--purpose)
2. [Technology Stack](#2-technology-stack)
3. [Repository Layout](#3-repository-layout)
4. [Architecture & Data Flow](#4-architecture--data-flow)
5. [adp-core: Protocol Logic](#5-adp-core-protocol-logic)
6. [adp-server: HTTP Binary](#6-adp-server-http-binary)
7. [Routes & Endpoints](#7-routes--endpoints)
8. [Key Abstractions & Patterns](#8-key-abstractions--patterns)
9. [Build System & Configuration](#9-build-system--configuration)
10. [Testing Strategy](#10-testing-strategy)
11. [How to Add a New Feature](#11-how-to-add-a-new-feature)
12. [Debugging Guide](#12-debugging-guide)
13. [Glossary](#13-glossary)

---

## 1. Project Identity & Purpose

### What does this application do?

This is a **reference implementation of ADP-01 (Arcadestr Distribution Protocol)** — an open HTTP protocol for serving digital game file downloads using **Nostr identities and event kinds** as the authentication and ownership layer.

A developer publishes a NIP-99 (`kind:30402`) game listing on Nostr relays with ADP extension tags (`server`, `file_hash`, `version`, `acquisition`, `fulfillment_authorization`, `lud16`). Buyers can authorize downloads with local tokens, portable Entitlement Grants (provisional local `kind:1030`), anchored NIP-102 receipts (`kind:1020`), or explicit public/timed listing policy. A zero price never grants access. Servers are interchangeable — no single operator is privileged.

### Who is it for?

- **Game developers** who want to distribute game builds through Nostr-based identity, without relying on centralized storefronts.
- **Hosting operators** who run ADP-compliant servers for multiple developers. The provisioning system keeps each developer's fulfillment keys isolated and encrypted at rest.
- **Buyers** who purchase games via Lightning Network (bolt11 invoices or NIP-57 zap receipts) and download files using Nostr-authenticated HTTP requests.

### Main user-facing features

- **Provisioning** — developers request a fulfillment key from an operator. The operator generates a keypair, publishes a `kind:30404` attestation to relays, encrypts the private key at rest, and returns the public key.
- **Upload** — authenticated upload of game archives via multipart POST. The file's SHA-256 must match the listing's `file_hash` tag.
- **Purchase confirmation** — buyer presents a zap receipt or bolt11+preimage as payment proof; the server signs a `kind:1020` NIP-102 receipt with a held fulfillment key and issues a short-lived download token.
- **Download** — checks a local token, then portable grants and anchored receipts, then explicit `public` or active `timed-access` policy. Every path requires the server to be named in the current listing's `server` tags.
- **File integrity** — every file read from disk is SHA-256 verified against the listing's `file_hash` before being served. A background task re-verifies all stored files on an interval.

### Desktop, web, or both?

This is a **pure HTTP server** (axum). There is no desktop UI, no Tauri, no Leptos, no web frontend. It runs as a daemon that serves HTTP requests and publishes Nostr events to relays.

---

## 2. Technology Stack

| Name | Version | Role | Where it appears |
|---|---|---|---|
| Rust | 2021 edition | Language for both crates | `Cargo.toml` (workspace), both crates |
| `nostr` | 0.44 | Nostr protocol types + NIP-44 encryption | `adp-core/Cargo.toml`, throughout `adp-core/src/` |
| `nostr-sdk` | 0.44 | Relay client, event building/signing | `adp-server/Cargo.toml`, `relay.rs`, `keystore.rs`, routes |
| `axum` | 0.7 | HTTP framework + multipart support | `adp-server/Cargo.toml`, `routes/`, `main.rs` |
| `sqlx` | 0.8 | SQLite async via compile-time queries | `adp-server/Cargo.toml`, `storage.rs`, `keystore.rs` |
| `aes-gcm` | 0.10 | AES-256-GCM for fulfillment key encryption at rest | `adp-server/Cargo.toml`, `crypto.rs` |
| `lightning-invoice` | 0.32 | bolt11 invoice parsing | `adp-core/Cargo.toml`, `payment.rs`; also `adp-server/examples/gen_test_invoice.rs` |
| `reqwest` | 0.12 (rustls-tls) | HTTP client for LNURL resolution | `adp-server/Cargo.toml`, `lnurl.rs` |
| `sha2` | 0.10 | SHA-256 hashing | Both crates' `Cargo.toml`, used in `payment.rs`, `upload.rs`, `game.rs`, `integrity.rs` |
| `serde` / `serde_json` | 1 | Serialization everywhere | Both crates |
| `thiserror` | 1 | Error type derivation | Both crates |
| `tracing` / `tracing-subscriber` | 0.1 / 0.3 | Structured logging | `adp-server/Cargo.toml`, `main.rs`, various modules |
| `tower-http` | 0.6 | HTTP tracing middleware | `adp-server/Cargo.toml`, `routes/mod.rs` |
| `uuid` | 1 (v4) | Order IDs and download tokens | Both crates, `receipt.rs`, `purchase.rs` |
| `base64` | 0.22 | NIP-98 auth header decoding | `adp-core/Cargo.toml`, `nip98.rs` |
| `async-trait` | 0.1 | Async trait for `Relay` | `adp-server/Cargo.toml`, `relay.rs` |
| `wiremock` | 0.6 | HTTP mock for route integration tests | `adp-server/Cargo.toml` (dev) |
| `tempfile` | 3 | Temp directories in tests | `adp-server/Cargo.toml` (dev) |

### Why this stack?

- **No framework abstraction**: This is a straightforward Rust HTTP server. The two-crate split (`adp-core` + `adp-server`) reflects a deliberate architectural boundary: protocol logic lives without HTTP or SQL dependencies so it can be unit-tested without a database or network.
- **nostr-sdk 0.44** provides the relay client (`Client::fetch_events`) and event-signing primitives. It is pinned at 0.44 to match the `nostr` crate version.
- **sqlx** with SQLite for zero-config persistence. Migrations run automatically at startup.
- **AES-256-GCM** for key-at-rest encryption. The `ADP_MASTER_KEY` environment variable provides the key; there is no HSM/KMS integration yet (called out in README as a known simplification for production use).

---

## 3. Repository Layout

### Top-level structure

```
adp-server/                              # Root: Cargo workspace root, SQLite migrations, docs
├── Cargo.toml                           # Workspace definition — members: [adp-core, adp-server]
├── Cargo.lock                           # Locked dependencies
├── .env.example                         # Template for env config (copy to .env)
├── ADP-01.md                            # The protocol spec this code implements
├── adp-01-roadmap-v3.md                 # Implementation roadmap / completion checklist
├── VERIFICATION_REPORT.md                    # Historical verification report (records the earlier 47-test run)
├── README.md                            # Entry-point README (known to be partially stale)
│
├── adp-core/                            # Protocol logic crate — no HTTP, no SQL
│   ├── Cargo.toml
│   ├── src/
│   │   ├── lib.rs                       # Public re-exports; module declarations
│   │   ├── error.rs                    # AdpError enum (protocol-level errors)
│   │   ├── listing.rs                   # AdpListing parser + exact authorization references
│   │   ├── chain.rs                     # Shared graph error and completeness semantics
│   │   ├── nip98.rs                     # NIP-98 HTTP Auth decode + verify
│   │   ├── payment.rs                    # verify_bolt11_preimage + verify_zap_receipt
│   │   ├── provisioning.rs             # kind:30404/30406 builder + parser
│   │   └── receipt.rs                  # kind:1020 builder + parser + latest_status_in_chain
│   └── tests/
│       ├── delegation_tests.rs         # 7 tests: signer authorization edge cases
│       └── protocol_regression_tests.rs # 9 tests: round-trip, zap, chain, price
│
├── adp-server/                          # HTTP binary crate
│   ├── Cargo.toml
│   ├── src/
│   │   ├── main.rs                     # Entry point: init, announce, serve
│   │   ├── lib.rs                      # Module declarations only (11 modules)
│   │   ├── config.rs                  # Config: reads env vars into typed struct
│   │   ├── state.rs                   # AppState: shared state passed via axum
│   │   ├── error.rs                    # ApiError enum → HTTP status mapping
│   │   ├── keystore.rs                 # KeyStore: CRUD for provisioned keys (SQLite + AES-GCM)
│   │   ├── crypto.rs                   # MasterCipher: AES-256-GCM encrypt/decrypt wrapper
│   │   ├── storage.rs                  # Storage: SQLite data access (purchases, files, tokens)
│   │   ├── relay.rs                    # RelayClient: multi-relay typed lookups, publishing, and chain completion
│   │   ├── announcement.rs            # kind:30403 server announcement builder
│   │   ├── integrity.rs               # spawn_file_hash_reverification (background task)
│   │   ├── lnurl.rs                    # LNURL resolution: resolve_lud16_lsp_pubkey
│   │   └── routes/
│   │       ├── mod.rs                  # Router construction + middleware
│   │       ├── health.rs              # GET /health, /healthz
│   │       ├── well_known.rs           # GET /.well-known/adp
│   │       ├── provision.rs           # POST /provision, POST /provision/revoke
│   │       ├── upload.rs              # POST /upload
│   │       ├── purchase.rs            # POST /purchase/confirm
│   │       └── game.rs                # GET /game/:game_coordinate
│   ├── examples/
│   │   └── gen_test_invoice.rs        # Generates bolt11 invoice + preimage for local testing
│   └── tests/
│       ├── announcement_tests.rs
│       ├── error_mapping_tests.rs
│       ├── health_tests.rs
│       ├── keystore_tests.rs
│       ├── lnurl_tests.rs
│       ├── multi_relay_freshness_tests.rs
│       └── route_integration_tests.rs
│
├── migrations/
│   ├── 0001_init.sql                   # purchases, files, download_tokens tables
│   └── 0002_provisioning.sql           # fulfillment_keys table + partial unique index
│
├── data/                               # Runtime data (gitignored)
│   ├── adp.db                          # SQLite database
│   └── files/                          # Uploaded game archives, keyed by their SHA-256 hex hash
│
├── docs/superpowers/                    # Auto-generation and planning docs
│   └── plans/ & specs/
│
├── listing_event.json                  # Example kind:30402 listing for testing
├── provision_response.json             # Example response from POST /provision
└── testgame.bin                        # Small binary used as test fixture during upload tests
```

### What to open each file to change

| File | Why you'd open it |
|---|---|
| `adp-core/src/listing.rs` | Add or change listing tag parsing (new ADP-01 tag, changed tag format) |
| `adp-core/src/provisioning.rs` | Change developer-signed fulfillment authorization roots, capabilities, cancellation, or historical resolution |
| `adp-core/src/payment.rs` | Change payment proof verification (new proof type, changed LSP verification logic) |
| `adp-core/src/provisioning.rs` | Change provisioning event formats (kind:30404 or kind:30405 tag changes) |
| `adp-core/src/receipt.rs` | Change NIP-102 receipt format (tag additions, content schema changes) |
| `adp-core/src/nip98.rs` | Change NIP-98 verification (clock skew tolerance, tag requirements) |
| `adp-core/src/error.rs` | Add new protocol error variants |
| `adp-server/src/config.rs` | Add new environment variable configuration |
| `adp-server/src/state.rs` | Add new shared state types (e.g. connection pool, caches) |
| `adp-server/src/error.rs` | Add new HTTP error variants or status code mappings |
| `adp-server/src/storage.rs` | Add new database queries (new functionality needs new tables/queries) |
| `adp-server/src/keystore.rs` | Change key storage schema, add new key derivation patterns |
| `adp-server/src/relay.rs` | Change relay interaction patterns (different fetch strategies, timeouts) |
| `adp-server/src/routes/provision.rs` | Add new provisioning sub-endpoints or change auth model |
| `adp-server/src/routes/upload.rs` | Add new upload constraints (size limits, multi-file upload) |
| `adp-server/src/routes/purchase.rs` | Add new payment verification paths or change receipt content |
| `adp-server/src/routes/game.rs` | Add download throttling, IP-based rate limiting, streaming responses |
| `adp-server/src/main.rs` | Change startup sequence, add new middleware, register new plugins |
| `migrations/0001_init.sql` | Change purchase schema, add new storage tables |
| `migrations/0002_provisioning.sql` | Change fulfillment key schema, add indexes |

---

## 4. Architecture & Data Flow

### 4.1 The Big Picture

```
┌──────────────────────────────────────────────────────────────┐
│                    ADP Server (adp-server)                    │
│                                                              │
│  ┌──────────────┐   ┌─────────────────────────────────┐     │
│  │  RelayClient  │   │       axum HTTP Router           │     │
│  │  (nostr-sdk)  │   │                                 │     │
│  │               │   │  GET  /health                    │     │
│  │  ──► Relays   │   │  GET  /.well-known/adp           │     │
│  │  ◄── Relays   │   │  POST /provision                 │     │
│  └──────┬───────┘   │  POST /provision/revoke          │     │
│         │           │  POST /upload                     │     │
│  ┌──────┴───────┐   │  POST /purchase/confirm            │     │
│  │   KeyStore    │   │  GET  /game/:coordinate          │     │
│  │  (SQLite +    │   └──────────────┬──────────────────┘     │
│  │   AES-GCM)   │                  │                      │
│  └──────┬───────┘   ┌──────────────┴──────────────┐      │
│         │           │        Storage (SQLite)          │      │
│         └───────────┤  purchases│files│tokens         │      │
│                     └─────────────────────────────────┘      │
│                                                              │
└──────────────────────────────────────────────────────────────┘
                           │
                           ▼
                ┌─────────────────────┐
                │    Nostr Relays      │
                │ (wss://relay.*)      │
                │                     │
                │  kind:30402 listings│
                │  kind:30404 attest. │
                │  kind:1020 receipts │
                └─────────────────────┘
```

External actors:
- **Developer** — calls `POST /provision`, `POST /upload`, `POST /purchase/confirm` (via NIP-98 auth), manages listing tags on relays.
- **Buyer** — calls `POST /purchase/confirm` with payment proof or `POST /entitlement/claim` for an eligible campaign, then downloads via token or NIP-98 portable credential lookup. Explicit public/timed policy can also authorize a download.
- **LSP (Lightning Service Provider)** — the LNURL-pay endpoint that signs `kind:9735` zap receipts; resolved from the listing's `lud16` tag.
- **Relays** — Nostr relays store listings, receipts, and attestations. This server reads listing state fresh on every request that needs authorization-sensitive data.

### 4.2 Request Lifecycle: Purchase → Download

The most instructive flow is a buyer purchasing and downloading a game:

#### Step 1: Buyer confirms purchase

1. Buyer constructs a `POST /purchase/confirm` request with:
   - `Authorization: Nostr <base64>` — NIP-98 token bound to `POST /purchase/confirm` at this server's URL.
   - JSON body with `game_coordinate`, `listing_event`, and one of `zap_receipt_event` or `(bolt11, preimage)`.

2. `purchase.rs:confirm_handler` verifies:
   - NIP-98 token signature, URL match, method match, and clock skew (`adp_core::verify_nip98`).
   - `listing_event` signature and coordinate match.
   - Fetches the *fresh* listing from relays (`fetch_listing`) — must not use a cache for delegation checks.
   - Parses the listing and confirms this operator holds an authorized fulfillment key for it.
   - Verifies the payment proof: either `verify_zap_receipt` (checks receipt signature, `P` tag, `a`/`e` tag, LSP pubkey) or `verify_bolt11_preimage` (checks `sha256(preimage) == payment_hash`, optional amount enforcement).
   - Replay check: the zap receipt's event id (or preimage for bolt11 path) must not already be in the `purchases` table (`payment_proof_already_used`). The database `UNIQUE` constraint on `payment_proof_event_id` is the second layer of defense.

3. On success:
   - Builds `ReceiptContent` (items, totals, `fulfilled_at`).
   - NIP-44 encrypts the content to the buyer's pubkey using the fulfillment key's secret key.
   - Signs a `kind:1020` receipt with the fulfillment key.
   - Publishes the receipt to relays.
   - Records the purchase in SQLite.
   - Issues a short-lived download token (UUID v4, stored in `download_tokens` table, default TTL 900s).

#### Step 2: Buyer downloads the game

The download handler at `game.rs:download_handler` first validates the current listing, distribution, and file metadata, then selects the first valid access basis:

**Path A (fast — local token)**:
```
GET /game/30402:developer_pubkey:d-tag?token=uuid-v4
```
- Looks up the token in SQLite. The token is scoped to `game_coordinate`, so it only works for the intended game.
- If valid and not expired, proceeds directly to the distribution check.

**Path B (portable — relay credentials)**:
```
GET /game/30402:developer_pubkey:d-tag
Authorization: Nostr <base64>
```
- Verifies the NIP-98 token and extracts the buyer's pubkey.
- Queries relays for Entitlement Grants and NIP-102 receipts addressed to the buyer and game coordinate.
- Resolves credential status chains and signer authorization-root chains using per-relay EOSE completeness.
- Accepts a grant or paid/fulfilled receipt signed by the publisher or a fulfillment key anchored to an active publisher-signed kind `30406` lifecycle.

**Path C (explicit listing policy)**:
- Accepts `acquisition: public` or a currently active half-open `timed-access` interval.
- Price is not an authorization signal; zero price without explicit policy remains gated.

After the access check:
- **Distribution authorization** — checks the already-fetched fresh listing with `listing.distributes_via(server_url)`. Returns HTTP 451 ("Unavailable For Legal Reasons") if this server is not named in the listing's `server` tags. (HTTP 451 is a deliberate choice — it distinguishes "you're not authorized" from "you don't own this.")
- **File hash verification** — reads the file from disk and SHA-256 hashes it. If the hash doesn't match the listing's `file_hash`, returns 500 (data corruption detected).
- **Conditional GET** — if the request's `If-None-Match` header matches the file hash's ETag, returns 304 Not Modified.
- Otherwise returns the file with an `ETag` header.

### 4.3 Provisioning Lifecycle

**Scenario: developer wants a fulfillment key for a new game scope.**

1. Developer calls `POST /provision` with NIP-98 auth and optional `scope` body field.
2. Handler extracts the developer pubkey from the verified NIP-98 token.
3. Checks `active_key_for_scope(developer_pubkey, scope)` — if an unrevoked key exists for this scope, returns it (idempotent).
4. Generates a new `Keys::generate()` keypair.
5. Builds `kind:30404` attestation event via `build_attestation_event_builder(...)`, signs with operator key.
6. Publishes attestation to relays.
7. Encrypts the private key via `MasterCipher::encrypt(...)` (AES-256-GCM with server's master key).
8. Stores the encrypted key + nonce in `fulfillment_keys` table.

**Revocation**:
1. Developer calls `POST /provision/revoke` with NIP-98 auth and `fulfillment_pubkey`.
2. Server verifies the developer owns this key (SQL query with both developer_pubkey and fulfillment_pubkey).
3. Builds a new `kind:30404` with `revoked_at` set to now, publishes it.
4. Updates the DB row's `revoked_at` field.
5. Local revocation prevents this operator from signing anything new. Existing credentials remain verifiable from their immutable developer-signed `kind:30406` root; listing removal and operator `30404` revocation are not retroactive.

### 1.4 Data Flow Diagram

```
Developer's listing (kind:30402) on relays
    │
    ├──► upload.rs: POST /upload
    │       Check: developer or delegated key (NIP-98)
    │       Check: file SHA-256 matches listing's file_hash
    │       Action: write file to disk, record in `files` table
    │
    ├──► purchase.rs: POST /purchase/confirm
    │       Check: listing fresh from relay
    │       Check: buyer (NIP-98)
    │       Check: payment proof (zap receipt or bolt11+preimage)
    │       Check: replay protection (payment_proof_event_id unique)
    │       Action: sign kind:1020 receipt, publish to relay
    │       Action: insert `purchases` row
    │       Action: insert `download_tokens` row (15m TTL)
    │
    └──► game.rs: GET /game/:coordinate
            Fetch fresh listing and validate distribution/file metadata
            Path A (fast): ?token= → resolves via `download_tokens`
            Path B (portable): NIP-98 → queries relays for grants/receipts
                ├── resolve credential status chain
                └── resolve signer kind:30406 authorization root
            Path C: explicit public or active timed-access listing policy
            Check: distributes_via(this server URL) — HTTP 451 if no
            Check: file_hash matches on-disk file (SHA-256)
            Action: serve file (with ETag/If-None-Match support)
```

---

## 5. adp-core: Protocol Logic

This crate lives at `adp-core/` and has **zero** dependencies on HTTP, SQLite, or networking. It depends only on `nostr`, `serde`, `sha2`, `uuid`, `lightning-invoice`, `base64`, and the shared workspace deps.

### 5.1 Module Map

| Module | Key types | Key functions | What it does |
|---|---|---|---|
| `lib.rs` | — | — | Re-exports all public API at crate root |
| `error.rs` | `AdpError` | — | 10 error variants, all with `#[error("...")]` messages |
| `listing.rs` | `AdpListing`, `FulfillmentAuthorization` | `AdpListing::from_event`, `coordinate()`, `distributes_via()` | Parses `kind:30402` tags and exact authorization root/key references |
| `provisioning.rs` | `AuthorizationTerms`, `ResolvedAuthorization` | `parse_authorization_event`, `resolve_authorization_at` | Validates capability-scoped `kind:30406` roots and prospective cancellation |
| `nip98.rs` | `Nip98Verified` | `decode_authorization_header`, `verify_nip98` | Decodes `Authorization: Nostr <base64>` and verifies NIP-98 token |
| `payment.rs` | — | `verify_bolt11_preimage`, `verify_zap_receipt` | Validates payment proofs |
| `provisioning.rs` | `ParsedAttestation`, `ParsedAcceptance` | `build_attestation_event_builder`, `parse_attestation_event`, `build_acceptance_event_builder`, `parse_acceptance_event` | Constructs and parses provisioning events |
| `receipt.rs` | `ParsedReceipt`, `ReceiptContent`, `ReceiptItem`, `BuildReceiptParams` | `new_order_id`, `encrypt_receipt_content`, `build_receipt_event_builder`, `parse_receipt_event`, `latest_status_in_chain` | Builds/parses `kind:1020` receipts and resolves chain tips |

### 5.2 `AdpError` — Protocol Error Types

`adp-core/src/error.rs` defines 10 variants:

```rust
pub enum AdpError {
    InvalidSignature,                                          // event.verify() failed
    MissingTag(&'static str),                                  // required tag absent
    CoordinateMismatch { expected: String, found: String },    // coordinate mismatch
    UnauthorizedSigner(String),                                // not developer, not delegated
    RevokedAtSigningTime { revoked_at: u64, created_at: u64 }, // key revoked before receipt
    InvalidPaymentProof(String),                               // payment proof invalid
    Nip98(String),                                              // NIP-98 auth failure
    Encryption(String),                                         // NIP-44 encrypt failed
    Nostr(String),                                              // nostr protocol error
    Serde(#[from] serde_json::Error),                           // JSON serialization
}
```

These are protocol-level errors that do **not** carry HTTP status codes. The mapping to HTTP status happens in `adp-server/src/error.rs` (see §7.2).

### 5.3 Fulfillment Authorization

Listings contain repeatable `fulfillment_authorization` tags with an immutable
`kind:30406` root ID and fulfillment pubkey. New delegated operations require an
exact current listing entry, an active developer-signed lifecycle, the operation's
capability, and local key custody when the server signs. Historical receipt and grant
verification uses the credential root timestamp and does not require current listing
membership or `kind:30404` evidence. Cancellation is prospective and effective at
its event timestamp.

### 5.4 Receipt Chain Resolution

`latest_status_in_chain` at `receipt.rs:176-203` takes all `kind:1020` events for a single `order_id` and finds the terminal (authoritative) event by following `e`-tag references. It builds a `HashSet<String>` of all referenced event IDs, then returns the one event that is *not* referenced by any other — the chain tip.

This is deliberately conservative: if multiple events share the same `order_id` but the `e`-tag references form a fork (ambiguous chain), `None` is returned rather than falling back to `created_at` timestamps (which the signer controls). See `protocol_regression_tests.rs:102-143` for the fork scenario test.

### 5.5 NIP-98 Verification

`nip98.rs:verify_nip98` enforces:
- Event signature verification (`event.verify()`).
- Kind check (`27235`).
- Clock skew — `max 60 seconds` from `Timestamp::now()`.
- `u` tag must match the expected URL (with trailing-slash normalization).
- `method` tag must match the expected HTTP method (case-insensitive).

### 5.6 Payment Proof Verification

**`verify_bolt11_preimage`** (`payment.rs:14-49`):
- Parses the bolt11 invoice.
- Computes `SHA-256(preimage)` and compares to `invoice.payment_hash`.
- If `expected_amount_msat` is `Some(n)`, enforces `invoice.amount >= n`.

**`verify_zap_receipt`** (`payment.rs:55-98`):
- Verifies event signature.
- Checks kind is `9735`.
- Checks signature pubkey matches the listing's resolved LSP `nostrPubkey`.
- Checks the receipt has a `P` tag with the buyer's pubkey.
- Checks the receipt has an `a` or `e` tag with the game coordinate.

---

## 6. adp-server: HTTP Binary

### 6.1 Entry Point — `main.rs`

The server startup sequence:

1. **Logging init** — `tracing_subscriber` with `EnvFilter` (defaults to `info`, overridable via `RUST_LOG`).
2. **Config load** — `Config::from_env()` reads env vars.
3. **Minimum relays check** — refuses to start if `< 2` relays configured (enforces multi-relay freshness for listing data).
4. **Directory creation** — files dir and parent of DB path.
5. **Operator keys** — parses `ADP_OPERATOR_NSEC`; logs the public key.
6. **Storage connect** — `Storage::connect` runs pending migrations, returns pool.
7. **KeyStore init** — wraps pool + master key.
8. **Relay connect** — `RelayClient::connect` subscribes to all configured relays. The operator's key is passed as the client's identity (used for event signing at publish time).
9. **Server announcement** — builds a `kind:30403` event and publishes to relays.
10. **Background revert async** — `spawn_file_hash_reverification` runs in a `tokio::spawn` loop.
11. **Router + axum::serve** — graceful shutdown on SIGTERM/Ctrl+C.

### 6.2 AppState — Shared State

`state.rs` defines `AppState`, the axum-managed shared state:

```rust
pub struct AppState {
    pub config: Arc<Config>,
    pub storage: Storage,          // Clone (SqlitePool inside)
    pub relay: Arc<dyn Relay>,     // Interface for mocking in tests
    pub keystore: Arc<KeyStore>,   // Thread-safe read/write via SqlitePool
    pub operator_keys: Keys,       // Used in provisioning_handler to sign attestations
}
```

`Storage` is `Clone` because `SqlitePool` is internally `Arc`. `relay` is `Arc<dyn Relay>` to allow the `MockRelay` in tests. `keystore` is `Arc<KeyStore>` for shared access to the pool + master key.

### 6.3 API Error Mapping

`error.rs` defines `ApiError` with two protocol-level sources (`adp_core::AdpError` via `#[from]`, `sqlx::Error` via `#[from]`) and several direct HTTP variants (`BadRequest`, `Unauthorized`, `PaymentRequired`, `Conflict`, `NotFound`, `Forbidden`, `DistributionUnauthorized`, `Internal`).

The `IntoResponse` implementation maps each variant to HTTP status:

| Error variant | Status | Meaning |
|---|---|---|
| `AdpError::Nip98(_)` | 401 | Bad auth token |
| `AdpError::Encryption/_` | 500 | NIP-44 or AES-GCM failure |
| `AdpError::Nostr(_)` | 500 | Nostr protocol error |
| `AdpError::Serde(_)` | 500 | JSON serialization failure |
| Other `AdpError` variants | 400 | Bad request (invalid tags, malformed data) |
| `ApiError::Storage(_)` | 500 | SQLite error |
| `ApiError::BadRequest` | 400 | Generic bad request |
| `ApiError::Unauthorized` | 401 | Missing or invalid auth |
| `ApiError::PaymentRequired` | 402 | Payment proof invalid |
| `ApiError::Conflict` | 409 | Replay / duplicate |
| `ApiError::NotFound` | 404 | Resource not found |
| `ApiError::Forbidden` | 403 | Ownership check fails |
| `ApiError::DistributionUnauthorized` | 451 | Server not in listing's `server` tags |
| `ApiError::Internal` | 500 | Catch-all internal error |

The `DistributionUnauthorized` variant is deliberately HTTP 451 (Unavailable For Legal Reasons), which is semantically distinct from 403 (Forbidden). The spec rationale: 403 means "you don't own this," while 451 means "the server is not authorized to distribute this listing regardless of ownership."

### 6.4 KeyStore

`keystore.rs` manages the lifecycle of provisioned fulfillment keys. Key points:

- **Encryption at rest**: Private keys are encrypted via `MasterCipher` (AES-256-GCM) before insertion into the `fulfillment_keys` table. The nonce is stored alongside the ciphertext.
- **Idempotent provisioning**: `active_key_for_scope(developer_pubkey, scope)` returns the first unrevoked key for the given scope. This is the idempotency mechanism — if a developer re-requests the same scope, the same key is returned without generating a new one.
- **Ownership checks**: `revoke` requires both `developer_pubkey` and `fulfillment_pubkey`, and the `developer_pubkey` comes from the NIP-98 token (not the request body), so only the key's owner can revoke it.
- **Lookup by fulfillment pubkey**: `key_for_fulfillment_pubkey` is the critical method used by `purchase.rs` and `upload.rs` — given a pubkey from a listing's delegation tag, find the corresponding private key (if this operator holds it). Returns `None` if the key was revoked or never provisioned here.
- **Partial unique index**: `idx_fulfillment_keys_active_scope` enforces that only one unrevoked key exists per `(developer, scope)` pair, while allowing multiple revoked rows for the same pair. This is *more* correct than a blanket UNIQUE constraint because it allows re-provisioning after revocation.

### 6.5 Storage (`storage.rs`)

Straightforward SQLite access layer. Key tables:

- **`purchases`**: `order_id` (UNIQUE), `payment_proof_event_id` (UNIQUE), `buyer_pubkey`, `game_coordinate`, `receipt_event_id`, `created_at`. The dual UNIQUE constraints provide NIP-102's idempotency at the database level.
- **`download_tokens`**: `token` (TEXT PRIMARY KEY), scoped to `game_coordinate`, with `expires_at`. Used for Path A fast downloads.
- **`files`**: `file_hash` (SHA-256, PRIMARY KEY), `game_coordinate`, `file_path`, `uploaded_at`. `record_file` uses `ON CONFLICT(file_hash) DO UPDATE` so re-uploading the same file updates its mapping.

`migrations/` are run at startup via `sqlx::migrate!` in `Storage::connect`.

### 6.6 Relay Client (`relay.rs`)

Wraps `nostr_sdk::Client` behind the `Relay` trait. The trait exposes typed
lookups for listings, receipts, campaigns, entitlement grants, authorizations,
authorization successors, and individual events, plus `publish`; the older
`fetch_listing` and `fetch_receipts` shapes remain as hidden adapters for
routes that have not migrated.

```rust
#[async_trait]
pub trait Relay: Send + Sync {
    fn server_url(&self) -> &str;
    async fn publish(&self, event: &Event) -> RelayResult<()>;
    async fn lookup_event(&self, event_id: EventId) -> RelayLookup<Event>;
    async fn lookup_listing(&self, developer: PublicKey, d_tag: &str) -> RelayLookup<Event>;
    async fn lookup_campaigns(&self, coordinate: &str) -> RelayLookup<Vec<Event>>;
    async fn lookup_entitlements(&self, buyer: PublicKey, coordinate: &str) -> RelayLookup<Vec<Event>>;
    async fn lookup_receipts(&self, buyer: PublicKey, coordinate: &str) -> RelayLookup<Vec<Event>>;
    async fn lookup_authorizations(&self, developer: PublicKey, coordinate: &str) -> RelayLookup<Vec<Event>>;
    async fn lookup_authorization_successors(&self, developer: PublicKey, event_id: EventId) -> RelayLookup<Vec<Event>>;
}
```

The trait exists specifically to enable test doubles such as `MockRelay` in
`route_integration_tests.rs`. `RelayClient` implements it using
`nostr_sdk::Client` and returns `RelayLookup<T>` so callers can distinguish a
confirmed absence from insufficient relay coverage.

Key design decisions:
- Collection lookups query all configured relays concurrently, require at least two successful relay queries for a complete result, and record failures as `RelayLookup::Unavailable` rather than treating partial data as absence. An exact event match can be returned as soon as it is found.
- Listing lookup selects the newest valid event with `select_latest_listing`. Receipt lookup builds a filter with `#p` (not `.author()`), fetching events that **tag** the buyer pubkey and game coordinate.
- Individual event lookup uses an `ids` filter, not an `#e` tag filter. This is important for predecessor-chain completion, where the referenced event ID itself must be fetched.
- Relay queries require matching EOSE, unsubscribe subscriptions during cleanup, and enforce event-count, serialized-byte, and authorization-graph depth/edge limits.
- `publish` calls `self.client.send_event(event)` which publishes to all connected relays.

### 6.7 LNURL Resolution (`lnurl.rs`)

Resolves a `lud16` Lightning address to the LSP's `nostrPubkey`. The flow:

1. Split `name@domain` at `@`.
2. Construct the LNURL-pay endpoint URL: `https(or http)://domain/.well-known/lnurp/{name}`.
3. HTTP GET the endpoint, parse the JSON response.
4. Extract the `nostrPubkey` field and parse as a `PublicKey`.

The scheme is always `https://` except when `ADP_ALLOW_INSECURE_LNURL=true` AND the domain is `localhost`, `127.0.0.1`, or `::1` (see `is_loopback_domain` at `lnurl.rs:64-70`). This is the only HTTP-like fallback in the codebase.

### 6.8 File Integrity (`integrity.rs`)

A background task (`spawn_file_hash_reverification`) runs every `ADP_FILE_REVERIFY_INTERVAL_SECS` (default 3600s). On each tick, it reads all files from the `files` table, re-computes SHA-256, and rejects any mismatch with `tracing::error!`. This catches silent disk corruption between upload and download.

`verify_stored_files_once` is also exposed as a public function for potential administration endpoints.

---

## 7. Routes & Endpoints

### 7.1 Router Construction

`routes/mod.rs` builds the axum `Router`:

```rust
Router::new()
    .route("/health", get(health::health_handler))
    .route("/healthz", get(health::health_handler))
    .route("/.well-known/adp", get(well_known::get_well_known))
    .route("/provision", post(provision::provision_handler))
    .route("/provision/revoke", post(provision::revoke_handler))
    .route("/upload", post(upload::upload_handler))
    .route("/purchase/confirm", post(purchase::confirm_handler))
    .route("/game/:game_coordinate", get(game::download_handler))
    .layer(DefaultBodyLimit::max(1024 * 1024 * 1024))  // 1 GB multipart/upload limit
    .layer(TraceLayer::new_for_http())                   // Request logging via tracing
    .with_state(state)
```

### 7.2 Endpoint Reference Table

| Method | Path | Auth | Request body | Response | Error codes |
|---|---|---|---|---|---|
| `GET` | `/health` | none | — | `{"status": "ok"}` | — |
| `GET` | `/healthz` | none | — | `{"status": "ok"}` | — |
| `GET` | `/.well-known/adp` | none | — | ADP version, operator identity, and provisional kind map | — |
| `POST` | `/provision` | NIP-98 (developer) | `{"scope": "..."}` | `{"fulfillment_pubkey":"...","attestation_event_id":"...","scope":"..."}` | 401 (bad auth), 500 (internal) |
| `POST` | `/provision/revoke` | NIP-98 (developer) | `{"fulfillment_pubkey":"..."}` | `{"fulfillment_pubkey":"...","attestation_event_id":"...","revoked_at":...}` | 401, 400 (bad pubkey), 403 (not theirs), 500 |
| `POST` | `/upload` | NIP-98 (developer or delegated key) | Multipart: `listing_event` + `file` | `{"game_coordinate":"...","file_hash":"...","download_url":"..."}` | 401, 400 (bad listing/signature/hash), 404 (listing not on relays), 500 |
| `POST` | `/purchase/confirm` | NIP-98 (buyer) | `{"game_coordinate":"...","listing_event":{...},"zap_receipt_event":?... }` | `{"receipt":{...},"download_token":"...","token_expires_at":...}` | 401, 400, 402 (bad payment), 409 (replay), 404 (listing not found), 500 |
| `POST` | `/entitlement/claim` | NIP-98 (buyer) | `{"game_coordinate":"...","campaign_event_id":"..."}` | grant event and download token | 400, 401, 403, 404, 409, 503 |
| `GET` | `/game/:game_coordinate` | NIP-98 or `?token=` | — | Binary file (with ETag) | 401, 400, 403 (no ownership), 404 (file/listing not found), 451 (not authorized), 500 (file corruption) |

### 7.3 Route Details

#### `GET /.well-known/adp` — `well_known.rs`

Unauthenticated endpoint returning server metadata. The `pubkey` field is the operator's identity key (from `ADP_OPERATOR_NSEC`). The response uses the centralized ADP version and advertises Entitlement Grant `1030` and ADP campaign `1031` as provisional kinds.

#### `POST /provision` & `/provision/revoke` — `provision.rs`

Auth helper `verify_developer` at line 149-162 is a pattern worth copying:
- Reads `Authorization` header.
- Calls `decode_authorization_header` then `adp_core::verify_nip98` with the exact expected URL (constructed as `state.config.server_url + path`).
- Returns the verified pubkey.

For provision, the developer's NIP-98 token must be signed by their own key — the extracted pubkey is used both for the attestation's `p` tag and for the keystore idempotency lookup.

For revoke, the handler additionally:
1. Parses `fulfillment_pubkey` from request body.
2. Calls `attestation_for_fulfillment_pubkey` — a DB query that filters on both `developer_pubkey` (from token) and `fulfillment_pubkey` (from request). If no row found, the caller doesn't own this key.
3. Builds a new `kind:30404` attestation with `revoked_at` set to `now`, publishes it, then sets `revoked_at` in the DB.

#### `POST /upload` — `upload.rs`

Verification chain:
1. NIP-98 auth (developer or fulfillment key).
2. Parse multipart form: `listing_event` (JSON Nostr event) + `file` (binary).
3. Verify the submitted `listing_event` is a valid NIP-99 listing (signature check).
4. Fetch the same listing fresh from relays — confirms the relay-hosted version matches.
5. Check uploader authorization: the signer is the developer, or the listing contains an exact authorization root/key pair whose lifecycle is active with `upload_build`.
6. Compute SHA-256 of uploaded bytes, compare to listing's `file_hash`.
7. Write file to disk at `{files_dir}/{computed_hash}`.
8. Record in `files` table.

Delegated upload authenticates with the fulfillment key itself, so server key custody is not required for this operation.

#### `POST /purchase/confirm` — `purchase.rs`

This is the most complex handler (~180 lines). Key design points:

**Signing key selection** (lines 110-129): The handler iterates all listing fulfillment delegations, finds one that's:
1. Currently active (`authorizes_at(now, listing_created_at)`)
2. Held by this operator (`keystore.key_for_fulfillment_pubkey` returns `Some(keys)`)

This means the operator might be running a key for this listing but the listing's delegation tag must also be active — the operator cannot unilaterally sign receipts for a listing that no longer delegates to their key.

**Payment proof path selection** (lines 134-156):
- If `zap_receipt_event` is provided → verifies it and uses its event id as the replay key.
- Else if both `bolt11` and `preimage` are provided → verifies them and uses the preimage as the replay key.
- Otherwise → 400 error.

**Receipt content** (lines 180-213): The encrypted content follows NIP-102's schema with `order_id`, `items`, `subtotal`/`tax`/`shipping`/`total`/`currency`, `notes`, and `fulfilled_at`. The content is NIP-44 encrypted to the buyer's pubkey using the fulfillment key's secret key (so only the buyer and the signing key holder can read it).

**Replay protection** (lines 161-172): The `replay_key` is either the zap receipt's event id or the preimage hash. The `UNIQUE(payment_proof_event_id)` constraint in the `purchases` table is the final backstop.

**Price resolution** (`expected_amount_msat` at lines 37-44):
- `SAT`/`SATS` → `amount * 1000` (millisatoshis per satoshi).
- `BTC` → `amount * 100_000_000_000` with up to 8 decimal places.
- Other currencies (fiat) → `None` (amount check skipped).

#### `GET /game/:game_coordinate` — `game.rs`

The download handler fetches the fresh listing before authentication, validates distribution and file metadata, then evaluates token, portable credentials, and explicit listing policy in order:

**Path A — download token** (lines 49-56): If `?token=` is present, look it up in SQLite. The token was issued by the same server during `POST /purchase/confirm`, scoped to the game coordinate. The fresh listing fetch still occurs, but no receipt relay query is needed.

**Path B — portable credential**: If no token, require NIP-98 auth and extract the buyer pubkey. Query relays for Entitlement Grants and NIP-102 receipts, resolve each credential chain and its signer authorization root, and accept any currently valid durable credential.

**Path C — explicit policy**: If no durable credential authorizes access, accept only `acquisition: public` or a currently active half-open `timed-access` interval. Zero price is ignored for authorization.

**Distribution check**: `listing.distributes_via(state.relay.server_url())`. If the server isn't named in the current listing's `server` tags, returns HTTP 451 for every access basis.

**File integrity** (lines 97-113): Looks up the file path by hash, re-verifies SHA-256 on every download, supports `If-None-Match` conditional requests.

---

## 8. Key Abstractions & Patterns

### 8.1 The `Relay` Trait

`relay.rs` defines `#[async_trait] pub trait Relay` with typed lookup methods for each protocol resource and a `publish` operation. This is the only abstraction boundary in the server crate that exists *solely* for testability. `RelayClient` implements it with real relay connections. `MockRelay` (in `tests/route_integration_tests.rs`) implements it with an in-process `HashMap<String, Event>`.

The trait is used via `Arc<dyn Relay>` in `AppState`, so either implementation can be injected at startup. No other type in the server uses trait objects — this is notable because it's the only test double in the codebase.

### 8.2 Two-Layer Error Handling

Protocol errors (`AdpError`) and server errors (`ApiError`) are separate enums connected by `#[from]`:

```
AdpError (adp-core)            ApiError (adp-server)
─────────────                   ──────────────
MissingTag                      ─► Protocol(AdpError) ─► 400 (most variants)
InvalidSignature                ─► Protocol(AdpError) ─► 400
InvalidPaymentProof             ─► Protocol(AdpError) ─► 400
UnauthorizedSigner              ─► Protocol(AdpError) ─► 400
Nip98                           ─► Protocol(AdpError) ─► 401
Encryption                      ─► Protocol(AdpError) ─► 500
Nostr                           ─► Protocol(AdpError) ─► 500
Serde                           ─► Protocol(AdpError) ─► 500
                                ─► Storage(sqlx::Error) ─► 500
                                ─► BadRequest(string) ─► 400
                                ─► Unauthorized(string) ─► 401
                                ─► PaymentRequired(string) ─► 402
                                ─► Forbidden(string) ─► 403
                                ─► NotFound(string) ─► 404
                                ─► Conflict(string) ─► 409
                                ─► DistributionUnauthorized(string) ─► 451
                                ─► Internal(string) ─► 500
```

The `?` operator in route handlers flows through `ApiError::Protocol(#[from] adp_core::AdpError)`, which then maps to the correct status. Explicit `.map_err(|e| ApiError::PaymentRequired(...))` is used for payment failures that must return 402 rather than 400.

### 8.3 Database-Level Idempotency

Two `UNIQUE` constraints in `purchases` table enforce idempotency at the storage layer:

```sql
order_id                TEXT NOT NULL UNIQUE,
payment_proof_event_id  TEXT NOT NULL UNIQUE,
```

APPLICATION code checks `payment_proof_already_used` before attempting the insert, but even if that check had a race condition, the `UNIQUE` constraint would reject the second insert. This matches ADP-01's specification: "replay protection must be enforced at the database layer, not just in application logic."

### 8.4 NIP-44 Encryption Pattern

Receipt content is NIP-44 encrypted to the buyer's pubkey using the **signing key's** secret key:

```rust
nip44::encrypt(
    signing_keys.secret_key(),  // sender's secret — the fulfillment key
    buyer_pubkey,              // recipient's public — the buyer
    plaintext,                 // the ReceiptContent JSON
    nip44::Version::V2,
)
```

This follows NIP-102's requirement that receipt content be "readable only by the merchant and the customer" — the merchant (fulfillment key holder) can read it because they have the secret key; the buyer can read it because NIP-44 allows them to derive the shared secret.

### 8.5 Historical Authorization

Authorization, campaign, grant, and receipt graphs share bounded predecessor
reachability and fork/cycle mechanics while retaining protocol-specific transition
admission. Invalid successors are excluded, valid siblings may recover from the last
valid tip, and historical evaluation ignores later events. Missing relay evidence is
reported as unavailable rather than invalid.

### 8.6 Graceful Shutdown

`main.rs:86-110` implements graceful shutdown via `axum::serve`'s `with_graceful_shutdown`:

```rust
let ctrl_c = async { tokio::signal::ctrl_c().await... };
#[cfg(unix)]
let terminate = async { tokio::signal::unix::signal(SIGTERM).recv().await };
tokio::select! {
    _ = ctrl_c => {},
    _ = terminate => {},
}
tracing::info!("shutdown signal received");
```

This handles both Ctrl+C and SIGTERM. On Windows, the SIGTERM variant becomes `std::future::pending` (never resolves), so only Ctrl+C works there.

---

## 9. Build System & Configuration

### 9.1 How to Build and Run

```bash
# Prerequisites: Rust toolchain (stable), SQLite development headers (for sqlx).

# Set up configuration
cp .env.example .env
# Edit .env: set ADP_OPERATOR_NSEC and ADP_MASTER_KEY

# Load environment
set -a; source .env; set +a

# Check everything compiles
cargo check -p adp-core --all-targets
cargo check -p adp-server --all-targets

# Run all tests
cargo test --workspace

# Full verification gate
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# Run the server
cargo run -p adp-server
```

The CI pipeline (`.github/workflows/ci.yml`) runs these steps:
```
cargo fmt --all --check
cargo build --workspace
cargo clippy --workspace -- -D warnings
cargo test --workspace
```

### 9.2 Environment Variables

| Variable | Required | Default | Description |
|---|---|---|---|
| `ADP_SERVER_URL` | No | `http://localhost:8080` | Server's public base URL. Must match the listing's `server` tag. Used to construct NIP-98 `u` tag expectations. |
| `ADP_BIND_ADDR` | No | `0.0.0.0:8080` | TCP bind address. |
| `ADP_DB_PATH` | No | `./data/adp.db` | SQLite database path. |
| `ADP_FILES_DIR` | No | `./data/files` | Directory for uploaded game archives. |
| `ADP_RELAYS` | No | `wss://relay.damus.io,wss://nos.lol` | Comma-separated relay URLs. Minimum 2. |
| `ADP_OPERATOR_NSEC` | **Yes** | — | bech32 `nsec1...` or hex secret key. Signs kind:30403 announcements and kind:30404 attestations. |
| `ADP_MASTER_KEY` | **Yes** | — | 64 hex chars = 32 raw bytes. AES-256-GCM key for fulfillment key encryption at rest. |
| `ADP_ALLOW_INSECURE_LNURL` | No | `false` | If `true` and domain is loopback (localhost, 127.0.0.1, ::1), allowed http:// LNURL resolution. |
| `ADP_DOWNLOAD_TOKEN_TTL_SECS` | No | `900` | TTL for download tokens issued by `/purchase/confirm`. |
| `ADP_FILE_REVERIFY_INTERVAL_SECS` | No | `3600` | Seconds between background SHA-256 re-verification passes over stored files. |

### 9.3 Workspace Structure

The root `Cargo.toml` defines the workspace and shared dependencies:

```toml
[workspace]
members = ["adp-core", "adp-server"]
resolver = "2"

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["full"] }
thiserror = "1"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
uuid = { version = "1", features = ["v4"] }
sha2 = "0.10"
hex = "0.4"
anyhow = "1"
nostr = { version = "0.44", features = ["nip44"] }
nostr-sdk = "0.44"
lightning-invoice = "0.32"
```

### 9.4 Feature Flags

Neither crate defines Cargo feature flags. The `nip44` feature of the `nostr` crate is enabled in workspace dependencies to support NIP-44 encryption in `receipt.rs`.

---

## 10. Testing Strategy

### 10.1 Test Layout

```
adp-core/tests/
├── authorization_tests.rs       # authorization graph and credential resolution contracts
├── campaign_tests.rs            # campaign parsing and state transitions
├── delegation_tests.rs          # pure logic tests for verify_signer_authorization
├── entitlement_tests.rs         # entitlement grant parsing and validation
├── protocol_regression_tests.rs # listing, receipt, attestation, and zap regressions
├── public_contract_tests.rs     # public protocol API contracts
└── receipt_authorization_tests.rs # receipt signer and authorization behavior

adp-server/tests/
├── announcement_tests.rs          # 1 test: kind:30403 event building
├── claim_route_tests.rs            # campaign claim, idempotency, cancellation, mismatch
├── download_authorization_tests.rs # grant, receipt, explicit policy, zero-price denial
├── error_mapping_tests.rs         # 3 tests: ApiError → HTTP status code mapping
├── health_tests.rs                # 1 test: GET /health endpoint
├── keystore_tests.rs              # 5 tests: key round-trip, idempotency, scope isolation, lookup, revoke
├── lnurl_tests.rs                 # 2 tests: LNURL resolution and malformed LUD16 (wiremock)
├── multi_relay_freshness_tests.rs # 3 live-WebSocket tests: dead relay tolerance and latest listing selection
├── public_contract_tests.rs       # public server API contracts
└── route_integration_tests.rs     # full lifecycle and route integration tests
```

Additionally, there are inline `#[cfg(test)] mod tests` in:
- `adp-server/src/routes/upload.rs` (4 tests: file_hash required, delegated uploader)
- `adp-server/src/routes/game.rs` (receipt matching, ETag, and file hash verification)
- `adp-server/src/routes/purchase.rs` (3 tests: SATS/BTC/fiat price resolution)
- `adp-server/src/relay.rs` (filter construction, relay completeness, chain limits, cycle detection, and lookup adapters)
- `adp-server/src/lnurl.rs` (4 tests: exact loopback detection and scheme selection)
- `adp-server/src/integrity.rs` (2 tests: file hash verification + mismatch rejection)

Total (verified with `cargo test --workspace`): **192 tests across 27 suites**.

### 10.2 What's Tested vs What's Not

**Well-tested:**
- Pure delegation logic (9 scenarios covering all interval edge cases)
- Protocol-level round-trips (building → signing → parsing attestation and acceptance events)
- Receipt parsing and chain resolution (missing status, ambiguous forks)
- Zap verification (buyer P tag, LSP pubkey mismatch)
- Error-to-HTTP mapping (6 protocol variants + server-level variants)
- Price resolution (SATS, BTC, fiat)
- Zero-price denial without explicit acquisition policy
- File hash verification (matching, mismatched)
- Upload authorization (file hash required, delegated uploader)
- LNURL resolution (wiremock-based happy path + malformed address)

**Not tested:**
- Full relay-network behavior beyond the mock and local WebSocket integration suites
- Multi-relay conflict (no integration test with two relays publishing different listing versions for the same coordinate)
- LNURL transport errors (LNURL endpoint returning 404 or malformed JSON)
- The actual relay network interaction (all relay tests use `MockRelay` or in-process `nostr-sdk` constructs)

### 10.3 MockRelay Pattern

`tests/route_integration_tests.rs` defines `MockRelay` implementing the `Relay` trait:

```rust
#[async_trait]
impl Relay for MockRelay {
    // ...
    async fn fetch_listing(&self, developer_pubkey: PublicKey, d_tag: &str) -> RelayResult<Option<Event>> {
        let lock = self.events.lock().unwrap();
        // filter by kind:30402, author, and identifier tag
        Ok(lock.values()
            .filter(|e| e.kind.as_u16() == 30402 && e.pubkey == developer_pubkey)
            .find(|e| e.tags.iter().any(|t| /* d tag match */))
            .cloned())
    }
}
```

The full integration test (`provision_confirm_download_revoke_confirm_fails`) exercises the complete lifecycle:
1. Provision a fulfillment key.
2. Confirm purchase (with mock payment).
3. Download via token.
4. Revoke the key.
5. Confirm another purchase → must fail because the key was revoked.

---

## 11. How to Add a New Feature

This section walks through adding a hypothetical feature: **a new endpoint that reads server stats (file count, purchase count, uptime) and returns them as JSON**. It follows the existing patterns in the codebase.

### Step 1: Add the Rust backend logic

Decide where the logic lives:
- **If the logic is protocol-level** (parsing events, verifying signatures) → `adp-core/src/` with a new module or extension to an existing one.
- **If the logic touches HTTP, SQL, or the relay** → `adp-server/src/` with a new module or route.

For our stats endpoint, the logic reads from the SQLite database, so it belongs in the server crate.

### Step 2: Add the query method to `Storage`

In `adp-server/src/storage.rs`, add new methods:

```rust
pub async fn purchase_count(&self) -> Result<i64, sqlx::Error> {
    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM purchases")
        .fetch_one(&self.pool)
        .await?;
    Ok(count)
}

pub async fn file_count(&self) -> Result<i64, sqlx::Error> {
    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM files")
        .fetch_one(&self.pool)
        .await?;
    Ok(count)
}
```

Pattern to follow: see `stored_files` at `storage.rs:152-164` — it uses `sqlx::query_as` with a tuple type and `fetch_all`/`fetch_one`.

### Step 3: Create the new route handler

Create `adp-server/src/routes/stats.rs`:

```rust
use axum::extract::State;
use axum::Json;
use serde::Serialize;

use crate::error::ApiResult;
use crate::state::AppState;

#[derive(Serialize)]
pub struct StatsResponse {
    purchase_count: i64,
    file_count: i64,
}

pub async fn stats_handler(
    State(state): State<AppState>,
) -> ApiResult<Json<StatsResponse>> {
    let purchase_count = state.storage.purchase_count().await?;
    let file_count = state.storage.file_count().await?;
    Ok(Json(StatsResponse {
        purchase_count,
        file_count,
    }))
}
```

Pattern to follow: see `health.rs` (simplest handler, no auth) or `well_known.rs` (reads from state).

### Step 4: Register the route

In `adp-server/src/routes/mod.rs`:

```rust
pub(crate) mod stats;

// Inside `router()`:
.route("/stats", get(stats::stats_handler))
```

Pattern to follow: see how existing routes are registered around line 17-28.

### Step 5: Add tests

Add tests to `adp-server/tests/stats_tests.rs`:

```rust
use axum::Router;

#[tokio::test]
async fn stats_endpoint_returns_counts() {
    // Use the existing test infrastructure pattern from route_integration_tests.rs
}
```

If the test needs to insert data first, follow the pattern from `keystore_tests.rs` which uses `Storage::connect` with a tempfile.

### Step 6: Build and verify

```bash
cargo check -p adp-server --all-targets
cargo clippy -p adp-server -- -D warnings
cargo test -p adp-server -- route_integration_tests
```

### Other common additions

| If you want to... | Follow this pattern |
|---|---|
| Add a new environment variable | Edit `config.rs:Config`: add field, add env var read in `from_env`. Add to `.env.example`. |
| Add a new NIP-98-protected endpoint | Copy `verify_developer` helper from `provision.rs:149-162` — it constructs the expected URL, decodes the auth header, and calls `adp_core::verify_nip98`. |
| Add a new table | Write a new migration file (`migrations/0003_*.sql`). Add query methods to `storage.rs`. |
| Add a new event kind to the protocol | Add constants and builder/parser to the appropriate `adp-core/src/*.rs` module. Export from `lib.rs`. |
| Add a new payment proof type | Add verification function to `adp-core/src/payment.rs`. Wire it in `purchase.rs:confirm_handler`'s payment path selection (lines 134-156). |

---

## 12. Debugging Guide

### 12.1 Backend Debugging

**Log level control:**

Set `RUST_LOG` to control verbosity:

```bash
export RUST_LOG=adp_server=debug  # debug per-crate
export RUST_LOG=debug             # everything
export RUST_LOG=tower_http=trace   # axum request/response tracing (TraceLayer)
```

The `TraceLayer` middleware at `routes/mod.rs:26` logs every request method, path, status, and duration at the `info` level.

**Common errors and what they mean:**

- `"ADP_RELAYS must configure at least 2 relays"` — server startup check failed. Set at least 2 relay URLs.
- `"failed to parse ADP_OPERATOR_NSEC"` — invalid bech32 or hex secret key.
- `"ADP_MASTER_KEY must be set"` / `"ADP_MASTER_KEY must decode to exactly 32 bytes"` — the key is exactly 64 hex characters.
- `"payment proof already used"` (HTTP 409) — a zap receipt event id or preimage has been reused. This is correct replay protection.
- `"this server is not named in the listing's server tags"` (HTTP 451) — the listing's `server` tags don't include this server's URL (as configured in `ADP_SERVER_URL`).
- `"no held key currently authorized for this listing"` (HTTP 500) — the operator holds a key, but the listing's delegation tag is inactive (revoked or not yet valid) for the listing.

### 12.2 Generating Test Invoices

The `gen_test_invoice` example generates a bolt11 invoice + preimage pair:

```bash
cargo run -p adp-server --example gen_test_invoice
```

Output:
```
bolt11: lnbct1...
preimage: a1b2c3...
```

These can be used in `curl` calls to `POST /purchase/confirm` with the bolt11/preimage payment path.

### 12.3 Known Gotchas

1. **The README is partially stale.** It mentions "verify_zap_receipt does not resolve the developer's lud16" and "purchase/confirm hardcodes amount: '0'" — both of these have been fixed but the README wasn't updated. Use `CODEBASE.md` (this document) or read the actual code instead.

2. **Relay completeness is fail-closed.** Each lookup requires at least two successful relay queries and matching EOSE before it can report absence or return a complete collection. A disconnected or timed-out relay produces `RelayLookup::Unavailable`; callers must surface that as an unavailable dependency rather than treating it as “not found.”

3. **Master key is never logged**, but if a panic occurs in `decode_master_key()` (at startup, not during runtime), the `.expect()` message mentions the env var name `ADP_MASTER_KEY` but not the value. The value is hex-decoded into `[u8; 32]` which has no `Display` impl. Safe, but worth knowing.

4. **`parse_decimal_units` conservatism.** The price parser at `purchase.rs:46-58` returns `None` for amounts with more decimal digits than `max_decimals` (8 for BTC, 0 for SATS). This is fail-closed (correct for payment verification), but edge cases like `"0.99999999999"` for BTC or extremely large SATS values are not covered by tests.

5. **`kind:30404` publish on revoke not verified by integration test.** The route integration test calls revoke and checks HTTP 200, then checks that the next confirm fails. It does NOT assert that a `kind:30404` with `revoked_at` was published to the mock relay. The code path builds and publishes it, but this gap exists.

6. **The `resolve_lud16_lsp_pubkey` hardcodes `https://`** except for loopback domains when `ADP_ALLOW_INSECURE_LNURL=true`. If you're testing zap receipts locally without TLS, you need `ADP_ALLOW_INSECURE_LNURL=true` and a local LNURL endpoint on localhost.

7. **sqlx migration path.** The `sqlx::migrate!("../migrations")` call in `storage.rs:29` uses a relative path from `adp-server/src/`. This works during `cargo run -p adp-server` (which runs from the workspace root), but may need adjustment if the working directory is different.

8. **Zero price is not access policy.** Public or timed access must be explicit in the listing's `acquisition` tags. A zero-priced listing without such a policy still requires a valid token, grant, or receipt.

---

## 13. Glossary

| Term | Definition |
|---|---|
| **ADP-01** | Arcadestr Distribution Protocol — the HTTP protocol spec this server implements. See `ADP-01.md`. |
| **ADP Server** | An HTTP server implementing this protocol. |
| **NIP-99** | Nostr Implementation Possibility 99 — classified listings event kind (`30402`). Games are listed via NIP-99. |
| **NIP-102** | Nostr Implementation Possibility 102 — marketplace receipt event kind (`1020`). Used to prove purchase. |
| **NIP-57** | Nostr Implementation Possibility 57 — Lightning Network zap receipts (`Kind:9735`). Payment proof option. |
| **NIP-98** | HTTP Auth via Nostr event — a `kind:27235` event in an `Authorization: Nostr <base64>` header that proves the caller controls a given Nostr key. |
| **Game coordinate** | `30402:<developer-pubkey>:<d-tag>` — the stable identifier for a specific game listing. |
| **Fulfillment key** | A Nostr keypair the developer delegates (via a listing tag) to sign purchase receipts on their behalf. |
| **Operator** | The party running an ADP server. Identified by the pubkey that signs `kind:30403` announcements and `kind:30404` attestations. |
| **Developer** | The Nostr keypair that signed the game's `kind:30402` listing event. |
| **LSP** | Lightning Service Provider — the LNURL-pay endpoint that signs `kind:9735` zap receipts on the developer's behalf. |
| **Scope** | An optional label a developer attaches to a provisioning request, letting the same operator hold multiple independent fulfillment keys for the same developer (e.g. one per game). |
| **Provisioning attestation** | `kind:30404` event published by the operator attesting that `fulfillment_pubkey` is authorized to act on behalf of the developer for the given scope. |
| **Fulfillment authorization** | Regular developer-signed `kind:30406` root with explicit capabilities and prospective cancellation. |
| **Receipt** | `kind:1020` NIP-102 receipt confirming a purchase. Signed by the developer or a delegated fulfillment key. |
| **Download token** | Short-lived UUID issued by `/purchase/confirm`. Enables fast Path A downloads without relay queries. |
| **Access basis** | Ordered authorization basis: local token, portable grant, portable receipt, explicit public policy, or active timed-access policy. |
| **Entitlement Grant** | Provisional `kind:1030` durable credential addressed to a buyer and game coordinate. |
| **ADP campaign** | Provisional `kind:1031` publisher-controlled eligibility and cancellation chain used by `/entitlement/claim`. |
| **Distribution authorization** | The check that a server is named in the listing's `server` tags. Independent of ownership proof. HTTP 451 if this check fails. |
| **`kind:30403`** | Server announcement event — how clients discover ADP server. |
| **`fulfillment_authorization` tag** | Repeatable listing entry containing an exact `kind:30406` root ID, fulfillment pubkey, and optional relay hint. |
| **`server` tag** | Listing tag naming an authorized distributor server URL. Repeatable. |
| **`file_hash` tag** | Listing tag with the SHA-256 hex of the game archive. Used for upload and download integrity verification. |
| **LUD16** | Lightning address in the form `name@domain`. Stored in the listing's `lud16` tag. |
| **ETag** | HTTP entity tag header. Used by the download handler for conditional responses (`304 Not Modified`). Valued is `"{sha256_hex}"`. |
