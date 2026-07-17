# Entitlement Grants and Free Acquisitions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement ADP 0.3 explicit acquisition policy, developer-controlled campaigns and delegated authorization lifecycles, durable Entitlement Grants and anchored receipts, idempotent claims, and portable download authorization.

**Architecture:** Pure event parsing, chain resolution, and authority evaluation stay in `adp-core`; per-relay completeness and chain retrieval stay in `adp-server::relay`; durable local state stays in `Storage`; routes only orchestrate these layers. Shared contracts land before parallel work, after which independent workstreams own disjoint files and Sol reconciles all changes.

**Tech Stack:** Rust 2021, `nostr`/`nostr-sdk` 0.44, Axum 0.7, SQLx 0.8 with SQLite, Tokio, Serde, UUID, existing NIP-44 and NIP-98 helpers.

---

## Worktree and Existing Changes

Work in the current repository because the authoritative documents and user changes are untracked or modified there. Do not revert or overwrite the existing changes in:

- `adp-server/src/lnurl.rs`
- `adp-server/src/routes/game.rs`
- `adp-server/tests/multi_relay_freshness_tests.rs`

The `game.rs` zero-price addition is part of the current diff but conflicts with the approved protocol; remove only that behavior while retaining unrelated formatting or test changes.

## File Map

### New files

- `adp-core/src/kinds.rs`: provisional kinds and shared ADP version.
- `adp-core/src/chain.rs`: shared chain-resolution error vocabulary.
- `adp-core/src/campaign.rs`: campaign transitions, parsing, resolution, and effective state.
- `adp-core/src/entitlement.rs`: generic Entitlement Grant builder, parser, and topology.
- `adp-core/src/entitlement_adp.rs`: ADP campaign and signer authority for grants.
- `adp-core/tests/campaign_tests.rs`: campaign protocol matrix.
- `adp-core/tests/authorization_tests.rs`: kind 30406 authorization lifecycle matrix.
- `adp-core/tests/entitlement_tests.rs`: generic and ADP grant matrix.
- `adp-core/tests/receipt_authorization_tests.rs`: ADP receipt-anchor matrix.
- `migrations/0003_entitlements.sql`: claims, receipt profile classification, and token source metadata.
- `adp-server/src/ownership.rs`: independent grant/receipt credential checks and access result types.
- `adp-server/src/routes/entitlement.rs`: `POST /entitlement/claim`.
- `adp-server/tests/claim_route_tests.rs`: claim route and publication-state integration tests.
- `adp-server/tests/download_authorization_tests.rs`: complete access precedence matrix.
- `adp-server/tests/entitlement_storage_tests.rs`: migration and persistent-integrity tests.

### Modified files

- `adp-core/src/lib.rs`: module declarations and public exports.
- `adp-core/src/error.rs`: precise parse/chain/authority errors.
- `adp-core/src/listing.rs`: exact `d`, acquisition policy, pointers.
- `adp-core/src/provisioning.rs`: kind 30406 authorization lifecycle.
- `adp-core/src/receipt.rs`: authorization anchor and complete chain validation.
- `adp-core/src/delegation.rs`: retain current-listing checks only; remove portable-verification claims.
- `adp-core/tests/protocol_regression_tests.rs`: listing and centralized-kind regressions.
- `adp-server/src/relay.rs`: typed lookup completeness, per-relay queries, filters, chain completion.
- `adp-server/src/storage.rs`: claims, profile-3 receipts, attributed tokens, integrity checks.
- `adp-server/src/error.rs`: typed 503 mapping.
- `adp-server/src/lib.rs`: ownership module.
- `adp-server/src/routes/mod.rs`: entitlement route registration.
- `adp-server/src/routes/purchase.rs`: current plus 30406 issuance checks and anchored receipt persistence.
- `adp-server/src/routes/game.rs`: `AccessBasis` orchestration and removal of zero-price inference.
- `adp-server/src/routes/well_known.rs`: stable event-kind metadata.
- `adp-server/src/announcement.rs`: shared ADP version.
- Every `Relay` test double: new methods and `RelayLookup` return values.
- `.env.example`, `README.md`, `CODEBASE.md`, `ADP-01.md`, `NIP-103.md`, and the v3 amendment: protocol documentation.

## Parallelization Gates

1. Complete Task 1 before dispatching any implementation workstream.
2. After Task 1, Tasks 2-5 can run in parallel with one owner per core file.
3. Task 6 depends on Tasks 3-5.
4. Task 7 depends on Task 4.
5. Tasks 8 and 9 can run in parallel after Task 1.
6. Task 10 depends on Tasks 4, 7, 8, and 9.
7. Task 11 depends on Tasks 2, 3, 4, 6, 7, 8, and 9.
8. Tasks 12-14 run after functional integration.

Sol owns Task 1, integration, review, and the final verification gate. Dispatch
substantial independent Tasks 2-9 and 14 to GPT-5.6 Terra agents with the exact
file ownership shown below. Use GPT-5.6 Luna only for one bounded migration or
mechanical test/document correction. Agents must report changed files, commands,
results, assumptions, and overlap; Sol inspects every diff before integration.

### Task 1: Freeze Shared Contracts

