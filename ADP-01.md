# ADP-01: Arcadestr Distribution Protocol

`draft` `optional`

A minimal, open HTTP protocol for serving gated digital game files, using Nostr identities and event kinds as the authentication and ownership layer. Any operator can run a compliant ADP server; game developers choose which servers carry their builds. Ownership is proven via a [NIP‑102](NIP-102.md) `kind:1020` receipt, signed either by the developer's own key or by a key the developer has delegated for fulfillment. This credential is portable across servers and does not depend on any single server remaining online.

ADP is modeled after [Blossom](https://github.com/hzrd149/blossom): servers are interchangeable, the protocol is the contract, and no central operator is privileged.

## Terminology

| Term | Definition |
|------|------------|
| **ADP server** | An HTTP server implementing this spec |
| **Developer** | The Nostr keypair that signed the game's [NIP‑99](99.md) listing event |
| **Fulfillment key** | A Nostr keypair authorized through a developer-signed `kind:30406` lifecycle for explicit operational capabilities |
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
| `fulfillment_authorization` | `<authorization-root-event-id>`, `<fulfillment-pubkey>`, `<relay-hint>` | Enables new operations under the referenced `kind:30406` authorization. Repeatable; relay hint optional. The authorization chain, not the listing, owns validity and capabilities. |
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
    ["fulfillment_authorization", "<authorization-root-1>", "a1b2...", "wss://relay.example.com"],
    ["fulfillment_authorization", "<authorization-root-2>", "c3d4..."],
    ["lud16", "studio@getalby.com"]
  ],
  "content": "...",
  "sig": "<developer-signature>"
}
```

`server` tags declare **distribution authorization only** — which servers may serve the file. They are independent of ownership and fulfillment-key authorization; see [Distribution Authorization](#distribution-authorization) below.

### Mandatory fields for client-published listings

Nothing in this section is a protocol-level restriction — any Nostr client can publish a `kind:30402` event with any subset of these tags, and relays MUST accept it regardless. This is guidance for a **publishing client's own UI**, not a wire-format requirement: a client that wants a listing to be immediately usable through ADP SHOULD enforce the following before allowing publish, and MUST clearly communicate to the buyer-facing side of the client when a listing lacks them, rather than silently degrading.

Two tiers, since they fail differently:

- **Payment-address tier — required for any priced listing, independent of ADP entirely:** `lud16`. Every priced listing needs a payment address regardless of whether ADP is involved; ADP additionally requires it be resolvable and stable per [LSP Verification](#lsp-verification-lud16), so a client SHOULD treat it as mandatory at publish time rather than optional.
- **Fulfillment tier — required only for listings that want automated install/download:** `file_hash`, `server` (≥1), and at least one applicable `fulfillment_authorization` unless the developer signs every operation directly. A listing missing these can still be a valid, purchasable listing — it simply cannot support one-click install, and a client SHOULD render it accordingly (e.g. Buy-only, no Install action), the same way an unsupported platform is already handled.

A publishing client enforcing the fulfillment tier SHOULD, before allowing submit:

1. Compute `file_hash` client-side from the selected build file rather than accepting it as freeform text — a hand-typed hash is a guaranteed source of publish/upload mismatches.
2. For direct developer signing, no fulfillment authorization is required. For delegated operation, the client calls `POST /provision`, verifies the matching operator-signed `kind:30404`, creates or reuses a coordinate-scoped `kind:30406` authorization root with explicit capabilities, publishes it, and adds the corresponding `fulfillment_authorization` listing tag.
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

## Fulfillment Authorization

A delegated fulfillment key derives authority from a developer-signed `kind:30406` authorization lifecycle. The current listing only enables an authorization for new operations; it does not contain or duplicate the lifecycle's validity timestamps.

A listing may enable multiple active authorizations:

```text
["fulfillment_authorization", "<authorization-root-id>", "<fulfillment-pubkey>", "<relay-hint>"]
```

The relay hint is optional. The root ID and fulfillment pubkey are both included so clients can reject mismatched references before graph traversal.

### Scoped capabilities

Each authorization root MUST contain at least one recognized capability. This version defines:

- `issue_receipt` — sign a NIP-102 root receipt;
- `issue_grant` — sign an Entitlement Grant root under an independently valid campaign or issuance policy;
- `upload_build` — authenticate and upload a build for the coordinate.

Capabilities are explicit and fail closed. Unknown capability values grant no authority. Capabilities are invariant across an authorization chain; adding, removing, or changing capabilities requires a new authorization root. Cancellation affects every capability in that root.

### Current issuance versus historical verification

A new delegated operation at time `T` requires both:

1. the current listing contains a matching `fulfillment_authorization` tag; and
2. the referenced authorization chain is active at `T` and contains the required capability.

A previously issued delegated credential remains valid without current listing membership when:

1. it contains `['authorization', '<root-id>']`;
2. its signer matches the fulfillment key in that root;
3. the root is bound to the same developer and game coordinate;
4. the authorization was active and had the required capability at the credential root's `created_at`.

Removing an authorization from a replacement listing prevents new issuance but MUST NOT retroactively invalidate credentials created while it was active.

Direct developer-signed receipts, grants, and uploads require no `kind:30406` event and delegated credentials without an `authorization` tag are invalid. No legacy listing-delegation fallback is defined.

### Fulfillment Provisioning Attestation (`kind:30404`)

`kind:30404` remains an operator-signed, parameterized replaceable attestation that the operator provisioned and claims possession of a fulfillment key for a developer. It is checked during provisioning and authorization creation, but ordinary historical credential verification relies on the developer-signed `kind:30406` lifecycle.

A matching `30404` MUST name the same operator, developer, and fulfillment key before a publishing client creates a delegated authorization. Operator revocation means the operator claims it no longer holds or uses the key; it does not retroactively invalidate credentials and does not replace developer cancellation of `30406`.

### Fulfillment Authorization (`kind:30406`)

`kind:30406` is a **regular, non-replaceable** developer-signed authorization event. A root and its chained updates form an append-only lifecycle.

#### Root tags

| Tag | Value | Description |
|---|---|---|
| `d` | `<authorization-id>` | Stable authorization identifier, SHOULD be UUID v4. Required and preserved. |
| `a` | `30402:<developer-pubkey>:<d-tag>` | Exact game coordinate. Required and preserved. |
| `p` | `<operator-pubkey>` | Operator holding the fulfillment key. Required and preserved. |
| `fulfillment_pubkey` | `<pubkey-hex>` | Authorized operational key. Required and preserved. |
| `capability` | `issue_receipt` \| `issue_grant` \| `upload_build` | Repeatable; at least one required and preserved. |
| `valid_from` | `<unix-seconds>` | Inclusive beginning of authority. Required and preserved. MUST NOT precede the root's `created_at`. |
| `status` | `active` \| `cancelled` | Root MUST use `active`. |
| `e` | `<immediate-predecessor-id>` | Absent on root; exactly one on updates. |

```jsonc
{
  "kind": 30406,
  "pubkey": "<developer-pubkey>",
  "created_at": 1700000000,
  "tags": [
    ["d", "<authorization-id>"],
    ["a", "30402:<developer-pubkey>:my-game-v1"],
    ["p", "<operator-pubkey>"],
    ["fulfillment_pubkey", "<fulfillment-pubkey>"],
    ["capability", "issue_receipt"],
    ["capability", "issue_grant"],
    ["capability", "upload_build"],
    ["valid_from", "1700000000"],
    ["status", "active"]
  ],
  "content": "",
  "sig": "<developer-signature>"
}
```

#### Cancellation

An update may only cancel the authorization. It MUST preserve `d`, `a`, `p`, `fulfillment_pubkey`, all `capability` values, and `valid_from`; contain exactly one immediate predecessor `e`; and use `status: cancelled`.

Cancellation is prospective:

```text
effective_cancelled_at = cancellation_event.created_at
```

A credential created before that timestamp remains valid. A credential created at or after it is unauthorized. Backdated cancellation fields are not defined or accepted.

#### Chain resolution

Authorization chains use the valid-prefix model:

- an event joins the valid chain only when its signature, invariant fields, transition, and immediate predecessor are valid;
- an invalid event does not alter state and is excluded from valid-successor fork analysis;
- descendants through an invalid event are unreachable;
- the developer may recover with a valid sibling referencing the last valid tip;
- multiple valid successors of one valid predecessor are a fork and fail closed;
- cycles and self-references are invalid;
- verifiers MUST NOT resolve ambiguity by `created_at`.

The authorization is active at time `T` when `valid_from <= T` and no valid cancellation has `created_at <= T`.

### Relay availability

A verifier MUST distinguish invalid authorization from insufficient evidence. An invalid, mismatched, or cancelled chain is rejected. Incomplete relay coverage or a missing required predecessor produces an indeterminate/unavailable result, not a declaration that the credential is cryptographically unauthorized.

## LSP Verification (`lud16`)

Confirming a `kind:9735` zap receipt's signature and its `p`/`a` tag references is not sufficient proof of payment on its own — those checks alone accept any self-signed event with the right shape. A server MUST additionally confirm the receipt was signed by the listing's actual Lightning Service Provider (LSP), resolved as follows:

1. Read the listing's `lud16` tag (`name@domain`).
2. Resolve it to an LNURL-pay endpoint per [LUD-16](https://github.com/lnurl/luds/blob/luds/16.md): `https://<domain>/.well-known/lnurlp/<name>`.
3. Fetch the endpoint and extract its declared `nostrPubkey`, per [NIP‑57](57.md)'s zap metadata requirements.
4. Require `zap_receipt.pubkey == nostrPubkey`.

