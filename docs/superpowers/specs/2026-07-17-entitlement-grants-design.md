# Entitlement Grants and Free Acquisitions Design

## Status

Approved design for implementing the Entitlement Grant Draft and the ADP-01
free-acquisition amendment v3 in the `adp-server` reference implementation.

Authoritative documents:

- `NIP-103.md` (`Entitlement Grant Draft`)
- `ADP-01-amendment-free-acquisitions-v3.md` (currently present under the
  unversioned filename and to be renamed before implementation documentation is
  finalized)
- `ADP-01.md`
- `NIP-102.md`
- `CODEBASE.md`
- `.github/workflows/ci.yml`

NIP-102 remains payment-backed. ADP adds an authorization anchor to its receipt
profile but does not create a free receipt path.

## Goals

Implement:

- explicit gated, public, and timed-access listing policy;
- publisher-signed claim campaigns;
- generic Entitlement Grant parsing, construction, and chain resolution;
- ADP campaign, issuance, delegation, and revocation verification;
- `POST /entitlement/claim`;
- persistent local claim idempotency with recoverable relay publication;
- portable grant and receipt ownership;
- public and timed downloads;
- durable coordinate-level ownership across listing versions;
- removal of all zero-price access inference.

The design preserves provisioning, uploads, payment proof, replay protection,
file integrity, conditional downloads, and current distribution authorization.

## Non-Negotiable Authority Rules

Only the developer key encoded in a game coordinate may:

- create a campaign;
- replace campaign terms before the effective start;
- cancel a campaign;
- revoke an Entitlement Grant.

The developer or a fulfillment key authorized by a specific signed historical
listing event may issue a root grant. Fulfillment keys cannot define policy,
change or cancel campaigns, or revoke grants.

Listing campaign pointers are discovery hints only. They never prove campaign
validity, claimability, ownership, or distribution authority.

## Shared Public Boundaries

Shared public types and module boundaries must be defined before parallel work so
implementation workstreams do not create incompatible abstractions.

### `adp-core/src/kinds.rs`

```rust
pub const EXPERIMENTAL_ENTITLEMENT_GRANT_KIND: u16 = 1030;
pub const EXPERIMENTAL_ADP_CAMPAIGN_KIND: u16 = 1031;
pub const ADP_VERSION: &str = "0.3.0";
```

The Nostr registry was checked at commit
`d51285770b3093e575bdf8d0bd81786fe1efbd6d`, dated 2026-06-23. Kinds 1030 and
1031 were unregistered; kind 1021 was allocated to NIP-15 Bid.

The values are provisional development values, not official allocations.
Builders, parsers, relay filters, metadata, announcements, and tests must import
these constants. Tests must prove the two values are distinct and neither is
1021.

After formal allocation, replacement consists of changing these constants,
changing `event_kind_status` from `provisional` to `allocated`, running all
protocol/filter/metadata tests, and reviewing protocol examples. Field names do
not change.

### `adp-core/src/listing.rs`

```rust
pub enum AcquisitionPolicy {
    Gated,
    Public,
    TimedAccess { starts: u64, ends: u64 },
    Invalid,
}

pub struct CampaignPointer {
    pub root_event_id: EventId,
    pub relay_hint: Option<RelayUrl>,
}
```

`AdpListing` exposes the acquisition policy and repeatable campaign pointers.

### `adp-core/src/campaign.rs`

```rust
pub struct CampaignTerms {
    pub mode: CampaignMode,
    pub starts: u64,
    pub ends: u64,
}

pub enum CampaignTransition {
    Root(CampaignTerms),
    ReplaceTerms(CampaignTerms),
    Cancel,
}

pub struct CampaignEvent { /* parsed signed event and invariant identity */ }
pub struct ResolvedCampaign { /* one linear validated chain */ }
pub enum CampaignStatus { Active, Cancelled }
pub enum ChainResolutionError { /* typed topology and transition failures */ }
```

Campaign namespace keys include developer, game coordinate, and campaign `d`.

### `adp-core/src/entitlement.rs`

This module is protocol-generic. It owns grant tag parsing, exact cardinality,
generic lifecycle validation, connected-chain resolution, and builder helpers.
It does not know ADP campaigns or fulfillment delegation.

```rust
pub enum GrantStatus { Granted, Revoked }
pub struct ParsedEntitlementGrant { /* generic tags and signed event */ }
pub struct ResolvedEntitlementGrant { /* one connected valid chain */ }
```

### `adp-core/src/entitlement_adp.rs`

This module owns ADP-specific grant verification:

- campaign-root and campaign-state verification;
- historical listing authorization anchors;
- developer or delegated root issuance;
- publisher-only revocation;
- per-event signer authority.

### Server boundaries

- `relay.rs`: relay filters, retrieval, bounded predecessor completion.
- `storage.rs`: persistent state and database/event integrity.
- `routes/entitlement.rs`: claim orchestration.
- `routes/game.rs`: access decision orchestration.
- Protocol parsing and chain resolution do not move into route handlers.

## Listing Policy

No `acquisition` tag produces `Gated`. Exactly
`["acquisition", "public"]` produces `Public`. Exactly
`["acquisition", "timed-access", starts, ends]` with numeric values and
`starts < ends` produces `TimedAccess`.

At least one acquisition tag combined with duplicates, unknown modes, malformed
cardinality, invalid timestamps, or an invalid interval produces `Invalid`.
`Gated` and `Invalid` both deny policy access, but remain distinct for diagnostics
and tests. Invalid policy does not invalidate durable credentials, purchases,
campaign claims, distribution metadata, or other unrelated listing behavior.

Timed access is half-open: `starts <= now < ends`. Price is never consulted for
download authorization. A zero or missing price does not grant access.

Current listing validation for claims and downloads requires a valid signature,
kind 30402, exactly one `d`, author equal to the coordinate developer, and `d`
equal to the requested coordinate identifier.

Campaign pointers accept a valid root event ID with an optional hint. A malformed
event ID discards the pointer. A valid event ID with a malformed hint preserves
the pointer with no hint. The pure parser accepts typed `wss://` hints. Because
the repository has no general insecure-relay policy, `ws://` hints are discarded.
Configured relays and coordinate discovery remain the authoritative fallback.

## Campaign Protocol

Every campaign event requires:

- kind 1031 through the shared constant;
- a valid signature;
- exactly one `d`, `a`, and `status`;
- author equal to the developer encoded in `a`;
- invariant developer, coordinate, and `d` through the chain.

A root has no predecessor, status `active`, complete `mode: claim`, `starts`, and
`ends`, `starts < ends`, and `starts >= root.created_at`.

A term replacement has exactly one predecessor, status `active`, complete
replacement `mode: claim`, `starts`, and `ends` terms,
`update.created_at < previous_effective_starts`, and
`new_starts > update.created_at`. At or after the effective start, only
cancellation is valid.

A cancellation has exactly one predecessor and may carry only `d`, `a`, `status:
cancelled`, and `e`. It inherits prior terms and is terminal.

Every successor requires `successor.created_at > predecessor.created_at`.

Resolution builds predecessor and successor maps, requires one connected linear
chain, and rejects self-reference, cycles, forks, disconnected events, cross-
campaign predecessors, invariant mutation, and invalid transitions. Forks are
never resolved by timestamps.

Internally, `ChainResolutionError` distinguishes missing predecessors from
malformed topology. Claims and ownership fail closed for every unresolved error,
but `MissingPredecessor` may trigger bounded immutable-ID recovery first.

Effective state at time `T` begins with root terms and applies transitions in
chain order when `created_at <= T`. Replacements replace all terms; cancellation
sets terminal cancelled status. A campaign is claimable exactly when its state is
active and `starts <= T < ends`. Cancellation at `T` blocks issuance at `T`.

## Generic Entitlement Grants

The generic parser enforces:

- exactly one `d`, `p`, `a`, and `status`;
- zero or one `source_event`;
- zero or one `authorization_event`;
- zero or one informational `reason`;
- status restricted to `granted` or `revoked`;
- valid IDs, coordinate, kind, and signature.

The root is `granted` and has no predecessor. An update has exactly one
predecessor and may only transition to terminal `revoked`. Every successor has a
strictly greater timestamp and points to the current chain tip.

Updates preserve kind, `d`, `p`, `a`, and the presence and exact value of
`source_event` and `authorization_event`. Event pubkey is not invariant because a
delegate may sign the root while a developer signs revocation. The consuming
protocol authorizes each signer independently.

Resolution rejects self-reference, cycles, forks, disconnected events, missing
or cross-grant predecessors, invariant mutation, and invalid transitions. It does
not choose a fork by timestamp.

Grant discovery begins with `(recipient, coordinate, d)` and then separates
disconnected root components. An unrelated colliding root cannot poison a valid
component. A fork descending from one root invalidates that component.

## ADP Entitlement Verification

An ADP campaign grant requires exactly one `source_event` referencing the
campaign root. The campaign developer and coordinate must match the grant.
Campaign state is evaluated at the root grant's signed `created_at`.