**Files:**
- Create: `adp-core/src/kinds.rs`
- Create: `adp-core/src/chain.rs`
- Create: `adp-core/src/campaign.rs`
- Create: `adp-core/src/entitlement.rs`
- Create: `adp-core/src/entitlement_adp.rs`
- Modify: `adp-core/src/listing.rs`
- Modify: `adp-core/src/provisioning.rs`
- Create: `adp-server/src/ownership.rs`
- Modify: `adp-core/src/lib.rs`
- Modify: `adp-core/src/error.rs`
- Modify: `adp-server/src/lib.rs`
- Modify: `adp-server/src/relay.rs`

- [ ] **Step 1: Add a compile-time public-contract test**

Create `adp-core/tests/public_contract_tests.rs` with imports and constant assertions:

```rust
use adp_core::{
    AcquisitionPolicy, AuthorizationTransition, CampaignTransition,
    ChainResolutionError, GrantStatus, ADP_VERSION,
    EXPERIMENTAL_ADP_CAMPAIGN_KIND, EXPERIMENTAL_ENTITLEMENT_GRANT_KIND,
};

#[test]
fn protocol_constants_are_centralized_and_provisional() {
    assert_eq!(EXPERIMENTAL_ENTITLEMENT_GRANT_KIND, 1030);
    assert_eq!(EXPERIMENTAL_ADP_CAMPAIGN_KIND, 1031);
    assert_ne!(EXPERIMENTAL_ENTITLEMENT_GRANT_KIND, 1021);
    assert_ne!(EXPERIMENTAL_ADP_CAMPAIGN_KIND, 1021);
    assert_ne!(EXPERIMENTAL_ENTITLEMENT_GRANT_KIND, EXPERIMENTAL_ADP_CAMPAIGN_KIND);
    assert_eq!(ADP_VERSION, "0.3.0");
}
```

- [ ] **Step 2: Run the contract test and verify it fails**

Run: `cargo test -p adp-core --test public_contract_tests`

Expected: compilation fails because the shared exports do not exist.

- [ ] **Step 3: Define the shared constants and chain errors**

Use these exact constants in `kinds.rs`:

```rust
pub const EXPERIMENTAL_ENTITLEMENT_GRANT_KIND: u16 = 1030;
pub const EXPERIMENTAL_ADP_CAMPAIGN_KIND: u16 = 1031;
pub const ADP_VERSION: &str = "0.3.0";
```

Define `ChainResolutionError` in `chain.rs` with variants `Empty`, `MissingPredecessor(EventId)`, `MultipleRoots`, `Fork(EventId)`, `Cycle`, `Disconnected`, `InvariantMutation(&'static str)`, `InvalidTransition(String)`, `TimestampRegression`, and `LimitExceeded`.

- [ ] **Step 4: Define final public type shapes without behavior**

Declare these protocol contracts with `Debug`, `Clone`, `PartialEq`, and `Eq`
where their contained Nostr types support those derives:

```rust
pub enum CampaignMode { Claim }
pub struct CampaignTerms { pub mode: CampaignMode, pub starts: u64, pub ends: u64 }
pub enum CampaignTransition { Root(CampaignTerms), ReplaceTerms(CampaignTerms), Cancel }
pub enum CampaignStatus { Active, Cancelled }
pub struct CampaignEvent {
    pub event: Event,
    pub campaign_id: String,
    pub coordinate: String,
    pub predecessor: Option<EventId>,
    pub transition: CampaignTransition,
}
pub struct ResolvedCampaign {
    pub root_event_id: EventId,
    pub developer_pubkey: PublicKey,
    pub campaign_id: String,
    pub coordinate: String,
    pub events: Vec<CampaignEvent>,
}

pub enum GrantStatus { Granted, Revoked }
pub struct ParsedEntitlementGrant {
    pub event: Event,
    pub grant_id: String,
    pub recipient: PublicKey,
    pub coordinate: String,
    pub source_event: Option<EventId>,
    pub authorization_event: Option<EventId>,
    pub reason: Option<String>,
    pub status: GrantStatus,
    pub predecessor: Option<EventId>,
}
pub struct ResolvedEntitlementGrant {
    pub root_event_id: EventId,
    pub events: Vec<ParsedEntitlementGrant>,
}

pub struct AuthorizationTerms {
    pub authorization_id: String,
    pub coordinate: String,
    pub fulfillment_pubkey: PublicKey,
    pub valid_from: u64,
    pub valid_until: Option<u64>,
}
pub enum AuthorizationTransition { ActiveRoot(AuthorizationTerms), Revoke }
pub struct AuthorizationEvent {
    pub event: Event,
    pub transition_d: String,
    pub authorization_id: String,
    pub coordinate: String,
    pub fulfillment_pubkey: PublicKey,
    pub predecessor: Option<EventId>,
    pub transition: AuthorizationTransition,
}
pub struct ResolvedAuthorization {
    pub root_event_id: EventId,
    pub developer_pubkey: PublicKey,
    pub terms: AuthorizationTerms,
    pub events: Vec<AuthorizationEvent>,
    pub revoked_at: Option<u64>,
}
```

Use `pub(crate)` only for fields not listed above. Later tasks may add private
indexes but must not rename these shared fields.

Define `AcquisitionPolicy` and `CampaignPointer` in `listing.rs` during this gate,
but defer tag parsing to Task 2. Define the authorization types in
`provisioning.rs`; Task 4 adds builders, parsers, and lifecycle behavior.

The server `relay.rs` contract is:

```rust
pub enum RelayLookup<T> {
    Found(T),
    ConfirmedAbsent { successful_relays: usize },
    Unavailable { successful_relays: usize, failed_relays: usize },
}
```

The server `ownership.rs` contract is:

```rust
pub enum AccessBasis {
    DownloadToken,
    EntitlementGrant,
    PurchaseReceipt,
    PublicPolicy,
    TimedAccessPolicy,
}

pub enum CredentialSource { Entitlements, Receipts }

pub enum CredentialCheck {
    Valid(AccessBasis),
    Absent,
    Invalid { candidate_count: usize },
    Unavailable { source: CredentialSource },
}
```

- [ ] **Step 5: Export contracts and run checks**

Update `adp-core/src/lib.rs` and `adp-server/src/lib.rs`, then run:

```bash
cargo fmt --all
cargo check -p adp-core --all-targets
cargo check -p adp-server --all-targets
```

Expected: both checks pass and no function contains a placeholder macro or forced panic.

- [ ] **Step 6: Commit the contract gate**

```bash
git add adp-core/src adp-core/tests/public_contract_tests.rs adp-server/src/lib.rs adp-server/src/ownership.rs adp-server/src/relay.rs
git commit -m "feat(core): define ADP 0.3 protocol contracts"
```

### Task 2: Parse Listing Acquisition and Campaign Pointers

**Files:**
- Modify: `adp-core/src/listing.rs`
- Test: `adp-core/tests/protocol_regression_tests.rs`

- [ ] **Step 1: Add failing listing tests**

Add tests named:

```rust
acquisition_absent_is_gated
acquisition_public_is_explicit
timed_access_uses_half_open_interval
duplicate_acquisition_is_invalid
malformed_timed_access_is_invalid
zero_price_without_acquisition_is_gated
campaign_pointer_accepts_event_id_without_hint
campaign_pointer_accepts_wss_hint
campaign_pointer_keeps_id_when_hint_is_malformed
campaign_pointer_discards_ws_hint
campaign_pointer_discards_malformed_event_id
duplicate_d_tag_is_rejected
```

Build signed kind 30402 fixtures with `Tag::parse`, then assert `AcquisitionPolicy::allows_at` and typed pointer fields.

- [ ] **Step 2: Verify tests fail**

Run: `cargo test -p adp-core --test protocol_regression_tests acquisition`

Expected: failure because `AdpListing` has no acquisition policy.

- [ ] **Step 3: Implement exact fail-closed parsing**

Add `acquisition_policy: AcquisitionPolicy` and `campaign_pointers: Vec<CampaignPointer>` to `AdpListing`. Count `d` and acquisition tags rather than overwriting values. `CampaignPointer` parses `EventId`; parse a hint as `RelayUrl` only when its scheme is `wss`. A malformed hint sets `None`, while a malformed event ID discards only that pointer.

Implement:

```rust
impl AcquisitionPolicy {
    pub fn allows_at(&self, now: u64) -> bool {
        match self {
            Self::Public => true,
            Self::TimedAccess { starts, ends } => *starts <= now && now < *ends,
            Self::Gated | Self::Invalid => false,
        }
    }
}
```

- [ ] **Step 4: Run focused and crate tests**

Run:

```bash
cargo test -p adp-core --test protocol_regression_tests
cargo test -p adp-core
```

Expected: all tests pass; malformed policy remains distinguishable from gated policy.

- [ ] **Step 5: Commit**

```bash
git add adp-core/src/listing.rs adp-core/tests/protocol_regression_tests.rs
git commit -m "feat(core): parse explicit acquisition policy"
```

### Task 3: Implement Campaign Protocol

**Files:**
- Modify: `adp-core/src/campaign.rs`
- Modify: `adp-core/src/error.rs`
- Create: `adp-core/tests/campaign_tests.rs`

- [ ] **Step 1: Write the campaign matrix as failing tests**

Use deterministic `Timestamp::from_secs` fixtures and add the required tests: valid root; wrong signer; coordinate signer mismatch; `starts < created_at`; malformed interval; exact start/end boundaries; valid pre-start replacement; replacement at old start; `new_starts <= update.created_at`; post-start term edit; valid cancellation; exact cancellation boundary; grant time before/at cancellation; fork; cycle; missing predecessor; changed `d`; changed `a`; and timestamp regression.

The root rejection assertion is:

```rust
assert!(matches!(
    resolve_campaign(&[root], developer.public_key(), &coordinate),
    Err(ChainResolutionError::InvalidTransition(_))
));
```

- [ ] **Step 2: Verify the matrix fails**

Run: `cargo test -p adp-core --test campaign_tests`

Expected: compile or assertion failures because campaign behavior is not implemented.

- [ ] **Step 3: Implement parsing and linear topology**

Implement `parse_campaign_event`, `resolve_campaign`, and `ResolvedCampaign::state_at`. Enforce exact cardinality, signature, kind 1031, developer author, one root, one successor per event, complete connected traversal, strict timestamp increase, invariant `d`/`a`, and typed `Root`, `ReplaceTerms`, and `Cancel` transitions.

- [ ] **Step 4: Implement effective state and claimability**

Implement:

```rust
impl ResolvedCampaign {
    pub fn is_claimable_at(&self, at: u64) -> bool {
        let state = self.state_at(at);
        state.status == CampaignStatus::Active
            && state.terms.starts <= at
            && at < state.terms.ends
    }
}
```

Use chain order only; timestamps determine whether an already-resolved transition is effective, never which fork wins.

For cycle/self-reference branch coverage, parse valid signed fixtures first and
then alter only the public parsed-node predecessor fields before calling the pure
resolver. Keep separate parser tests proving signed tags map to those fields; do
not weaken signature checks to manufacture impossible event-ID fixed points.