This resolution **MUST** use the `lud16` tag on the listing event itself — **never** a fallback to the developer's `kind:0` profile metadata. A listing's `lud16` is the single, self-contained source of payment-routing authority for that listing, consistent with how `file_hash`, `server`, and `fulfillment_authorization` are already scoped to the listing rather than to a separately mutable profile event. A developer who changes their profile `lud16` does not thereby change which LSP is authoritative for an already-published listing; they must republish the listing to change it.

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

The operator never constructs or publishes `kind:30406`; the developer client creates the coordinate-scoped authorization with explicit capabilities after validating the operator attestation.

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

**This endpoint is operational evidence only.** It stops this operator from using the key and updates `30404`; developer cancellation of the corresponding `30406` is the normative prospective authorization revocation. Existing earlier credentials remain valid.

### `POST /upload`

Developer-authenticated. Uploads a game build.

**Auth:** NIP-98 token; the signer MUST be the developer or a fulfillment key under a currently enabled, active authorization containing `upload_build`.

**Request body (`multipart/form-data`):**
```
listing_event: <JSON of signed kind 30402 event>
file: <binary game archive>
```

**Server MUST:**
1. Verify `listing_event.sig`.
2. Verify direct developer authority or a current listing authorization with active `upload_build` capability.
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
4. The server signs directly as developer, or the current listing references an active authorization for its fulfillment key containing `issue_receipt`.
5. Payment proof is valid: `zap_receipt_event.sig` valid, buyer `P` tag matches the NIP‑98 token pubkey, an `a`/`e` tag references the claimed `game_coordinate` with a matching amount, **and** `zap_receipt_event.pubkey` matches the LSP resolved from the listing's `lud16` tag per [LSP Verification](#lsp-verification-lud16) — or the `bolt11`/`preimage` pair validates per NIP‑102.
6. The payment proof event id has not been used in a prior confirmed purchase (replay protection).

