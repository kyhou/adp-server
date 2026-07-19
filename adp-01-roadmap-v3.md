# ADP Server Implementation Roadmap (v3)

**Supersedes `adp-01-roadmap-v2.md` entirely.** The pivot to multi-tenant hosting
(one operator, many developers, distinct fulfillment keys) is a foundational
architecture change, not an incremental feature — it touches the same files most
of v2's Phase 1 bug fixes targeted (`purchase.rs`'s signing step, `config.rs`,
`state.rs`, `well_known.rs`). Fixing those bugs against code that's about to be
rewritten would be wasted work, so this roadmap reorders around that reality:
**the provisioning system is now Phase 1, ahead of the correctness fixes v2 had
first.** Single-tenant / self-hosted is no longer a distinct mode — per the
decision behind this pivot, a developer self-hosting is just an operator with
exactly one tenant, going through the same provisioning flow as anyone else;
direct signing (`E_1020.pubkey == listing.pubkey`) remains available as a concept
but requires no `adp-server` code path at all, since it means the developer signs
receipts themselves, outside this codebase entirely.

Read alongside the current `ADP-01.md` — every endpoint, event kind, and formula
referenced below is defined there in full; this document only sequences the work.

---

## Phase 0: Client-side prerequisite — unchanged from v2, still gates Phase 2's NEW-LSP

`lud16` must be resolved identically by Arcadestr's client (when requesting the
buyer's invoice) and by `adp-server` (when verifying the zap receipt's LSP). This
phase lives in the *Arcadestr* repo, not `adp-server`.

| # | Priority | Task | Files |
|---|----------|------|-------|
| 0.1 | **BLOCKER** | Audit `GameListing.lud16`'s source in `core/src/marketplace.rs` — confirm it reads the listing's own `lud16` tag, never a `kind:0` profile fallback. Fix if it doesn't. | `core/src/marketplace.rs` |
| 0.2 | HIGH | Add `lud16` as a mandatory publish-form field for any priced listing. | `app/src/ui_v2/views/publish.rs` |
| 0.3 | HIGH | Add `file_hash` (client-computed), `server`, `fulfillment_pubkey` to the publish form and `GameListing`; missing fields render Buy-only, no Install. | `app/src/models.rs`, `app/src/ui_v2/views/publish.rs`, `core/src/nostr.rs` |
| 0.4 | HIGH | Parser reads `fulfillment_pubkey`; drop any `lud16` profile fallback. Keep in sync with `adp-core::AdpListing::from_event`. | `core/src/marketplace.rs` |

**Exit criteria:** publish → buy → verify round-trip works end-to-end with fields sourced identically on both sides.

---

## Phase 1: Multi-tenant key provisioning — foundational, do this first

Everything here is new; none of it exists in the current reference implementation,
which assumes exactly one fulfillment key loaded from `ADP_FULFILLMENT_NSEC` at
startup. That assumption is being removed, not extended.

### 1.1 Encrypted key storage

New table and module. Keys are stored per `(developer_pubkey, scope)`, mirroring
`ADP-01.md`'s per-game isolation design.

```sql
-- migrations/0002_provisioning.sql
CREATE TABLE IF NOT EXISTS fulfillment_keys (
    id                    INTEGER PRIMARY KEY AUTOINCREMENT,
    developer_pubkey      TEXT NOT NULL,
    scope                 TEXT NOT NULL DEFAULT '',
    fulfillment_pubkey    TEXT NOT NULL UNIQUE,
    encrypted_privkey     BLOB NOT NULL,
    encryption_nonce      BLOB NOT NULL,
    attestation_event_id  TEXT NOT NULL,
    valid_from            INTEGER NOT NULL,
    revoked_at            INTEGER,
    created_at            INTEGER NOT NULL,
    UNIQUE(developer_pubkey, scope)
);

CREATE INDEX IF NOT EXISTS idx_fulfillment_keys_developer ON fulfillment_keys(developer_pubkey);
```

New module `adp-server/src/keystore.rs`:

```rust
pub struct KeyStore { pool: SqlitePool, master_key: [u8; 32] }

impl KeyStore {
    /// Idempotent: returns the existing active key for (developer_pubkey, scope)
    /// if one exists, otherwise generates, encrypts, stores, and returns a new one.
    pub async fn provision(&self, developer_pubkey: &str, scope: &str)
        -> Result<(Keys, bool /* is_new */), KeyStoreError>;

    /// Looks up the private key for a specific fulfillment pubkey — this is what
    /// confirm_handler calls, not a developer-keyed lookup, since a listing's
    /// fulfillment_pubkey tag is what confirm_handler actually has in hand.
    pub async fn key_for_fulfillment_pubkey(&self, fulfillment_pubkey: &str)
        -> Result<Option<Keys>, KeyStoreError>;

    pub async fn revoke(&self, developer_pubkey: &str, fulfillment_pubkey: &str)
        -> Result<(), KeyStoreError>;
}
```

Encryption: AES-256-GCM under a server-wide master key, mirroring the pattern in
Arcadestr's own `core/src/storage/encryption.rs` rather than inventing a new
approach. Master key comes from a new `ADP_MASTER_KEY` env var (raw 32-byte hex,
or derived via the same scrypt/argon2 path `core::storage::encryption` already
uses if you want passphrase-based setup instead of a raw key file). **Explicitly
document in the README that this is a reference-implementation baseline — an
operator running this at real scale should be evaluating an HSM or KMS**, per
`ADP-01.md`'s Security Considerations note on this exact point.

### 1.2 `POST /provision`

New `adp-server/src/routes/provision.rs`. Implements `ADP-01.md`'s
`POST /provision` exactly:

1. Verify NIP‑98 token → developer pubkey.
2. Parse optional `scope` from body (default `""`).
3. Call `keystore.provision(developer_pubkey, scope)`.
4. If newly created: build and publish a `kind:30404` attestation (`d` =
   `<developer_pubkey>:<fulfillment_pubkey>`, `p` = developer_pubkey,
   `fulfillment_pubkey` tag with `valid_from` = now, optional `scope` tag).
5. Return `{ fulfillment_pubkey, attestation_event_id, scope }`.

Add a new `adp-core` module, `adp-core/src/provisioning.rs`, for building/parsing
`kind:30404` and `kind:30406` events — mirror `receipt.rs`'s pattern
(`build_*_event_builder` functions returning `EventBuilder`, plus `parse_*_event`
functions), rather than constructing tags inline in the route handler.

### 1.3 `POST /provision/revoke`

New handler in the same file. NIP‑98 verified, confirms the calling developer owns
the named key per `keystore` records, republishes `kind:30404` with `revoked_at` =
now, calls `keystore.revoke(...)`. Per `ADP-01.md`, this is an audit/hygiene
endpoint — it does not itself invalidate any receipt; document that in a comment
so a future maintainer doesn't mistake it for the security-critical check (that's
still the listing's own `fulfillment_pubkey` tag).

### 1.4 Rewrite `confirm_handler`'s signing step

Currently signs with a single `state.fulfillment_keys`. Replace with:

1. Iterate `listing.fulfillment_delegations`, applying the backdating clamp
   (§1.6) to find which delegated pubkeys are currently authorized.
2. For each currently-authorized pubkey, call
   `state.keystore.key_for_fulfillment_pubkey(pubkey)`.
3. Sign with whichever one this server actually holds. If none, return `500` —
   "this server does not hold a currently-authorized fulfillment key for this
   listing" (same error class as v2's old single-key misconfiguration case, now
   phrased for the multi-key world).

### 1.5 Server identity vs. fulfillment keys — separate concerns now

With no single static fulfillment key, `well_known.rs` and `kind:30403`
publishing need their own identity, independent of any developer's fulfillment
key. Add `ADP_OPERATOR_NSEC` (rename from the old `ADP_SERVER_NSEC` idea) as the
**one** static key this server config still needs — it signs the server's own
`kind:30403` announcement and appears in `/.well-known/adp`'s `pubkey` field.
Remove `ADP_FULFILLMENT_NSEC` and `AppState.fulfillment_keys` entirely; they no
longer mean anything in this architecture.