- [ ] **Step 5: Run tests and commit**

```bash
cargo test -p adp-core --test campaign_tests
cargo test -p adp-core
git add adp-core/src/campaign.rs adp-core/src/error.rs adp-core/tests/campaign_tests.rs
git commit -m "feat(core): validate ADP campaign chains"
```

### Task 4: Extend Kind 30406 into Authorization Lifecycles

**Files:**
- Modify: `adp-core/src/provisioning.rs`
- Create: `adp-core/tests/authorization_tests.rs`
- Modify: `adp-core/src/lib.rs`

- [ ] **Step 1: Add failing lifecycle tests**

Cover active root, unique transition `d`, duplicate tags, wrong developer, wrong coordinate, wrong fulfillment key, valid-from boundary, valid-until boundary, `valid_until <= valid_from`, terminal revocation, exact revocation time, timestamp regression, reused `(kind, developer, d)`, missing predecessor, fork, cycle, disconnected transition, self-reference, and invariant mutation.

Include:

```rust
assert!(authorization.authorizes_at(root.created_at.as_secs()));
assert!(!authorization.authorizes_at(revocation.created_at.as_secs()));
```

- [ ] **Step 2: Verify failures**

Run: `cargo test -p adp-core --test authorization_tests`

Expected: failures because existing 30406 accepts operator/key pairs and has no lifecycle.

- [ ] **Step 3: Implement the new 30406 wire shape**

Root tags are exactly one each of `d`, `authorization_id`, `a`, `p`, `valid_from`, and `status: active`; zero or one `valid_until`; no `e`. Revocation tags are exactly one new `d`, preserved `authorization_id`, `a`, `p`, `status: revoked`, and predecessor `e`; no validity terms.

Reject an empty authorization ID and any present `valid_until` that is not
strictly greater than `valid_from`.

Keep kind 30404 attestation builders/parsers unchanged. Replace old 30406 acceptance builders/parsers only where required by ADP 0.3; old-shape events must not parse as authorization roots.

Expose `build_authorization_root_event_builder(terms, transition_d)` and
`build_authorization_revocation_event_builder(root_terms, transition_d,
predecessor_id)`. Both return unsigned `EventBuilder` values for the coordinate
developer to sign; the revocation builder generates no mutable validity fields.

- [ ] **Step 4: Implement lifecycle resolution**

Implement `parse_authorization_event`, `resolve_authorization(root_id, events)`, and `ResolvedAuthorization::authorizes(signer, coordinate, at)`. Require every transition to belong to the root-ID component, preserve developer/authorization ID/coordinate/key, have a unique `d`, and form one strict linear chain.

Exercise cycle/self-reference resolver branches by modifying parsed-node
predecessor fields in tests, while parser tests continue requiring valid signed
events.

- [ ] **Step 5: Run focused tests and commit**

```bash
cargo test -p adp-core --test authorization_tests
cargo test -p adp-core --test protocol_regression_tests
git add adp-core/src/provisioning.rs adp-core/src/lib.rs adp-core/tests/authorization_tests.rs adp-core/tests/protocol_regression_tests.rs
git commit -m "feat(core): add delegated authorization lifecycles"
```

### Task 5: Implement Generic Entitlement Grants

**Files:**
- Modify: `adp-core/src/entitlement.rs`
- Create: `adp-core/tests/entitlement_tests.rs`

- [ ] **Step 1: Add failing generic grant tests**

Test builder/parser kind agreement, developer root, required tag duplicates, zero-or-one optional anchors/reason, invalid status, root predecessor, update without predecessor, invariant mutation, strict timestamp increase, terminal revocation, fork, cycle, disconnected root collision, cross-grant predecessor, and self-reference.

- [ ] **Step 2: Verify failures**

Run: `cargo test -p adp-core --test entitlement_tests generic_`

Expected: failures because no generic grant implementation exists.

- [ ] **Step 3: Implement builder and parser**

Implement `BuildEntitlementParams`, `build_entitlement_event_builder`, and `parse_entitlement_event`. Use kind 1030 exclusively. Validate signatures in the parser used for authorization; if a shape-only parser is retained for builders, name it explicitly so callers cannot mistake it for verified parsing.

- [ ] **Step 4: Implement connected-component resolution**

Group discovery first by `(recipient, coordinate, d)`, then split predecessor-connected components. Resolve each root component independently. Preserve kind, `d`, `p`, `a`, `source_event`, and `authorization_event`, but not pubkey. Reject forks beneath one root without invalidating unrelated colliding roots.

As with campaign topology tests, mutate predecessor fields only on already parsed
test nodes to cover cycle/self-reference branches without bypassing production
signature verification.

- [ ] **Step 5: Run tests and commit**

```bash
cargo test -p adp-core --test entitlement_tests
cargo test -p adp-core
git add adp-core/src/entitlement.rs adp-core/tests/entitlement_tests.rs
git commit -m "feat(core): implement Entitlement Grant chains"
```

### Task 6: Implement ADP Grant Authority

**Files:**
- Modify: `adp-core/src/entitlement_adp.rs`
- Modify: `adp-core/tests/entitlement_tests.rs`

- [ ] **Step 1: Add failing ADP authority tests**

Test publisher issuance, delegated issuance with active 30406 root, stranger signer, missing delegated anchor, campaign mismatch, campaign inactive at issuance, cancellation before/at issuance, later cancellation, publisher revocation, fulfillment revocation rejection, changed anchor on update, authorization revocation before/at/after issuance, and a different fulfillment key under the preserved anchor.