For direct developer issuance, `authorization_event` may be absent or singular.
If present, it must identify a valid matching signed historical kind 30402 event.

For fulfillment issuance, exactly one `authorization_event` is required. It
references a specific signed historical kind 30402 event. The coordinate is
replaceable, but this event's ID and signed contents are immutable. Its signature,
author, `d`, and coordinate must match, and its delegation must authorize the
root signer at the grant timestamp using the existing backdating clamp.

Every update preserves `d`, `p`, `a`, `source_event`, and
`authorization_event` presence/value. Only the coordinate developer may sign the
revocation. A fulfillment key cannot sign any grant update.

Later campaign cancellation, listing replacement, price or policy changes,
server-pointer changes, and key rotation do not invalidate a valid earlier grant.

## ADP NIP-102 Receipt Profile

NIP-102 remains payment-backed. ADP adds an optional generic tag with normative
ADP cardinality:

```json
["authorization_event", "<historical-listing-event-id>"]
```

Developer-signed receipt events may omit the anchor. A present anchor must be
singular and identify a valid matching signed historical listing event.

Fulfillment-signed receipt events require exactly one anchor. The historical
listing signature, author, `d`, and coordinate must match, and its delegation
must authorize the signer at that receipt event's timestamp using the existing
backdating clamp. There is no fallback for unanchored delegated receipts.

Receipt chains preserve order, buyer `p`, resource `a`, and anchor presence/value.
Successor timestamps strictly increase. Signer pubkey is not invariant. Every
event is authorized independently against the preserved anchor. A second
fulfillment key may sign a status update only when the same historical listing
authorized it at that timestamp; the developer may sign directly.

Payment proof, bolt11/preimage validation, zap validation, and payment replay
protection remain unchanged. Current listing state does not invalidate a valid
historically anchored receipt.

## Historical and Current Listing Roles

The `authorization_event` historical listing proves delegated signer authority
at credential issuance. It proves matching developer, coordinate, signed terms,
and delegation interval.

The current listing controls current server authorization, file hash, version,
hosted build metadata, explicit acquisition policy, and authority for issuing a
new receipt or grant now.

A historical event never authorizes current distribution. A current listing is
never used to re-evaluate historical delegated grant or receipt issuance.

## Relay Interface and Completion

The server relay trait gains focused methods:

```rust
fetch_event(event_id)
fetch_campaigns(game_coordinate)
fetch_campaign_chain(developer, campaign_d)
fetch_entitlements(buyer, game_coordinate)
fetch_grant_events_by_d(grant_d)
fetch_receipts(buyer, game_coordinate)
```

Portable filters use only:

- campaign discovery: kind 1031 and `#a`;
- campaign chain: kind 1031, developer author, and `#d`;
- grant discovery: kind 1030, `#p`, and `#a`;
- grant `d` lookup: kind 1030 and `#d`;
- receipt discovery: kind 1020, `#p`, and `#a`.

Immutable IDs fetch campaign roots, historical listings, and missing
predecessors. The implementation never relies on relay indexing of multi-character
tags.

Missing predecessor recovery has centralized finite limits for recursion depth
and total events, initially 32 and 64 respectively. Cycle detection remains
active while completing a chain. Exceeding either limit or failing to recover an
event fails authorization closed. Other malformed states do not trigger
timestamp-based recovery or selection.

## Persistent Claim State

Migration `0003_entitlements.sql` creates:

```sql
CREATE TABLE entitlement_claims (
    buyer_pubkey TEXT NOT NULL,
    game_coordinate TEXT NOT NULL,
    campaign_id TEXT NOT NULL,
    campaign_root_event_id TEXT NOT NULL,
    grant_id TEXT NOT NULL,
    grant_event_id TEXT NOT NULL,
    issuer_pubkey TEXT NOT NULL,
    authorization_event_id TEXT,
    grant_event_json TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('pending', 'published')),
    created_at INTEGER NOT NULL,
    last_attempt_at INTEGER,
    published_at INTEGER,
    UNIQUE (buyer_pubkey, game_coordinate, campaign_id),
    UNIQUE (grant_id),
    UNIQUE (grant_event_id),
    CHECK (
        (state = 'pending' AND published_at IS NULL)
        OR (state = 'published' AND published_at IS NOT NULL)
    )
);
```

The migration also adds nullable receipt issuer, authorization event, and complete
signed root receipt JSON columns to purchases so upgrades preserve existing rows.
Every newly issued local receipt must populate issuer and event JSON; delegated
receipts must also populate the authorization event. Missing required values when
loading a new-format record are corruption, not a compatibility fallback. The
migration adds nullable token source type and source event ID to download tokens.
`buyer_pubkey` already exists in migration 0001 and is preserved.

