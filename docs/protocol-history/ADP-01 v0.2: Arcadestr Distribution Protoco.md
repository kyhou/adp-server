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
| **Game coordinate** | `30402:<developer-pubkey>:<d-tag>` — the stable identifier for a listing |
| **Buyer** | The Nostr keypair that completed payment |

## Listing Extensions (`kind:30402`)

A developer distributing a game via ADP adds the following tags to their [NIP‑99](99.md) classified listing:

| Tag | Value | Description |
|-----|-------|--------------|
| `server` | `<https-url>` | Declares a server authorized to distribute this listing's file. Repeatable. |
| `file_hash` | `<sha256-hex>` | SHA‑256 of the game archive. Servers MUST verify stored files against this hash before serving. |
| `version` | `<semver-string>` | Build version. |
| `platform` | `<os>-<arch>` | Optional; a listing per platform is the simplest model. |
| `fulfillment_pubkey` | `<pubkey-hex>`, `<valid_from>`, `<revoked_at>` | Delegates receipt‑signing authority for this listing to `<pubkey-hex>`, valid from the given unix timestamp until `<revoked_at>` (empty if still active). Repeatable — see [Fulfillment Key Delegation](#fulfillment-key-delegation). |

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
    ["fulfillment_pubkey", "c3d4...", "1735000000", ""]
  ],
  "content": "...",
  "sig": "<developer-signature>"
}
```

`server` tags declare **distribution authorization only** — which servers may serve the file. They are independent of ownership proof; see [Distribution Authorization](#distribution-authorization) below.

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

### Updated Proof of Ownership

This extends [NIP‑102](NIP-102.md)'s Proof-of-Ownership condition ("signed by a pubkey the buyer trusts as the merchant") to:

```
signature(E_1020) is valid
  AND (
        E_1020.pubkey == listing.pubkey
        OR
        E_1020.pubkey is authorized per a fulfillment_pubkey tag's
        validity interval, evaluated at E_1020.created_at
      )
```

Verifiers MUST fetch the referenced listing fresh from relays — not from a local cache — before relying on delegation to authorize a security‑relevant action such as unlocking a download. A cached listing may not yet reflect a revocation.

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
5. Payment proof is valid: `zap_receipt_event.sig` valid, buyer `P` tag matches the NIP‑98 token pubkey, and an `a`/`e` tag references the claimed `game_coordinate` with a matching amount — or the `bolt11`/`preimage` pair validates per NIP‑102.
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
| `402` | Payment verification failed |
| `409` | Payment proof already used (replay) |
| `404` | Game not hosted on this server |
| `500` | Server's fulfillment key is not currently authorized for this listing |

### `GET /game/:game_coordinate`

Buyer-authenticated. Streams the game archive. Requires both an ownership check and a distribution-authorization check to pass independently.

**Auth:** NIP‑98 token, or `?token=<download_token>` from `/purchase/confirm`.

#### Ownership check

- **Path A — download token:** valid, unexpired, issued for this `game_coordinate` to this buyer.
- **Path B — receipt query (portable):** query relays for `kind:1020` events with `#p` = buyer pubkey and `#a` = `game_coordinate`. For the latest status in the order chain, verify the signer's authorization per [Fulfillment Key Delegation](#fulfillment-key-delegation), fetching the listing fresh.

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
                               5. Fetch listing fresh (not cached) ────→
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
- **Fulfillment key custody:** the fulfillment key SHOULD be a dedicated key distinct from the developer's identity key, since it must remain online to support non-interactive fulfillment. Its capability is scoped to signing receipts; compromise does not expose the developer's listing-publishing or social identity, and is recoverable via `revoked_at`.
- **Delegation freshness:** verifiers MUST fetch listings fresh, not from cache, before authorizing downloads based on delegation — see [Fulfillment Key Delegation](#fulfillment-key-delegation).
- **NIP‑98 token scope:** the `u` tag must exactly match the request URL including path; a token for `/purchase/confirm` MUST NOT be accepted for `/game/:id`.
- **File integrity:** `file_hash` is a developer commitment. Servers SHOULD periodically re-verify stored files against it.

## Relationship to Other NIPs

| NIP | Role |
|-----|------|
| [NIP‑99](99.md) | Classified listing (`kind:30402`) carrying `server`, `file_hash`, `version`, `platform`, and `fulfillment_pubkey` tags |
| [NIP‑102](NIP-102.md) | Marketplace receipt (`kind:1020`) — the ownership credential this protocol relies on |
| [NIP‑98](98.md) | HTTP Auth for all authenticated endpoints |
| [NIP‑44](44.md) | Encryption of receipt `.content` |
| [NIP‑57](57.md) | Zap Request/Receipt as payment proof |

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
- [ ] Enforcing idempotency on `order` id and replay protection on the payment-proof event id.
- [ ] Signing receipts with a key currently authorized per the listing's `fulfillment_pubkey` validity interval.
- [ ] Fetching listings fresh (not cached) before authorizing downloads via delegation.
- [ ] Evaluating ownership (Path A/B) and distribution authorization (`server` tags) as independent checks in `/game/:game_coordinate`.
- [ ] Rejecting NIP‑98 tokens whose `u` tag does not match the exact requested path.

## Open Questions

- **Multi-tenant hosted fulfillment:** the developer-facing flow for provisioning a fulfillment key on a shared hosting service is out of scope for this draft.
- **Revocation propagation latency:** no SLA is defined for how quickly relays/servers are expected to observe a `revoked_at` update.
- **Refunds:** no refund flow is defined; left to individual server operators, informed by NIP‑102's `status: refunded` receipt chain.