- [ ] **Step 2: Verify failures**

Run: `cargo test -p adp-core --test entitlement_tests adp_`

Expected: ADP authority assertions fail.

- [ ] **Step 3: Implement root and update verification**

Implement a pure function with explicit dependencies:

```rust
pub fn verify_adp_entitlement(
    grant: &ResolvedEntitlementGrant,
    campaign: &ResolvedCampaign,
    authorization: Option<&ResolvedAuthorization>,
) -> Result<(), AdpError>
```

Require one campaign-root `source_event`; require no anchor for direct developer signing unless a valid self-authorization lifecycle is supplied; require exactly one matching active lifecycle for delegated signing. Evaluate campaign and authorization at the root grant timestamp. Authorize every revocation event only as the coordinate developer.

- [ ] **Step 4: Run tests and commit**

```bash
cargo test -p adp-core --test entitlement_tests
cargo test -p adp-core
git add adp-core/src/entitlement_adp.rs adp-core/tests/entitlement_tests.rs
git commit -m "feat(core): enforce ADP grant authority"
```

### Task 7: Add ADP Authorization Anchors to NIP-102 Receipts

**Files:**
- Modify: `adp-core/src/receipt.rs`
- Modify: `adp-core/src/delegation.rs`
- Create: `adp-core/tests/receipt_authorization_tests.rs`

- [ ] **Step 1: Add failing receipt-profile tests**

Test direct developer receipt without anchor, direct receipt with valid self-authorization anchor, delegated receipt without anchor, multiple anchors, active matching lifecycle, wrong developer/key/coordinate, not-yet-valid, expired, revoked before and exactly at receipt time, valid before later revocation, update anchor mutation, strict timestamp increase, developer status update, same fulfillment-key status update while authorized, and different fulfillment-key rejection.

- [ ] **Step 2: Verify failures**

Run: `cargo test -p adp-core --test receipt_authorization_tests`

Expected: parser lacks `authorization_event` and existing chain resolution is insufficient.

- [ ] **Step 3: Extend receipt params and exact parsing**

Add `authorization_event_id: Option<EventId>` to `BuildReceiptParams` and `ParsedReceipt`. Enforce zero-or-one anchor and preserve existing payment proof tags and statuses. Do not add a non-payment builder path.

- [ ] **Step 4: Replace tip-only logic with complete chain validation**

Implement `resolve_receipt_chain` that preserves order, buyer, coordinate, and anchor; requires strict timestamps and one linear component; does not preserve signer pubkey; and authorizes each event against the coordinate developer or the preserved 30406 lifecycle. Retain `latest_status_in_chain` only as a compatibility wrapper over validated resolution if existing callers still require its signature.

- [ ] **Step 5: Restrict current-listing delegation helper semantics**

Update `verify_signer_authorization` documentation and call sites so it is used only for current issuance/upload checks. Portable receipt ownership must call the 30406-based verifier.

- [ ] **Step 6: Run tests and commit**

```bash
cargo test -p adp-core --test receipt_authorization_tests
cargo test -p adp-core
git add adp-core/src/receipt.rs adp-core/src/delegation.rs adp-core/tests/receipt_authorization_tests.rs
git commit -m "feat(core): anchor ADP receipts to authorization roots"
```

### Task 8: Implement Per-Relay Completeness and Chain Retrieval

**Files:**
- Modify: `adp-server/src/relay.rs`
- Modify: `adp-server/tests/multi_relay_freshness_tests.rs`
- Modify: `adp-server/tests/health_tests.rs`
- Modify: `adp-server/tests/route_integration_tests.rs`

- [ ] **Step 1: Add failing filter and completeness tests**

Assert serialized filters contain exactly the approved kind and indexed tags. Add in-process relay tests where two EOSE responses confirm absence, one EOSE plus one timeout returns unavailable, a failed relay does not count as absence, a current listing needs two completions, and immutable found events retain exact IDs.

- [ ] **Step 2: Add failing bounded-recovery tests**

Test missing predecessor recovery, depth 32 acceptance and 33 rejection, 64-event acceptance and 65 rejection, cycle detection during recovery, unresolved predecessor, authorization `#e` successor traversal, and inability to conclude no revocation with fewer than two successful relay completions.

- [ ] **Step 3: Verify failures**

Run:

```bash
cargo test -p adp-server relay
cargo test -p adp-server --test multi_relay_freshness_tests
```

Expected: failures because the current SDK aggregate query exposes no per-relay completeness.

- [ ] **Step 4: Fan out queries per configured relay**

Store normalized configured relay URLs in `RelayClient`. For each URL, call `Client::fetch_events_from([url], filter.clone(), RELAY_FETCH_TIMEOUT)` and count `Ok` results as completed auto-closing EOSE-backed queries. Merge events by event ID. Do not count errors or timeouts.

Use constants:

```rust
const MIN_SUCCESSFUL_RELAYS: usize = 2;
const MAX_CHAIN_FETCH_DEPTH: usize = 32;
const MAX_CHAIN_EVENTS: usize = 64;
```

- [ ] **Step 5: Implement trait methods and completion helpers**

Implement `fetch_event`, current `fetch_listing`, campaign/grant/receipt collections, authorization `#a` discovery, and authorization successor `#e` traversal with `RelayLookup`. Immutable found events may return immediately after an exact-ID match; confirmed absence and collection completeness require two successful relays. Current replaceable listing selection requires two successful relays before selecting the highest valid timestamp.

