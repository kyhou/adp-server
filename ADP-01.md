# ADP-01: Arcadestr Distribution Protocol

`draft` `optional`

A minimal, open HTTP protocol for serving gated digital game files, using Nostr identities and event kinds as the authentication and ownership layer. Any operator can run a compliant ADP server; game developers choose which servers carry their builds. Ownership is proven via a [NIP‑102](NIP-102.md) `kind:1020` receipt, signed either by the developer's own key or by a key the developer has delegated for fulfillment. This credential is portable across servers and does not depend on any single server remaining online.

ADP is modeled after [Blossom](https://github.com/hzrd149/blossom): servers are interchangeable, the protocol is the contract, and no central operator is privileged.

## Terminology

| Term | Definition |
|------|------------|
| **ADP server** | An HTTP server implementing this spec |
| **Developer** | The Nostr keypair that signed the game's [NIP‑99](99.md) listing event |
| **Fulfillment key** | A Nostr keypair the developer delegates, via a listing tag, to sign purchase receipts on their behalf |
| **Operator** | The party running an ADP server. May serve a single developer (including a developer running it for themselves) or many — an operator serving multiple developers issues each a distinct fulfillment key via [Provisioning](#provisioning-endpoints), never a key shared across unrelated developers. Identified by the same pubkey that signs its `kind:30403` announcement. |
| **Scope** | An optional label a developer attaches to a provisioning request, letting the same operator hold multiple independent fulfillment keys for the same developer (e.g. one per game, for isolation). Bookkeeping only — has no bearing on receipt verification. |
| **Game coordinate** | `30402:<developer-pubkey>:<d-tag>` — the stable identifier for a listing |
| **Buyer** | The Nostr keypair that completed payment |
| **LSP** | Lightning Service Provider — the operator of the LNURL-pay endpoint that signs `kind:9735` zap receipts on a developer's behalf, resolved from a listing's `lud16` tag |

## Listing Extensions (`kind:30402`)

A developer distributing a game via ADP adds the following tags to their [NIP‑99](99.md) classified listing:

| Tag | Value | Description |
|-----|-------|--------------|
| `server` | `<https-url>` | Declares a server authorized to distribute this listing's file. Repeatable. |
| `file_hash` | `<sha256-hex>` | SHA‑256 of the game archive. Servers MUST verify stored files against this hash before serving. |
| `version` | `<semver-string>` | Build version. |
| `platform` | `<os>-<arch>` | Optional; a listing per platform is the simplest model. |
| `fulfillment_pubkey` | `<pubkey-hex>`, `<valid_from>`, `<revoked_at>` | Delegates receipt‑signing authority for this listing to `<pubkey-hex>`, valid from the given unix timestamp until `<revoked_at>` (empty if still active). Repeatable — see [Fulfillment Key Delegation](#fulfillment-key-delegation). |
| `lud16` | `<name@domain>` | The Lightning address whose LNURL-pay endpoint is authoritative for this listing's purchases. Required if the listing accepts the zap-receipt payment proof path (not required if only bolt11/preimage proof is ever used). MUST be read from this tag, never from the developer's `kind:0` profile — see [LSP Verification](#lsp-verification-lud16). |

### Example Event

```jsonc
{
  "kind": 30402,
  "pubkey": "<developer-pubkey>",
  "tags": [
    ["d", "my-game-v1"],
    ["server", "https://dist.arcadestr.io"],
    ["file_hash", "a3f5...":],
    ["version", "1.0.0"],
    ["platform", "linux-x86_64"],
    ["fulfillment_pubkey", "a1b2...", "1700000000", "1735000000"],
    ["fulfillment_pubkey", "c3d4...", "1735000000", ""],
    ["lud16", "studio@getalby.com"]
  ],
  "content": "...",
  "sig": "<developer-signature>"
}
```

`server` tags declare **distribution authorization only** — which servers may serve the file. They are independent of ownership proof; see [Distribution Authorization](#distribution-authorization) below.

### Mandatory fields for client-published listings

Nothing in this section is a protocol-level restriction — any Nostr client can publish a `kind:30402` event with any subset of these tags, and relays MUST accept it regardless. This is guidance for a **publishing client's own UI**, not a wire-format requirement: a client that wants a listing to be immediately usable through ADP SHOULD enforce the following before allowing publish, and MUST clearly communicate to the buyer-facing side of the client when a listing lacks them, rather than silently degrading.

Two tiers, since they fail differently:

- **Payment-address tier — required for any priced listing, independent of ADP entirely:** `lud16`. Every priced listing needs a payment address regardless of whether ADP is involved; ADP additionally requires it be resolvable and stable per [LSP Verification](#lsp-verification-lud16), so a client SHOULD treat it as mandatory at publish time rather than optional.
- **Fulfillment tier — required only for listings that want automated install/download:** `file_hash`, `server` (≥1), `fulfillment_pubkey` (with `valid_from`). A listing missing these can still be a valid, purchasable listing — it simply cannot support one-click install, and a client SHOULD render it accordingly (e.g. Buy-only, no Install action), the same way an unsupported platform is already handled.

A publishing client enforcing the fulfillment tier SHOULD, before allowing submit:

1. Compute `file_hash` client-side from the selected build file rather than accepting it as freeform text — a hand-typed hash is a guaranteed source of publish/upload mismatches.
2. If `fulfillment_pubkey` is the developer's own identity key (direct signing), no further steps are needed. If it delegates to an operator, this is the moment the developer's client calls `POST /provision` (§[Provisioning Endpoints](#provisioning-endpoints)) against the chosen operator — reusing an existing key for this scope if one is already active, or obtaining a new one otherwise — then independently constructs and publishes the matching `kind:30406` Provisioning Acceptance. The developer's signer is already active for the listing's own signature in this same session, so this is one more signature in an already-open flow, not a separate round trip.
3. For each declared `server` URL, fetch `GET <server>/.well-known/adp` live as a basic reachability check. This confirms the URL is live and *a* server, but is not a substitute for step 2 — a server's single announced identity pubkey has no fixed relationship to any individual developer's delegated fulfillment key once an operator serves more than one developer.

A listing published through a different tool, without these fields, is not invalid — it is simply outside what this client's automated fulfillment can act on. Sellers who wish to use a different publishing interface and still support ADP fulfillment need to supply these fields by whatever means that interface offers (manual tag entry, a companion tool, etc.); this spec does not mandate any particular publishing UI.

## The `kind:30403` Server Announcement Event

ADP servers publish a parameterized replaceable event to announce themselves. Clients and developers query relays for `kind:30403` to discover available servers.

| Tag | Value | Description |
|-----|-------|--------------|
| `d` | `<server-slug>` | Replaceable-event identifier |
| `name` | Human-readable server name | |
| `url` | `<https-url>` | Base URL for HTTP endpoints |
| `supported_adp` | `<semver-string>` | ADP protocol version supported |
| `contact` | Free text | Operator contact |

### Example Event

```jsonc
{
  "kind": 30403,
  "pubkey": "<server-pubkey>",
  "tags": [
    ["d", "arcadestr-dist"],
    ["name", "Arcadestr Official Distribution"],
    ["url", "https://dist.arcadestr.io"],
    ["supported_adp", "0.2.0"],
    ["contact", "ops@arcadestr.io"]
  ],
  "content": "",
  "sig": "..."
}
```

## Fulfillment Key Delegation

A `kind:1020` receipt (defined in [NIP‑102](NIP-102.md)) MUST be signed either by the listing's own `pubkey`, or by a key the listing has delegated via a `fulfillment_pubkey` tag. Delegation lets a developer run (or subscribe to) automated fulfillment infrastructure without exposing their primary identity key.

### Validity interval

A fulfillment key `K` is authorized to sign a receipt with `created_at = T` if and only if a `["fulfillment_pubkey", K, valid_from, revoked_at]` tag exists on the listing such that:

```
valid_from <= T   AND   (revoked_at is empty OR revoked_at > T)
```

Listings MAY carry multiple `fulfillment_pubkey` tags representing rotation history; each is evaluated independently — there is no ordering dependency between tags.

To rotate keys, the developer republishes the listing with a `revoked_at` on the outgoing tag and a new tag for the incoming key. To revoke a compromised key immediately, without a replacement ready, the developer republishes the listing with only `revoked_at` set on the compromised tag.

**Backdating clamp.** `revoked_at` is a value inside a tag — relays enforce no semantic meaning on it, only ordering on the wrapping event's own `created_at`. Nothing stops a developer from republishing a listing today with `revoked_at` claiming some date months ago, which would retroactively invalidate every receipt signed since then — including a buyer's completely legitimate purchase, failing on a later reinstall (Path B) through no fault of their own. Verifiers MUST NOT trust a `revoked_at` value below the listing event's own `created_at`:

```
effective_revoked_at = max(declared revoked_at, listing_event.created_at)
```

All validity-interval checks in this spec use `effective_revoked_at`, never the raw tag value. This preserves scheduling a *future* revocation while making backdating structurally unusable: since relays reject a replaceable-event update whose `created_at` is older than what they already store (per standard Nostr replaceable-event semantics), the `max()` always resolves to the real publish time whenever a developer attempts to claim an earlier one. This does depend on relays actually enforcing that monotonicity — a single non-compliant relay could still serve a backdated version — which is exactly why the freshness rule below queries more than one.

`valid_from` does not need the same clamp: backdating it to retroactively *grant* authority that didn't exist already requires controlling the developer's identity key to add the fraudulent delegation in the first place, at which point the attacker has a far larger problem than this field.

### Updated Proof of Ownership

This extends [NIP‑102](NIP-102.md)'s Proof-of-Ownership condition ("signed by a pubkey the buyer trusts as the merchant") to:

```
signature(E_1020) is valid
  AND (
        E_1020.pubkey == listing.pubkey
        OR
        E_1020.pubkey is authorized per a fulfillment_pubkey tag's
        validity interval (using effective_revoked_at), evaluated
        at E_1020.created_at
      )
```

**Freshness.** Verifiers MUST NOT rely on a single relay's copy of the listing before authorizing a security-relevant action such as unlocking a download. Query at least two independent relays and use the returned version with the highest `created_at` — a single relay, compromised or merely stale, could otherwise serve an outdated or manipulated copy despite the clamp above. A verifier MAY additionally cache the listing it fetched at the moment of purchase alongside the receipt; doing so lets that specific purchase be re-verified later using only the immutable, developer-signed snapshot it already holds, with no relay dependency at all — this is a resilience option, not a requirement, since requiring buyers to retain anything locally would work against NIP-102's own goal of ownership proof being fully recoverable from relays alone.

### Fulfillment Provisioning Attestation (`kind:30404`) and Provisioning Acceptance (`kind:30406`)

`fulfillment_pubkey` alone answers "does the listing authorize this key" — it does not answer "did the key's actual holder (an operator, potentially serving many unrelated developers) and the developer actually agree it's provisioned on the developer's behalf." That second question matters whenever a developer didn't generate the fulfillment key themselves — most commonly, when using a **hosted ADP operator** rather than self-hosting.

This is established by **two events, from two different signers, that MUST agree with each other**: the operator's attestation that it holds the key for that developer, and the developer's own acceptance that they requested it from that specific operator. Provisioning happens at the moment a developer is about to publish a game through that operator, so the developer's signer is already active for the listing's own signature in the same session — the counter-signature adds one more signing round-trip to a session that's already happening, not a separate one.

**`kind:30404` — Fulfillment Provisioning Attestation, signed by the operator:**

| Tag | Value | Description |
|-----|-------|--------------|
| `d` | `<developer-pubkey>:<fulfillment-pubkey>` | Replaceable-event identifier — unique per (developer, fulfillment key) pair, so an operator can hold multiple independently-revocable attestations per developer if needed (e.g. per-game key isolation — see `scope` below). |
| `p` | `<developer-pubkey>` | The developer this key is provisioned for. Indexed for efficient relay lookup once a specific `(operator, developer, fulfillment key)` triple is already known — **not** intended as a general discovery mechanism; see [Trust Boundary](#trust-boundary) below. |
| `fulfillment_pubkey` | `<pubkey-hex>`, `<valid_from>`, `<revoked_at>` | Same validity-interval shape as the listing-side tag (§[Fulfillment Key Delegation](#fulfillment-key-delegation)) — the operator's own claim of when it started, and (if applicable) stopped, holding this key. |
| `scope` | Free text | Optional. The label the developer requested at provisioning time (§[Provisioning Endpoints](#provisioning-endpoints)) — e.g. a game slug, letting a developer hold multiple independent keys with the same operator, one per game. Purely informational; verifiers MUST NOT give it any security meaning. |

```jsonc
{
  "kind": 30404,
  "pubkey": "<operator-pubkey>",
  "tags": [
    ["d", "<developer-pubkey>:<fulfillment-pubkey>"],
    ["p", "<developer-pubkey>"],
    ["fulfillment_pubkey", "<fulfillment-pubkey>", "1700000000", ""],
    ["scope", "my-game-slug"]
  ],
  "content": "",
  "sig": "<operator-signature>"
}
```

**`kind:30406` — Provisioning Acceptance, signed by the developer:**

| Tag | Value | Description |
|-----|-------|--------------|
| `d` | `<operator-pubkey>:<fulfillment-pubkey>` | Replaceable-event identifier — unique per (operator, fulfillment key) pair from the developer's side. |
| `p` | `<operator-pubkey>` | The operator the developer is accepting provisioning from. |
| `fulfillment_pubkey` | `<pubkey-hex>` | The key the developer accepts as provisioned by that operator. No validity interval here — if a developer wants to walk away from an operator, the action that matters is revoking the delegation on their own listing (§[Fulfillment Key Delegation](#fulfillment-key-delegation)), which they already control unilaterally. This event only ever needs to say "yes, I asked for this," not track its own lifecycle. |

```jsonc
{
  "kind": 30406,
  "pubkey": "<developer-pubkey>",
  "tags": [
    ["d", "<operator-pubkey>:<fulfillment-pubkey>"],
    ["p", "<operator-pubkey>"],
    ["fulfillment_pubkey", "<fulfillment-pubkey>"]
  ],
  "content": "",
  "sig": "<developer-signature>"
}
```

**Neither event is meaningful alone.** A `(operator, developer, fulfillment key)` triple is only actionable once **both** a matching `kind:30404` (from the operator) and a matching `kind:30406` (from the developer, naming that same operator and key) exist on relays. There is no ordering requirement — Nostr gives no guarantee either publishes first — a publishing client checks for both before treating the provisioning as confirmed.

**Both events are a publish-time sanity check only — neither is ever consulted during receipt verification.** A `kind:1020` receipt's validity depends solely on the listing's own `fulfillment_pubkey` tag (§[Updated Proof of Ownership](#updated-proof-of-ownership)); pulling either of these into that check would break the listing's self-containment that every other ADP trust concern relies on. Their purpose is narrower: before publishing a listing that delegates to a key the developer didn't generate themselves, a publishing client SHOULD query relays for both a `kind:30404` matching `["d", "<developer-pubkey>:<fulfillment-pubkey>"]` with `pubkey == <operator-pubkey>`, and a `kind:30406` matching `["d", "<operator-pubkey>:<fulfillment-pubkey>"]` with `pubkey == <developer-pubkey>`, confirm the operator attestation's validity interval currently covers "now," and only then proceed. This catches a copy-paste error or a stale/revoked provisioning before the listing goes out with a key the operator no longer actually holds for that developer — the same failure mode `fulfillment_pubkey`'s own `revoked_at` prevents on the listing side, one step earlier in the chain.

When `fulfillment_pubkey == developer's own identity key` (direct signing, no delegation to any third party), neither event is needed or expected — there is no operator relationship to attest to.

### Trust boundary

Requiring both signatures closes the gap a unilateral attestation would otherwise leave open, rather than merely mitigating it:

- **Neither event alone can forge a listing, a receipt, or a provisioning relationship.** A `kind:30402` listing's identity is its own signature — an operator cannot produce a valid listing under a developer's pubkey without that developer's private key, regardless of what any `kind:30404`/`kind:30406` pair claims. And since receipt verification never consults either event, a fabricated one cannot authorize a fraudulent purchase either.
- **A rogue operator publishing a fake `kind:30404` naming an uninvolved developer produces an incomplete pair — it fails validation outright**, since no matching `kind:30406` from that developer will exist. This is a structural guarantee, not a client-discipline convention: it holds even if a client's relay query is scoped carelessly, because the missing developer signature makes the triple non-actionable regardless of how it's discovered.
- **A rogue developer-side claim has the same limit in the other direction** — a `kind:30406` naming an operator who never actually issued that key simply has no matching `kind:30404` to pair with, so it's equally inert.
- Client implementations SHOULD still prefer scoping lookups to an operator pubkey obtained out-of-band (the same way you'd trust you're on an operator's real website rather than a phishing clone) over broadly discovering "any pair mentioning me" — this is now a defense-in-depth practice rather than the sole protection, since the mutual-signature requirement is what actually rules out forgery.

## LSP Verification (`lud16`)

Confirming a `kind:9735` zap receipt's signature and its `p`/`a` tag references is not sufficient proof of payment on its own — those checks alone accept any self-signed event with the right shape. A server MUST additionally confirm the receipt was signed by the listing's actual Lightning Service Provider (LSP), resolved as follows:

1. Read the listing's `lud16` tag (`name@domain`).
2. Resolve it to an LNURL-pay endpoint per [LUD-16](https://github.com/lnurl/luds/blob/luds/16.md): `https://<domain>/.well-known/lnurlp/<name>`.
3. Fetch the endpoint and extract its declared `nostrPubkey`, per [NIP‑57](57.md)'s zap metadata requirements.
4. Require `zap_receipt.pubkey == nostrPubkey`.

This resolution **MUST** use the `lud16` tag on the listing event itself — **never** a fallback to the developer's `kind:0` profile metadata. A listing's `lud16` is the single, self-contained source of payment-routing authority for that listing, consistent with how `file_hash`, `server`, and `fulfillment_pubkey` are already scoped to the listing rather than to a separately mutable profile event. A developer who changes their profile `lud16` does not thereby change which LSP is authoritative for an already-published listing; they must republish the listing to change it.

If a listing has no `lud16` tag, the zap-receipt payment path MUST NOT be accepted for it — this is a hard failure (`402`), not a silent skip of the LSP check. The bolt11/preimage self-contained proof path has no LSP-trust dependency and remains available regardless.

## HTTP Endpoints

All endpoints requiring authentication use `Authorization: Nostr <base64-nip98-event>` per [NIP‑98](98.md).

### `GET /.well-known/adp`

Public. No auth required.

**Response `200`:**
```json
{
  "adp_version": "0.2.0",
  "pubkey": "<server-pubkey>",
  "name": "Arcadestr Official Distribution",
  "url": "https://dist.arcadestr.io"
}
```

### Provisioning Endpoints

These exist so any developer can onboard to any ADP-compliant hosted operator through the same flow, rather than each operator inventing its own bespoke API — the same interoperability goal that motivates every other part of this spec.

### `POST /provision`

Developer-authenticated. Requests the operator provision (or return an existing) fulfillment key for the calling developer, optionally scoped for per-game key isolation.

**Auth:** NIP‑98 token. The developer's identity comes from the token, never the request body.

**Request body (`application/json`):**
```json
{
  "scope": "my-game-slug"
}
```
`scope` is optional; omit it to request a default, unscoped key.

**Server MUST:**
1. Verify the NIP‑98 token.
2. Check whether an active key already exists for `(developer_pubkey, scope)`. If so, return it unchanged — this endpoint is idempotent, not a mint-on-every-call.
3. Otherwise, generate a new keypair dedicated to this `(developer_pubkey, scope)` pair. A key MUST NOT be reused across different developers, and MUST NOT be reused across different scopes requested by the same developer.
4. Publish a `kind:30404` attestation for `(developer_pubkey, fulfillment_pubkey)` with `valid_from` set to now, including the `scope` tag if one was given.
5. Persist the mapping, with the private key encrypted at rest (see [Security Considerations](#security-considerations)).

**Response `200`:**
```json
{
  "fulfillment_pubkey": "<hex>",
  "attestation_event_id": "<hex>",
  "scope": "my-game-slug"
}
```

The operator never constructs or publishes the `kind:30406` half — that remains the developer's own client's responsibility, preserving the mutual-attestation property from [Trust Boundary](#trust-boundary).

### `POST /provision/revoke`

Developer-authenticated. Requests the operator stop holding and using a previously provisioned key.

**Auth:** NIP‑98 token. The server MUST confirm the calling developer owns the named key per its own records before acting.

**Request body (`application/json`):**
```json
{
  "fulfillment_pubkey": "<hex>"
}
```

**Server MUST:**
1. Republish the corresponding `kind:30404` with `revoked_at` set to now.
2. Cease signing anything new with this key, and SHOULD securely delete or rotate it out of active storage.

**This endpoint is an operational and audit convenience — it does not, by itself, invalidate any past or future receipt.** The check a buyer's ownership verification actually depends on is the listing's own `fulfillment_pubkey` `revoked_at` tag (§[Fulfillment Key Delegation](#fulfillment-key-delegation)), which the developer already controls unilaterally regardless of whether any operator cooperates or is even reachable. A developer whose operator has gone offline can still fully revoke a delegation by editing their own listing; this endpoint exists so the operator's own records — and the public `kind:30404` audit trail — stay honest about what it still holds.

### `POST /upload`

Developer-authenticated. Uploads a game build.

**Auth:** NIP‑98 token; `pubkey` MUST match the `pubkey` of the included `listing_event`, or a currently-authorized `fulfillment_pubkey` on that listing.

**Request body (`multipart/form-data`):**
```
listing_event: <JSON of signed kind 30402 event>
file: <binary game archive>
```

**Server MUST:**
1. Verify `listing_event.sig`.
2. Verify the uploader's authorization per [Fulfillment Key Delegation](#fulfillment-key-delegation) or direct developer signature.
3. Verify the uploaded file's SHA‑256 matches `file_hash` in `listing_event`.
4. Store the file indexed by `game_coordinate`.

**Response `200`:**
```json
{
  "game_coordinate": "30402:<developer-pubkey>:<d-tag>",
  "file_hash": "<sha256-hex>",
  "download_url": "https://dist.arcadestr.io/game/30402:<developer-pubkey>:<d-tag>"
}
```

### `POST /purchase/confirm`

Buyer-authenticated. Submits payment proof; receives a signed NIP‑102 receipt and a download authorization.

**Auth:** NIP‑98 token. The buyer's identity is derived from the token's `pubkey`; no buyer identity fields are accepted from the request body.

**Request body (`application/json`):**
```json
{
  "game_coordinate": "30402:<developer-pubkey>:<d-tag>",
  "listing_event": { "...": "signed kind 30402 event" },
  "zap_receipt_event": { "...": "signed kind 9735 event" }
}
```

`zap_receipt_event` MAY be replaced with `bolt11` + `preimage` fields, per NIP‑102's payment-proof flexibility.

**Server MUST verify:**

1. NIP‑98 token: `kind = 27235`, correct `u` tag URL, correct `method` tag, timestamp within 60 seconds of server clock.
2. `listing_event.sig` is valid.
3. `listing_event.pubkey` is the expected developer for the claimed `game_coordinate`.
4. The server's own fulfillment key is currently authorized per the listing's `fulfillment_pubkey` tags (or the server holds the developer's own key).
5. Payment proof is valid: `zap_receipt_event.sig` valid, buyer `P` tag matches the NIP‑98 token pubkey, an `a`/`e` tag references the claimed `game_coordinate` with a matching amount, **and** `zap_receipt_event.pubkey` matches the LSP resolved from the listing's `lud16` tag per [LSP Verification](#lsp-verification-lud16) — or the `bolt11`/`preimage` pair validates per NIP‑102.
6. The payment proof event id has not been used in a prior confirmed purchase (replay protection).

**Server MUST on success:**

1. Generate an `order` id (UUID v4).
2. Construct a `kind:1020` receipt: `order`, `p` (buyer), `a` (`game_coordinate`), payment-proof tags, `status: paid`, `amount`, `currency`.
3. Encrypt the itemized order JSON to the buyer's pubkey per [NIP‑44](44.md) and set it as `.content`.
4. Sign with the fulfillment key (or developer key) and publish to relays.
5. Persist `(order_id, payment_proof_event_id, buyer_pubkey, game_coordinate)` with a unique constraint on `order_id`, satisfying NIP‑102's idempotency requirement.

**Response `200`:**
```json
{
  "receipt": { "...": "signed kind 1020 event" },
  "download_token": "<opaque-token>",
  "token_expires_at": 1234567890
}
```

**Error responses:**

| Status | Reason |
|---|---|
| `400` | Malformed request body |
| `401` | NIP‑98 token invalid or expired |
| `402` | Payment verification failed (including: zap receipt not signed by the listing's resolved LSP, or listing has no `lud16` and only a zap receipt was presented) |
| `409` | Payment proof already used (replay) |
| `404` | Game not hosted on this server |
| `500` | Server's fulfillment key is not currently authorized for this listing |

### `GET /game/:game_coordinate`

Buyer-authenticated. Streams the game archive. Requires both an ownership check and a distribution-authorization check to pass independently.

**Auth:** NIP‑98 token, or `?token=<download_token>` from `/purchase/confirm`.

#### Ownership check

- **Path A — download token:** valid, unexpired, issued for this `game_coordinate` to this buyer.
- **Path B — receipt query (portable):** query relays for `kind:1020` events with `#p` = buyer pubkey and `#a` = `game_coordinate`. For the latest status in the order chain, verify the signer's authorization per [Fulfillment Key Delegation](#fulfillment-key-delegation), fetching the listing from multiple relays and using the highest `created_at` result per [Updated Proof of Ownership](#updated-proof-of-ownership).

Path B requires no relationship between the purchasing server and the serving server — the receipt and its delegation are independently verifiable by anyone.

#### Distribution authorization

Independent of ownership: the serving server MUST confirm its own URL appears in the listing's `server` tags for this `game_coordinate`. A server not listed there MUST NOT serve the file, regardless of a presented receipt's validity.

**Response `200`:** binary game archive stream.
**Response `304`:** not modified (conditional GET SHOULD be supported).

**Error responses:**

| Status | Reason |
|---|---|
| `403` | Ownership check failed |
| `451` | Server not authorized to distribute this file |

## Provisioning Flow

```
Developer Client                ADP Server (operator)       Nostr Relays
─────────────────────────────────────────────────────────────────────────
1. Construct NIP-98 token
2. POST /provision ─────────────────────────────────────→
   { scope: "my-game-slug" }
                               3. Check for existing active
                                  key at this scope
                               4. Generate keypair (if none)
                               5. Publish kind:30404 ────────────────────→
                               6. Persist mapping, encrypted
                               7. Return fulfillment_pubkey,
   ←── attestation_event_id ──────
8. Construct + sign kind:30406
   (developer's own key)
9. Publish kind:30406 ────────────────────────────────────────────────────→
10. Add fulfillment_pubkey tag
    to kind:30402 listing,
    sign, publish ─────────────────────────────────────────────────────→
```

## Purchase Flow

```
Buyer Client                    ADP Server                  Nostr Relays
─────────────────────────────────────────────────────────────────────────
1. Pay Lightning invoice
2. Poll for kind:9735 zap receipt                        ← kind 9735 appears
3. Construct NIP-98 token
4. POST /purchase/confirm ──────────────────────────→
                               5. Verify NIP-98, listing, payment proof
                               6. Verify fulfillment key authorization
                               7. Build + NIP-44 encrypt kind:1020 receipt
                               8. Sign with fulfillment key
                               9. Publish ──────────────────────────────→
                               10. Return receipt + download_token ────→
11. GET /game/:coordinate ──────────────────────────→
    ?token=<download_token>
                               12. Verify token (ownership Path A)
                               13. Verify server tags (distribution auth)
                               14. Stream archive ──────────────────────→
15. Save to disk
```

## Portable Download Flow (different server, no prior relationship)

```
Buyer Client                    New ADP Server              Nostr Relays
─────────────────────────────────────────────────────────────────────────
1. Construct NIP-98 token
2. GET /game/:coordinate ──────────────────────────→
   Authorization: Nostr <token>
                               3. No download token; use ownership Path B
                               4. Query kind:1020 receipts by #p + #a ──→
                                                                      ←──
                               5. Fetch listing from ≥2 relays,
                                  take highest created_at ────→
                                                                      ←──
                               6. Verify signer per fulfillment_pubkey
                                  validity interval
                               7. Verify server URL in listing's
                                  server tags (distribution auth)
                               8. Stream archive ───────────────────────→
9. Save to disk
```

## Trust Model

Ownership proof (the `kind:1020` receipt) and distribution authorization (`server` tags) are deliberately separate. A receipt is self-sovereign: any party can verify it against the listing without trusting the server that issued it. `server` tags are a distinct, developer-controlled allowlist of who may hold and serve copies of the file — a server MUST NOT infer distribution rights from a valid receipt alone, since that would let any node holding a leaked copy of the archive serve it to anyone with proof of purchase, regardless of whether the developer sanctioned that node as a mirror.

## Security Considerations

- **Replay attacks:** servers MUST store the payment-proof event id used in each confirmed purchase and reject reuse, even against a different buyer.
- **Receipt forgery:** payment proofs (zap receipts, bolt11/preimage) are independently verifiable; a server does not trust the buyer's bare assertion of payment.
- **Fulfillment key custody:** every fulfillment key MUST be distinct per developer — an operator serving multiple developers MUST NOT let two developers share a key, since compromising it would compromise every developer behind it at once. Keys must remain online to support non-interactive fulfillment; each key's capability is scoped to signing receipts for its one developer, so compromise doesn't expose that developer's listing-publishing or social identity, and is recoverable via `revoked_at`.
- **Multi-tenant key storage:** an operator holding many developers' fulfillment keys MUST encrypt them at rest (e.g. AES-256-GCM under a server-wide master key, at minimum) — this store is now a considerably higher-value target than any single self-hosted key, since its compromise cascades across every developer the operator serves. An operator running at meaningful scale SHOULD evaluate an HSM or KMS rather than relying on a bare master-key secret.
- **Backdated revocation:** `revoked_at` is developer-controlled tag content, not something relays give any special meaning to — verifiers MUST apply the clamp in [Validity interval](#validity-interval) (`effective_revoked_at = max(declared, listing_event.created_at)`) rather than trusting the raw value, or a developer could retroactively invalidate their own buyers' legitimate past receipts.
- **Delegation freshness:** verifiers MUST query multiple independent relays and use the highest `created_at` result, not a single relay's response, before authorizing downloads based on delegation — see [Updated Proof of Ownership](#updated-proof-of-ownership).
- **NIP‑98 token scope:** the `u` tag must exactly match the request URL including path; a token for `/purchase/confirm` MUST NOT be accepted for `/game/:id`.
- **File integrity:** `file_hash` is a developer commitment. Servers SHOULD periodically re-verify stored files against it.

## Relationship to Other NIPs

| NIP | Role |
|-----|------|
| [NIP‑99](99.md) | Classified listing (`kind:30402`) carrying `server`, `file_hash`, `version`, `platform`, `fulfillment_pubkey`, and `lud16` tags |
| [NIP‑102](NIP-102.md) | Marketplace receipt (`kind:1020`) — the ownership credential this protocol relies on |
| [NIP‑98](98.md) | HTTP Auth for all authenticated endpoints |
| [NIP‑44](44.md) | Encryption of receipt `.content` |
| [NIP‑57](57.md) | Zap Request/Receipt as payment proof; its LSP metadata is what [LSP Verification](#lsp-verification-lud16) checks a zap receipt's signer against |

## Implementation Notes

- **`kind:30403` storage:** relays SHOULD retain server announcement events; they are replaceable and low-frequency.
- **Multi-server redundancy:** multiple `server` tags on one listing declare independent, redundant distribution points; none is privileged over another.
- **Version negotiation:** `supported_adp` in `/.well-known/adp` is advisory; no negotiation protocol is defined in this draft.

## Reference Implementation Checklist

For a server to be considered ADP‑01 compliant it SHOULD implement:

- [ ] Serving `/.well-known/adp` and publishing a `kind:30403` announcement.
- [ ] Verifying uploader authorization (developer key or active `fulfillment_pubkey`) before accepting `/upload`.
- [ ] Verifying file SHA‑256 against `file_hash` on upload and periodically thereafter.
- [ ] Verifying NIP‑98, listing signature, and payment proof before issuing a receipt in `/purchase/confirm`.
- [ ] Resolving the listing's `lud16` tag (never the developer's profile) to an LSP `nostrPubkey` and confirming the zap receipt was signed by it, before accepting the zap-receipt payment path — see [LSP Verification](#lsp-verification-lud16).
- [ ] Enforcing idempotency on `order` id and replay protection on the payment-proof event id.
- [ ] Signing receipts with a key currently authorized per the listing's `fulfillment_pubkey` validity interval, using `effective_revoked_at` — see [Validity interval](#validity-interval).
- [ ] Implementing `POST /provision` and `POST /provision/revoke` (§[Provisioning Endpoints](#provisioning-endpoints)), publishing a `kind:30404` attestation per provisioned key, with a distinct key per `(developer, scope)` pair — never a key shared across developers.
- [ ] Encrypting stored fulfillment private keys at rest.
- [ ] Fetching listings from multiple independent relays and using the highest `created_at` result (not a single relay's response, and not a local cache) before authorizing downloads via delegation.
- [ ] Evaluating ownership (Path A/B) and distribution authorization (`server` tags) as independent checks in `/game/:game_coordinate`.
- [ ] Rejecting NIP‑98 tokens whose `u` tag does not match the exact requested path.

## Open Questions

- **Revocation propagation latency:** no SLA is defined for how quickly relays/servers are expected to observe a `revoked_at` update, on either the listing side or the `kind:30404` side.
- **Operator offboarding notification:** when an operator calls `POST /provision/revoke`, nothing in this spec requires notifying the affected developer that their key was revoked operator-side — a developer could be left unaware their listing's `fulfillment_pubkey` now points at a key the operator has stopped honoring, until a buyer's purchase fails. Left to individual operator UX for now.
- **Developer counter-signature liveness:** `kind:30406` has no expiry of its own (see rationale in its tag table) — if this proves insufficient in practice (e.g. an operator wants to prove a developer's acceptance is *current*, not just that it once happened), a `valid_from`/`revoked_at` pair could be added to it later, mirroring `kind:30404`.
- **Refunds:** no refund flow is defined; left to individual server operators, informed by NIP‑102's `status: refunded` receipt chain.