**Server MUST on success:**

1. Generate an `order` id (UUID v4).
2. Construct a `kind:1020` root receipt including public payment binding and, for delegated signing, `authorization` pointing to the exact `kind:30406` root.
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
- **Path B — receipt query (portable):** query receipts by buyer and coordinate, resolve the receipt chain, and verify either direct developer signing or the immutable `authorization` root at the receipt root's `created_at`. Current listing membership is not required for historical credential validity.

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
8. Construct + sign coordinate-scoped kind:30406 root
   with explicit capabilities
9. Publish authorization root ────────────────────────────────────────────────────→
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
                               5. Fetch receipt authorization root
                                  and complete lifecycle ─────→
                                                                      ←──
                               6. Verify authorization at the
                                  receipt root's created_at
                               7. Verify server URL in listing's
                                  server tags (distribution auth)
                               8. Stream archive ───────────────────────→
9. Save to disk
```

## Trust Model

Ownership proof (the `kind:1020` receipt) and distribution authorization (`server` tags) are deliberately separate. A receipt is self-sovereign: any party can verify it against the listing without trusting the server that issued it. `server` tags are a distinct, developer-controlled allowlist of who may hold and serve copies of the file — a server MUST NOT infer distribution rights from a valid receipt alone, since that would let any node holding a leaked copy of the archive serve it to anyone with proof of purchase, regardless of whether the developer sanctioned that node as a mirror.

## Security Considerations

- **Replay attacks:** servers MUST store canonical identifiers for every supplied proof and reject reuse, even against a different buyer.
- **Receipt forgery:** a portable receipt is an authorized merchant attestation. Public proofs must validate, encrypted BOLT-11/preimage evidence must match the signed payment binding when selectively disclosed, and buyer assertions alone are insufficient.
- **Fulfillment key custody:** every fulfillment key MUST be distinct per developer — an operator serving multiple developers MUST NOT let two developers share a key, since compromising it would compromise every developer behind it at once. Keys must remain online to support non-interactive fulfillment; each key's capability is scoped to signing receipts for its one developer, so compromise doesn't expose that developer's listing-publishing or social identity, and is recoverable prospectively through `kind:30406` cancellation.
- **Multi-tenant key storage:** an operator holding many developers' fulfillment keys MUST encrypt them at rest (e.g. AES-256-GCM under a server-wide master key, at minimum) — this store is now a considerably higher-value target than any single self-hosted key, since its compromise cascades across every developer the operator serves. An operator running at meaningful scale SHOULD evaluate an HSM or KMS rather than relying on a bare master-key secret.
- **Prospective cancellation:** `kind:30406` cancellation is effective at the cancellation event timestamp and cannot invalidate earlier credentials.
- **Authorization completeness:** invalid chains and valid forks fail closed; insufficient relay coverage is indeterminate rather than proof of invalidity.
- **NIP‑98 token scope:** the `u` tag must exactly match the request URL including path; a token for `/purchase/confirm` MUST NOT be accepted for `/game/:id`.
- **File integrity:** `file_hash` is a developer commitment. Servers SHOULD periodically re-verify stored files against it.

## Relationship to Other NIPs

| NIP | Role |
|-----|------|
| [NIP-99](99.md) | Classified listing carrying distribution metadata and `fulfillment_authorization` root references |
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
- [ ] Verifying direct developer upload or current enabled authorization with `upload_build`.
- [ ] Verifying file SHA‑256 against `file_hash` on upload and periodically thereafter.
- [ ] Verifying NIP‑98, listing signature, and payment proof before issuing a receipt in `/purchase/confirm`.
- [ ] Resolving the listing's `lud16` tag (never the developer's profile) to an LSP `nostrPubkey` and confirming the zap receipt was signed by it, before accepting the zap-receipt payment path — see [LSP Verification](#lsp-verification-lud16).
- [ ] Enforcing idempotency on `order` id and replay protection on the payment-proof event id.
- [ ] Signing delegated receipts only under a current enabled authorization containing `issue_receipt`, and embedding its immutable root ID.
- [ ] Implementing `POST /provision` and `POST /provision/revoke` (§[Provisioning Endpoints](#provisioning-endpoints)), publishing a `kind:30404` attestation per provisioned key, with a distinct key per `(developer, scope)` pair — never a key shared across developers.
- [ ] Encrypting stored fulfillment private keys at rest.
- [ ] Resolving authorization chains with valid-prefix semantics and distinguishing invalidity from incomplete relay coverage.
- [ ] Evaluating ownership (Path A/B) and distribution authorization (`server` tags) as independent checks in `/game/:game_coordinate`.
- [ ] Rejecting NIP‑98 tokens whose `u` tag does not match the exact requested path.

## Open Questions

- **Authorization propagation latency:** no SLA is defined for observing a new listing enablement or `30406` cancellation.
- **Operator offboarding notification:** operator revocation should notify the developer so the developer can cancel `30406` and remove listing enablement.
- **Refunds:**** no refund flow is defined; left to individual server operators, informed by NIP‑102's `status: refunded` receipt chain.