- [ ] **Step 6: Update deterministic mocks**

Every mock must model `Found`, `ConfirmedAbsent`, and `Unavailable` explicitly. Do not turn an empty vector into absence outside the mock's declared successful-relay count.

- [ ] **Step 7: Run tests and commit**

```bash
cargo test -p adp-server relay
cargo test -p adp-server --test multi_relay_freshness_tests
git add adp-server/src/relay.rs adp-server/tests/multi_relay_freshness_tests.rs adp-server/tests/health_tests.rs adp-server/tests/route_integration_tests.rs
git commit -m "feat(server): track relay lookup completeness"
```

### Task 9: Add Entitlement and Receipt Persistence

**Files:**
- Create: `migrations/0003_entitlements.sql`
- Modify: `adp-server/src/storage.rs`
- Create: `adp-server/tests/entitlement_storage_tests.rs`

- [ ] **Step 1: Write the migration**

Create `entitlement_claims` exactly as specified, including unique claim/grant/event constraints and the pending/published timestamp check. Alter `purchases` with nullable `adp_profile_version`, `authorization_event_id`, `receipt_issuer_pubkey`, and `receipt_event_json`. Alter `download_tokens` with nullable `source_type` and `source_event_id`; do not add the already-existing `buyer_pubkey` again.

- [ ] **Step 2: Add failing clean/upgrade tests**

Test a clean migration, an upgrade after applying 0001/0002 and inserting purchases/files/tokens, preservation of existing rows, null legacy profile classification, version-3 required metadata, token source metadata, and claim uniqueness.

- [ ] **Step 3: Add failing integrity tests**

Insert valid signed claim and receipt JSON, then independently corrupt each duplicated column. Assert load methods return a typed storage corruption error for event ID, issuer, buyer, coordinate, campaign/source, authorization root, signature, profile version, and state timestamp mismatches.

- [ ] **Step 4: Implement typed records and storage operations**

Add `StoredEntitlementClaim`, `ClaimState`, and `StoredPurchaseReceipt`. Implement `find_entitlement_claim`, `insert_pending_entitlement_claim`, `record_claim_publish_attempt`, `mark_claim_published`, `record_purchase_v3`, and source-aware `issue_download_token`. Detect SQLite unique-constraint races and reload the winning row.

- [ ] **Step 5: Keep transactions short**

No storage method accepts a relay handle or performs network I/O. Pending insertion and published-state update are separate transactions. `record_claim_publish_attempt` runs even after publication failure.

- [ ] **Step 6: Run tests and commit**

```bash
cargo test -p adp-server --test entitlement_storage_tests
cargo test -p adp-server --test keystore_tests
git add migrations/0003_entitlements.sql adp-server/src/storage.rs adp-server/tests/entitlement_storage_tests.rs
git commit -m "feat(server): persist entitlement claim state"
```

### Task 10: Issue Anchored Purchase Receipts and Shared Metadata

**Files:**
- Modify: `adp-server/src/routes/purchase.rs`
- Modify: `adp-server/src/routes/well_known.rs`
- Modify: `adp-server/src/announcement.rs`
- Modify: `adp-server/src/error.rs`
- Modify: `adp-server/tests/announcement_tests.rs`
- Modify: `adp-server/tests/error_mapping_tests.rs`
- Modify: `adp-server/tests/route_integration_tests.rs`

- [ ] **Step 1: Add failing purchase issuance tests**

Test that delegated purchase confirmation requires current listing server/key authorization and an active 30406 lifecycle; writes the root ID into the receipt; and stores profile version 3, issuer, anchor, and exact signed JSON. Test direct signing when `operator_keys.pubkey == coordinate developer`, no lifecycle, revoked lifecycle, and unavailable lifecycle queries.

- [ ] **Step 2: Add failing metadata/error tests**

Assert announcement and well-known metadata both use `ADP_VERSION`; well-known `event_kinds` equals 1030/1031 constants and `event_kind_status == "provisional"`; relay query/publication failures map to 503 through `ApiError::ServiceUnavailable`.

- [ ] **Step 3: Verify failures**

Run:

```bash
cargo test -p adp-server --test announcement_tests
cargo test -p adp-server --test error_mapping_tests
cargo test -p adp-server --test route_integration_tests
```

- [ ] **Step 4: Implement anchored purchase issuance**

Select `operator_keys` directly only when its pubkey equals the coordinate developer. Otherwise, after current listing and held-key selection, discover active 30406 roots for the coordinate, complete successor traversal, and select one whose invariant `p` equals the signing key. Pass its root ID to `BuildReceiptParams`. Publish, then call `record_purchase_v3` and issue a token with `source_type = "purchase"` and `source_event_id = receipt.id`.

- [ ] **Step 5: Centralize metadata and 503 errors**

Replace both hardcoded `0.2.0` strings with `ADP_VERSION`. Add stable well-known fields. Map `RelayLookup::Unavailable` and relay publication errors to `ServiceUnavailable`; keep missing local signing authority as 500.

- [ ] **Step 6: Run tests and commit**

```bash
cargo test -p adp-server --test announcement_tests
cargo test -p adp-server --test error_mapping_tests
cargo test -p adp-server --test route_integration_tests
git add adp-server/src/routes/purchase.rs adp-server/src/routes/well_known.rs adp-server/src/announcement.rs adp-server/src/error.rs adp-server/tests/announcement_tests.rs adp-server/tests/error_mapping_tests.rs adp-server/tests/route_integration_tests.rs
git commit -m "feat(server): issue ADP 0.3 anchored receipts"
```