### 1.6 Backdating clamp

In `adp-core`:

```rust
// listing.rs, on FulfillmentDelegation
pub fn authorizes_at(&self, at: u64, listing_created_at: u64) -> bool {
    let effective_revoked_at = self.revoked_at.map(|r| r.max(listing_created_at));
    self.valid_from <= at && effective_revoked_at.map_or(true, |r| r > at)
}
```

Thread `listing.event.created_at` through every call site — `delegation.rs`'s
`verify_signer_authorization`, and anywhere `authorizes_at` is called directly
(`upload.rs`'s uploader-authorization check, `purchase.rs`'s server-authorization
check). Add a unit test in `adp-core/tests/delegation_tests.rs` asserting a
`revoked_at` older than the listing's own `created_at` is clamped, not trusted
raw — this is the regression test for exactly the backdating scenario
`ADP-01.md`'s Validity Interval section describes.

### 1.7 Multi-relay quorum — confirm, don't assume

`relay.rs`'s `fetch_listing` already does `events.into_iter().max_by_key(|e|
e.created_at.as_u64())` across whatever `nostr_sdk::Client` returns — and since
the client is configured with the full relay pool, this **may already satisfy**
`ADP-01.md`'s "query ≥2 relays, take highest `created_at`" requirement, given a
config with 2+ relays. Don't assume this — write an integration test against a
real (or locally-run) multi-relay setup that publishes conflicting versions of
the same listing to different relays and confirms the highest-`created_at` one
wins. If it doesn't hold up in practice, that's a real bug to fix here, not later.
Separately, add a startup check in `config.rs`/`main.rs` refusing to start with
fewer than 2 configured relays, since a single-relay deployment can't satisfy this
requirement no matter what the code does.

**Phase 1 exit criteria:**
```bash
cargo build --workspace && cargo clippy --workspace -- -D warnings && cargo test --workspace
```
Plus a manual round trip: provision a key for a test developer, publish a listing
delegating to it, confirm a purchase, confirm the receipt is signed by the
provisioned key and validates.

---

## Phase 2: Correctness fixes (former v2 Phase 1 items, done against the new architecture)

These are the same underlying bugs v2 identified — do them now that Phase 1 has
settled what `purchase.rs` actually looks like, not before.

| # | Priority | Task | Sequencing note |
|---|----------|------|------------------|
| 1 | **BLOCKER** | **Test-first.** Fix `fetch_receipts`'s relay filter — confirm empirically whether `.pubkey(buyer_pubkey)` filters `#p` or `.author()`; correct to `#p`. Path B returns zero receipts for legitimate buyers if wrong. | Independent of Phase 1's changes; can be done in parallel with 1.1–1.3 if you want to parallelize, but must land before Phase 2's other items are considered "done." |
| NEW-LSP | **BLOCKER** | NIP-57 LSP verification per `ADP-01.md`'s LSP Verification section — resolve `lud16` from the listing tag, fetch the LNURL-pay endpoint's `nostrPubkey`, require the zap receipt's signer match it. **Blocked by Phase 0.** | New `adp-server/src/lnurl.rs`; `adp-core/src/payment.rs`'s `verify_zap_receipt` takes the resolved pubkey as a required parameter. |
| 2 | CRITICAL | Fix `ApiError::Protocol`'s blanket 400 mapping — audit actual reachable variants via `grep -n '?'` across the three handlers, not the enum definition. Confirmed mis-mapped: `Nip98` (→401), `Encryption`/`Nostr`/`Serde` (→500). `InvalidPaymentProof` is already correct (explicit `.map_err`). `UnauthorizedSigner`/`RevokedAtSigningTime` are unreachable via `?` — don't add dead match arms. | Independent. |
| 4 → 3 | HIGH | Resolve listing price from NIP-99 `price` tags (`AdpListing.price: Option<(String, String)>`), then wire real amount verification into `verify_bolt11_preimage`. Only pass a real `expected_amount_msat` when the currency is sats/BTC-denominated; skip (don't fiat-convert) otherwise. | #4 before #3. |

**Phase 2 exit criteria:** same as Phase 1, plus `cargo test -p adp-core` showing the
delegation test suite (now including the clamp test from 1.6) green.

---

## Phase 3: Remaining upload/file correctness

| # | Priority | Task |
|---|----------|------|
| 5 | HIGH | Make `file_hash` required at `/upload` time (currently `Option`, silently skipped; `game.rs` unconditionally 404s without one — the two paths disagree). While here: update the uploader-authorization check to use `keystore.key_for_fulfillment_pubkey` / the clamped `authorizes_at` from 1.6, since the old single-key check no longer applies. |

---

## Phase 4: Missing spec features (kind:30403 itself already lands in Phase 1.5)

| # | Priority | Task |
|---|----------|------|
| 7 | MEDIUM | Conditional GET (`304`) via `ETag` from `file_hash`. Depends on #5. |
| 8 | LOW | Periodic file hash re-verification via background `tokio::spawn`. |

---

## Phase 5: Test coverage

| # | Priority | Task |
|---|----------|------|
| 9 | HIGH | Pure function tests: `verify_nip98`, `verify_bolt11_preimage`, `verify_zap_receipt` (including LSP mismatch), `latest_status_in_chain`, `parse_coordinate`, `ApiError::into_response` mapping, **the clamp from 1.6**, **`kind:30404`/`kind:30406` build/parse round-trips**. |
| 10 | MEDIUM | Storage tests: `Storage` CRUD + `UNIQUE` constraints, **plus `KeyStore`: provision idempotency, encryption round-trip, revoke behavior, and that `key_for_fulfillment_pubkey` never returns a key for a different developer's scope.** |
| 11 | LOW | Route integration tests (`tower::ServiceExt::oneshot`, in-memory SQLite): purchase golden path, replay rejection, no-authorized-key-held case, distribution-unauthorized (`451`), **provision → confirm → revoke → confirm-fails-after-revoke end-to-end.** |

---

## Phase 6: DevOps — same trimmed scope as v2

| # | Priority | Task | Status |
|---|----------|------|--------|
| 13 | HIGH | Health endpoints + graceful shutdown | Do now |
| 15 | HIGH | Error path logging in all handlers | Do now, after Phase 1/2 land in these files |
| 16 | MEDIUM | CI (check/test/clippy) | Do now — cheap, matches the existing gate-discipline workflow |
| 14 | MEDIUM | Request logging + upload size limits | Do now, bundle with 15 |
| 14a | MEDIUM | CORS | Validate need first (only if adp-server is ever called directly from browser JS, not just via the Tauri bridge) |
| 12, 17–19 | LOW — deferred | Docker, docker-compose, metrics, k8s | Defer until a deployment target exists |

---

## Recommended execution order

1. **Phase 0** (client repo) — confirmed/fixed before anything touching LSP verification starts.
2. **Phase 1** in full — this is the foundational rewrite; everything after it assumes it's done.
3. **Phase 2**, in this sub-order: #1 (test-first) and NEW-LSP in parallel → #2 → #4 → #3.
4. **Phase 3** (#5).
5. **Phase 5, items 9–10** alongside each phase above as its code lands — don't defer testing the clamp, the keystore, or the provisioning endpoints to a separate later pass.
6. **Phase 4** (#7, #8) once Phases 1–3 are green.
7. **Phase 6's "do now" items** alongside Phase 4.
8. **Phase 5, item 11** (full route integration) last.
9. Phase 6's deferred items: revisit once install/download and trending games are further along.

**Effort estimate:** Phase 1 (the provisioning system) is the largest single addition
in this project so far — encrypted key storage, two new endpoints, a rewritten
signing path, and the clamp — call it 18–25 hours on its own. Phases 2–5 are
roughly the same size as v2 estimated (~25–33 hours). Total: **43–58 hours**,
meaningfully more than v2's 33–43, because this is now a genuinely bigger system
than the single-tenant reference server was.