New purchase and entitlement tokens store buyer, coordinate, expiry, source type,
and source event ID. A download token remains an opaque bearer credential scoped
to one coordinate. Stored buyer identity is audit and issuance metadata; the
token-only path does not prove control of that key.

Loaded local receipt and claim rows are checked against their signed JSON,
including event ID, signer, buyer, coordinate, anchor, source/campaign root,
signature, and state consistency. Any disagreement is persistence corruption and
returns 500. Requests do not repair it silently.

## Claim Route

`POST /entitlement/claim` accepts:

```json
{
  "game_coordinate": "30402:<developer-pubkey>:<d-tag>",
  "campaign_event_id": "<campaign-root-event-id>"
}
```

The route verifies NIP-98 for the exact POST URL, parses the coordinate, fetches
and validates the current listing, checks current server authorization and local
hosting, fetches the campaign root by ID, completes and validates the publisher
chain, and evaluates claimability at server time.

The operator key signs directly only when its pubkey equals the coordinate
developer. Otherwise the server selects a held fulfillment key currently
authorized by the current listing. The grant `created_at` equals the claim
evaluation time within a small server-local tolerance.

After all checks pass, the route signs one candidate and attempts a short pending
insert. Concurrent losing candidates are discarded; the persisted signed event
is authoritative. No relay publication occurs in a database transaction.

The exact stored pending event is published and then marked published in a second
short transaction. Publication failure records `last_attempt_at`, leaves pending,
and returns 503. Publication success followed by state-update failure returns an
error and leaves pending. A retry republishes the same serialization and event ID;
relay deduplication makes this safe. Pending events are never regenerated,
retimestamped, or re-evaluated against later campaign cancellation. Their issuance
time is their signed `created_at`, and they were persisted only after successful
initial campaign and issuer validation.

Repeated published claims return 200 with the same grant and a fresh token:

```json
{
  "grant": {},
  "download_token": "...",
  "token_expires_at": 1234567890,
  "already_claimed": true
}
```

First success returns the same shape with `already_claimed: false`.

## Download Authorization

`GET /game/:coordinate` executes in this order:

1. Parse the coordinate.
2. Fetch and fully validate the current listing.
3. Confirm current server authorization.
4. Resolve current file hash, version, and local path.
5. If a token is present, validate existence, expiry, and coordinate and allow as
   `DownloadToken`.
6. Otherwise require NIP-98 and extract the buyer.
7. Evaluate Entitlement Grants.
8. Evaluate NIP-102 receipts.
9. Allow if either durable credential validates.
10. Otherwise allow explicit public or active timed policy.
11. If gated and a required credential query was unavailable or incomplete,
    return 503.
12. Otherwise return 403.
13. Recompute and verify the current file hash.
14. Preserve ETag and conditional responses.
15. Stream the file.

Public and timed access remain NIP-98 authenticated. Distribution authorization
and file integrity are mandatory for every access basis.

```rust
pub enum AccessBasis {
    DownloadToken,
    EntitlementGrant,
    PurchaseReceipt,
    PublicPolicy,
    TimedAccessPolicy,
}

pub enum CredentialCheck {
    Valid(AccessBasis),
    Absent,
    Invalid { candidate_count: usize },
    Unavailable { source: CredentialSource },
}
```

Grant and receipt queries and candidate chains are independent. Invalid
credentials are not collapsed into absence or unavailability. One invalid chain
cannot block another valid chain or the other credential type. A query failure
does not prevent checking the other type. Public or timed policy may allow access
despite unavailable credential relays. Malformed credentials alone do not cause
503; unresolved required relay data does.

## HTTP Errors

- 400: malformed coordinate, event, campaign, chain, transition, or anchor.
- 401: NIP-98 failure.
- 403: campaign outside its claimable interval/cancelled, or completed ownership
  checks found no credential or explicit policy.
- 404: a sufficiently completed lookup confirms the listing, campaign root,
  historical anchor, hosted file, or immutable resource is absent.
- 451: current listing does not authorize this server.
- 503: required relay query, chain completion, or publication service unavailable.
- 500: missing held signing authority, signing failure, persistence corruption,
  serialization disagreement, or another internal invariant failure.

The server adds a typed `ServiceUnavailable` error. Relay unavailability is never
reported as confirmed absence.

## Protocol Metadata

The shared `ADP_VERSION` is used by both `/.well-known/adp` and kind 30403 server
announcements. The well-known response adds stable fields:

```json
{
  "adp_version": "0.3.0",
  "pubkey": "...",
  "name": "...",
  "url": "...",
  "event_kinds": {
    "entitlement_grant": 1030,
    "adp_campaign": 1031
  },
  "event_kind_status": "provisional"
}
```

## Workstreams

After shared types and module exports are established, implementation separates
into independently reviewed workstreams:

1. `adp-core` campaign protocol.
2. `adp-core` generic grant and ADP grant verification.
3. Listing acquisition and campaign pointers.
4. ADP receipt historical anchors.
5. Relay trait, filters, and bounded completion.
6. Migration and storage integrity.
7. Claim route.
8. Download ownership refactor.
9. Integration and security tests.
10. Documentation and compatibility review.

Each delegated workstream inspects current code first, avoids unrelated
refactoring, adds focused tests, runs relevant checks, reports changed files and
assumptions, and identifies overlap. The orchestrator reconciles all results and
runs full verification.

## Required Tests

Campaign tests cover valid roots, signer and coordinate mismatch, malformed
times, `starts < created_at`, half-open boundaries, valid pre-start replacement,
replacement at start, `new_starts <= update.created_at`, post-start edits,
cancellation boundaries, before/at-cancellation issuance, forks, cycles,
timestamp regression, changed `d`, and changed `a`.

Grant tests cover developer and delegated roots, unauthorized roots, predecessor
cardinality, anchor cardinality and mutation, developer revocation, rejected
fulfillment revocation, forks, cycles, timestamp regression, campaign mismatch,
inactive issuance, later cancellation, and historical key rotation.

Receipt tests cover developer roots with and without valid anchors, mismatched
anchors, delegated roots requiring exactly one anchor, author and coordinate
mismatch, delegation intervals, historical rotation, anchor mutation, a second
fulfillment key authorized by the preserved listing, and a second key not
authorized by that anchor.

Listing tests cover public, timed boundaries, malformed and duplicate policy,
zero price without acquisition, pointers with and without hints, malformed event
IDs, valid IDs with malformed hints, and discarded `ws://` hints.

Relay tests cover every serialized filter, centralized kinds, immutable-ID
predecessor recovery, depth and event limits, cycles during recovery, unresolved
predecessors, colliding independent grant roots, and fork isolation.

Storage and route tests cover clean and upgrade migrations, preserved rows,
claim uniqueness, database/event corruption, token buyer/source metadata, token
expiry and coordinate scope, bearer semantics, direct and delegated claims,
campaign errors, server authorization, absent signing authority, repeat and
concurrent claims, token issuance, both publication failure windows, and identical
pending-event retry.

Download tests cover every `AccessBasis`, campaign cancellation, listing price and
policy changes, fulfillment-key rotation for grants and receipts, explicit policy,
zero-price denial, malformed-policy behavior with valid durable credentials,
credential isolation, relay-unavailability combinations, 403 versus 503,
distribution removal, and file-integrity failure.

Metadata tests prove announcements and well-known responses use `ADP_VERSION` and
advertise exactly the centralized kinds.

## Documentation

If the active v3 amendment is still under
`ADP-01-amendment-free-acquisitions.md`, rename it to
`ADP-01-amendment-free-acquisitions-v3.md`. Update `ADP-01.md`, `README.md`,
`CODEBASE.md`, endpoint tables, diagrams, schema, environment/configuration,
known gotchas, and test inventory. Update `NIP-103.md` only where generic grant
clarification is required. Keep NIP-102 payment-backed and document the anchor as
an ADP profile extension.

Remove stale statements that zero price bypasses ownership, only receipts prove
ownership, or only purchase confirmation issues tokens.

The final protocol-document consistency review compares all documents and code
for provisional kind values, tag names and cardinality, endpoint paths, request
and response fields, policy and status values, half-open time boundaries,
publisher and fulfillment authority, HTTP mappings, and advertised ADP version.

## Verification Gate

Run and report separately:

```bash
cargo fmt --all --check
cargo check -p adp-core --all-targets
cargo check -p adp-server --all-targets
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo clippy --workspace -- -D warnings
```

Completion is not claimed if any command fails or is skipped.

## Final Security Review

Confirm:

- no use of kind 1021;
- no duplicated protocol-kind or ADP-version literals;
- no zero-price access bypass;
- no fulfillment-signed campaign event or grant revocation;
- no listing-pointer authority;
- no timestamp-based fork resolution;
- no current-listing revalidation of historical delegated issuance;
- no global uniqueness claim;
- no relay publication inside a database transaction;
- no malformed candidate causing global credential failure;
- no current policy invalidating durable ownership;
- no unrelated refactoring.