### Task 11: Implement `POST /entitlement/claim`

**Files:**
- Create: `adp-server/src/routes/entitlement.rs`
- Modify: `adp-server/src/routes/mod.rs`
- Create: `adp-server/tests/claim_route_tests.rs`

- [ ] **Step 1: Add failing route tests**

Cover direct developer success, delegated success, not started, ended, cancelled, wrong developer, wrong coordinate, campaign fork, server unauthorized, hosted file absent, no held signing key, missing/revoked/unavailable 30406 lifecycle, idempotent repeat, concurrent duplicate, token issuance, and exact NIP-98 URL/method.

- [ ] **Step 2: Add failing publication-window tests**

Use a controllable relay for publication failure. For the database failure window,
install a SQLite trigger that raises `FAIL` before an entitlement row changes to
`published`, perform a successful relay publication, assert the route errors and
the row remains pending, drop the trigger, and retry. Assert retry republishes
byte-for-byte identical JSON and event ID, marks published, and returns the same
event with `already_claimed: true` plus a fresh token.

- [ ] **Step 3: Verify failures**

Run: `cargo test -p adp-server --test claim_route_tests`

- [ ] **Step 4: Implement verification order**

Implement the approved 17-step order: NIP-98, coordinate, complete current listing, signature/coordinate, current server, local hosted file, immutable campaign root, complete campaign chain, claimability, direct-or-held key selection, and active 30406 lifecycle for delegation. Use one captured `now` for claim evaluation and grant `created_at`.

- [ ] **Step 5: Implement pending publication state machine**

Build and sign one candidate only after all checks. Insert pending. On uniqueness loss, discard the candidate and load the winner. For pending rows, publish stored JSON exactly, record attempts, and mark published separately. Never regenerate or retimestamp pending events. Issue token metadata as `entitlement` plus grant event ID.

- [ ] **Step 6: Register route and run tests**

```bash
cargo test -p adp-server --test claim_route_tests
cargo test -p adp-server --test route_integration_tests
git add adp-server/src/routes/entitlement.rs adp-server/src/routes/mod.rs adp-server/tests/claim_route_tests.rs
git commit -m "feat(server): add entitlement claim endpoint"
```

### Task 12: Refactor Portable Ownership and Downloads

**Files:**
- Modify: `adp-server/src/ownership.rs`
- Modify: `adp-server/src/routes/game.rs`
- Create: `adp-server/tests/download_authorization_tests.rs`

- [ ] **Step 1: Add failing ownership-isolation tests**

Test valid grant, valid anchored receipt, malformed grant plus valid receipt, malformed receipt plus valid grant, colliding grant roots, fork isolation, grant relay unavailable plus valid receipt, receipt relay unavailable plus valid grant, both unavailable plus public/timed policy, malformed credentials without 503, and unresolved gated lookup with 503.

- [ ] **Step 2: Add failing route precedence tests**

Cover token bearer access, wrong-coordinate/expired token, explicit public, timed start/end, malformed policy, zero price denial, valid credential despite invalid policy, campaign cancellation after grant, listing price/policy changes, authorization rotation after valid grant/receipt, current server removal 451, and file-integrity failure for every basis.

- [ ] **Step 3: Verify failures**

Run: `cargo test -p adp-server --test download_authorization_tests`

- [ ] **Step 4: Implement independent credential checks**

`verify_ownership_via_entitlements` must discover by `#p/#a`, split by `d` and root component, complete required predecessors, retrieve campaign and authorization chains, and return `Valid`, `Absent`, `Invalid`, or `Unavailable` without early global failure. Receipt verification follows the same candidate isolation and 30406 anchor retrieval.

- [ ] **Step 5: Implement final route order**

Remove `listing_is_free` and its test. Validate current listing/distribution/file metadata first; token second; NIP-98 then grants and receipts; explicit policy next; 503 only for unresolved required relay data; otherwise 403. Log `AccessBasis` without buyer-sensitive content. Preserve existing ETag and hash verification code.

- [ ] **Step 6: Run tests and commit**

```bash
cargo test -p adp-server --test download_authorization_tests
cargo test -p adp-server game
git add adp-server/src/ownership.rs adp-server/src/routes/game.rs adp-server/tests/download_authorization_tests.rs
git commit -m "feat(server): authorize durable and explicit access"
```

### Task 13: Integrate Security and Compatibility Tests

**Files:**
- Create: `adp-server/tests/security_integration_tests.rs`
- Create: `adp-server/tests/migration_integration_tests.rs`
- Modify: `adp-server/tests/route_integration_tests.rs`

- [ ] **Step 1: Run all tests to expose cross-workstream gaps**

Run: `cargo test --workspace`

Expected: identify integration mismatches or missing required scenarios. Fix a
shared-contract mismatch in the task that owns that file before continuing.

- [ ] **Step 2: Add protocol-constant and filter guards**

Use `rg -n '\b1021\b|Kind::Custom\(1030\)|Kind::Custom\(1031\)|"0\.3\.0"' --glob '*.rs'`. The only numeric kind/version definitions must be centralized constants plus assertions that prohibit 1021. Replace all production literals.

- [ ] **Step 3: Add the stale-authority regression**

Create a full test where listing A and active authorization allow key F, authorization revocation and listing B remove F, a pre-revocation credential remains valid, and a credential at/after revocation fails despite referencing the old root. Verify portable validation never consults listing A.

- [ ] **Step 4: Add migration and publication recovery integration**

Run a database upgraded from 0002 through claim and purchase flows. Verify legacy rows remain null-profile, all new receipts are profile 3, pending events survive reconnect, and the same signed event is republished after both failure windows.

- [ ] **Step 5: Run workspace tests and commit**

```bash
cargo test --workspace
git add adp-server/tests/security_integration_tests.rs adp-server/tests/migration_integration_tests.rs adp-server/tests/route_integration_tests.rs
git commit -m "test: cover ADP 0.3 security boundaries"
```

### Task 14: Update Protocol and Repository Documentation

**Files:**
- Rename if present: `ADP-01-amendment-free-acquisitions.md` -> `ADP-01-amendment-free-acquisitions-v3.md`
- Modify: `ADP-01-amendment-free-acquisitions-v3.md`
- Modify: `ADP-01.md`
- Modify: `NIP-103.md`
- Modify: `README.md`
- Modify: `CODEBASE.md`
- Modify: `.env.example` only if runtime configuration changed

- [ ] **Step 1: Rename only the active unversioned amendment**

Check `test -e ADP-01-amendment-free-acquisitions.md`. If it exists and the v3 filename does not, rename that file. Do not rename historical ADP drafts.

- [ ] **Step 2: Update protocol semantics**

Document 1030/1031 as provisional, stable metadata fields, explicit acquisition values, campaign and grant cardinality, kind 30406 unique transition `d` plus stable `authorization_id`, `#e` traversal, two-relay completeness, claim request/response, error mapping, bearer tokens, and durable coordinate ownership. Keep NIP-102 payment-backed; describe `authorization_event` as an ADP profile tag.

- [ ] **Step 3: Update repository documentation**

Update endpoint tables, architecture diagrams, schema, route flows, test inventory, and gotchas. Remove statements that zero price grants access, only receipts prove ownership, or only purchase confirmation issues tokens.

- [ ] **Step 4: Run the protocol consistency review**

Compare documents and code for provisional kinds, tag names/cardinality, paths, JSON fields, policy/status values, half-open boundaries, authority rules, HTTP statuses, and ADP version. Search:

```bash
rg -n '1021|zero.?price|free listing|0\.2\.0|historical listing.*authoriz|only receipts|purchase/confirm.*only' ADP-01*.md NIP-*.md README.md CODEBASE.md adp-core adp-server
```

Every remaining match must be an explicit prohibition, historical migration note, or unrelated NIP assignment.

- [ ] **Step 5: Commit**

```bash
git add ADP-01.md ADP-01-amendment-free-acquisitions-v3.md NIP-103.md README.md CODEBASE.md .env.example
git commit -m "docs: document ADP 0.3 entitlement flows"
```

### Task 15: Sol Review and Verification Gate

**Files:**
- Review all files changed by Tasks 1-14
- Update: `VERIFICATION_REPORT.md` only if the repository continues using it for current results

- [ ] **Step 1: Review every workstream result**

Inspect changed files and agent reports. Reconcile shared types, errors, mock behavior, and user changes. Confirm no agent introduced unrelated refactoring or compatibility fallback for unanchored delegated credentials.

- [ ] **Step 2: Run security searches**

```bash
rg -n '\b1021\b|listing_is_free|Kind::Custom\(1030\)|Kind::Custom\(1031\)|"0\.3\.0"' --glob '*.rs'
rg -n 'timestamp.*(latest|greatest)|max_by.*created_at' adp-core/src/campaign.rs adp-core/src/entitlement.rs adp-core/src/provisioning.rs
rg -n 'authorization_event|authorization_id|source_event' adp-core adp-server
```

Expected: no zero-price helper; no production kind/version literals outside centralized constants; no timestamp fork selection; every delegated credential path resolves a 30406 root ID.

- [ ] **Step 3: Run each required command separately**

```bash
cargo fmt --all --check
cargo check -p adp-core --all-targets
cargo check -p adp-server --all-targets
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo clippy --workspace -- -D warnings
```

Expected: every command exits 0. Do not combine commands and do not claim completion if one is skipped or fails.

- [ ] **Step 4: Run the CI-equivalent comparison**

Confirm `.github/workflows/ci.yml` still corresponds to format, workspace build, clippy with warnings denied, and workspace tests. Report the exact output status of all seven commands.

- [ ] **Step 5: Final protocol consistency review**

Verify provisional values, tag names/cardinality, endpoint paths, request/response fields, acquisition/status values, half-open boundaries, developer/fulfillment authority, HTTP mapping, and advertised version against all authoritative documents.

- [ ] **Step 6: Stop on any final-review discrepancy**

If review finds a discrepancy, return to the task that owns that exact file,
write or update its focused regression test, rerun that task's command, and commit
the named files through that task. Do not create an unscoped catch-all commit and
do not proceed to the final report until the seven-command gate is clean.

## Final Report Contents

Report:

- implementation architecture and module boundaries;
- complete changed-file list;
- migration and legacy-row classification;
- claim state machine and failure recovery;
- new route and response behavior;
- centralized kind/version configuration;
- 30406 authorization lifecycle and rotation requirements;
- relay completeness rules;
- final ownership decision flow;
- tests added;
- each verification command and result separately;
- remaining limitation that 1030 and 1031 are provisional, unallocated kinds.
