# ADP-01: Arcadestr Distribution Protocol

**Status:** Draft  
**Version:** 0.1.0  

---

## Abstract

The Arcadestr Distribution Protocol (ADP) defines a minimal, open HTTP protocol for
serving gated digital game files using Nostr identities as the authentication layer.
Any operator can run a compliant ADP server. Game developers choose which servers
carry their builds. Buyers prove ownership via NIP-58 badge events on relays — a
credential that is portable across servers and survives any single server going
offline.

ADP is intentionally modeled after [Blossom](https://github.com/hzrd149/blossom):
servers are interchangeable, the protocol is the contract, and no central operator
is privileged.

---

## Motivation

Digital game distribution requires a gatekeeper at download time — the binary cannot
be on a public URL. The naive solution (a single centralized server) reproduces
exactly the custody and censorship risks that Nostr exists to avoid.

ADP solves this by separating three concerns:

1. **Identity** — Nostr keypairs (existing).
2. **Payment proof** — NIP-57 zap receipts + NIP-58 badge awards on relays (existing
   primitives, new composition).
3. **File serving** — Any operator running an ADP-compliant server.

A buyer's purchase credential lives on Nostr relays as a NIP-58 badge. Any ADP
server can verify that credential. No single server holds the authoritative record.

---

## Terminology

| Term | Definition |
|---|---|
| **ADP server** | An HTTP server implementing this spec |
| **Developer** | The Nostr keypair that signed the game's NIP-99 listing event |
| **Buyer** | The Nostr keypair that completed payment |
| **Game coordinate** | `30402:{developer_pubkey_hex}:{d-tag}` — the stable identifier for a listing |
| **Server pubkey** | The Nostr keypair the ADP server uses to sign badge awards |
| **Trusted issuer** | A server pubkey listed in the game's `server` tags; its badges are valid proof |

---

## Nostr Event Extensions

### 1. NIP-99 Listing Tags (extended)

A developer adding ADP distribution to their NIP-99 listing (kind `30402`) includes
the following additional tags:

```json
["server", "https://dist.arcadestr.io", "<server_pubkey_hex>"]
["server", "https://games.otherhoster.com", "<other_server_pubkey_hex>"]
["file_hash", "<sha256_hex_of_game_archive>"]
["version", "1.0.0"]
["platform", "linux-x86_64"]
```

- Multiple `server` tags declare redundant distribution servers.
- The third value in each `server` tag is the server's Nostr pubkey. This is the
  **trusted issuer** for purchase badges for this listing.
- `file_hash` commits the listing to a specific build. Servers MUST verify the
  stored file matches this hash before serving.
- `platform` is optional but recommended for multi-platform releases. A listing per
  platform is the simplest model (separate `d-tag` per platform).

### 2. Server Announcement Event (kind `30403`)

ADP servers publish a parameterized replaceable event to announce themselves on
relays. Clients and developers use this to discover available servers.

```json
{
  "kind": 30403,
  "pubkey": "<server_pubkey_hex>",
  "tags": [
    ["d", "arcadestr-dist"],
    ["name", "Arcadestr Official Distribution"],
    ["url", "https://dist.arcadestr.io"],
    ["description", "Official arcadestr distribution server"],
    ["supported_adp", "0.1.0"],
    ["contact", "ops@arcadestr.io"]
  ],
  "content": "",
  "sig": "..."
}
```

Clients can query relays for `kind:30403` events to discover available ADP servers.

### 3. NIP-58 Badge Definition (kind `30009`) — issued by server

Each ADP server publishes one badge definition per game it distributes. The `d-tag`
encodes both the server's identity and the game coordinate, making it
globally unique and independently verifiable.

```json
{
  "kind": 30009,
  "pubkey": "<server_pubkey_hex>",
  "tags": [
    ["d", "adp:30402:<developer_pubkey_hex>:<game_d_tag>"],
    ["name", "Owner: <Game Title>"],
    ["description", "Verified purchase via ADP"]
  ],
  "content": "",
  "sig": "..."
}
```

Badge coordinate: `30009:<server_pubkey>:adp:30402:<developer_pubkey>:<game_d_tag>`

### 4. NIP-58 Badge Award (kind `8`) — issued by server on purchase

Published to relays after a verified purchase. This is the buyer's portable
proof-of-ownership.

```json
{
  "kind": 8,
  "pubkey": "<server_pubkey_hex>",
  "tags": [
    ["a", "30009:<server_pubkey>:adp:30402:<developer_pubkey>:<game_d_tag>"],
    ["p", "<buyer_pubkey_hex>"]
  ],
  "content": "",
  "sig": "..."
}
```

---

## HTTP Endpoints

All endpoints requiring authentication use `Authorization: Nostr <base64_nip98_event>`
per NIP-98.

---

### `GET /.well-known/adp`

Public. Returns server metadata. No auth required.

**Response `200`:**
```json
{
  "adp_version": "0.1.0",
  "pubkey": "<server_pubkey_hex>",
  "name": "Arcadestr Official Distribution",
  "url": "https://dist.arcadestr.io"
}
```

---

### `POST /upload`

Developer-authenticated. Upload a game build to the server.

**Auth:** NIP-98 token. The token's `pubkey` must match the `pubkey` field of the
NIP-99 listing event included in the request body.

**Request body (`multipart/form-data`):**
```
listing_event: <JSON of signed kind 30402 event>
file: <binary game archive>
```

**Server MUST:**
1. Verify `listing_event.sig`.
2. Verify `listing_event.pubkey` matches the NIP-98 token's `pubkey`.
3. Verify the uploaded file's SHA-256 matches the `file_hash` tag in `listing_event`.
4. Store the file indexed by `game_coordinate`.
5. Publish the `BadgeDefinition` (kind `30009`) if not already published.

**Response `200`:**
```json
{
  "game_coordinate": "30402:<developer_pubkey>:<d-tag>",
  "file_hash": "<sha256_hex>",
  "download_url": "https://dist.arcadestr.io/game/30402:<developer_pubkey>:<d-tag>"
}
```

---

### `POST /purchase/confirm`

Buyer-authenticated. Submit payment proof; receive download authorization and badge.

**Auth:** NIP-98 token. The buyer's identity is derived from the token's `pubkey`.
No buyer identity fields are accepted from the request body.

**Request body (`application/json`):**
```json
{
  "game_coordinate": "30402:<developer_pubkey>:<d-tag>",
  "listing_event": { /* signed kind 30402 event */ },
  "zap_receipt_event": { /* signed kind 9735 event */ }
}
```

**Server MUST verify:**

1. NIP-98 token: `kind = 27235`, correct `u` tag URL, correct `method` tag, timestamp
   within 60 seconds of server clock.
2. `listing_event.sig` is valid.
3. `listing_event.pubkey` is the expected developer for the claimed `game_coordinate`.
4. `zap_receipt_event.sig` is valid (signed by the merchant's LSP).
5. `zap_receipt_event` contains a `P` tag matching the buyer's pubkey (from NIP-98
   token, not request body).
6. `zap_receipt_event` contains an `a` or `e` tag referencing the claimed
   `game_coordinate`.
7. BOLT11 amount in `zap_receipt_event` matches the `price` tag in `listing_event`.
8. `zap_receipt_event_id` has not been used in a prior confirmed purchase (replay
   protection).

**Server MUST on success:**

1. Record purchase in `purchases` table: `(buyer_npub, game_coordinate,
   zap_receipt_event_id, awarded_at)`.
2. Publish `BadgeAward` (kind `8`) to relays.
3. Return the `BadgeAward` event and a short-lived download token in the response.

**Response `200`:**
```json
{
  "badge_award": { /* signed kind 8 event */ },
  "badge_definition": { /* signed kind 30009 event */ },
  "download_token": "<opaque_token>",
  "token_expires_at": 1234567890
}
```

**Error responses:**

| Status | Reason |
|---|---|
| `400` | Malformed request body |
| `401` | NIP-98 token invalid or expired |
| `402` | Payment verification failed (amount mismatch, wrong listing, etc.) |
| `409` | Zap receipt already used (replay) |
| `404` | Game not hosted on this server |

---

### `GET /game/:game_coordinate`

Buyer-authenticated. Download the game archive.

**Auth:** NIP-98 token OR `?token=<download_token>` query parameter (from
`/purchase/confirm` response).

**Server MUST verify one of:**

**Path A — download token:** Token is valid, not expired, and was issued for this
`game_coordinate` to this buyer's pubkey.

**Path B — badge check:** Query relays for a kind `8` badge award where:
- `pubkey` matches a trusted issuer (one of the `server` tags in the listing).
- `p` tag matches the buyer's pubkey (from NIP-98 token).
- `a` tag references the badge definition for this `game_coordinate`.

Path B is the **portable ownership** path — it works even if the buyer never
interacted with this specific server before. A buyer who purchased through
`dist.arcadestr.io` can download from `games.otherhoster.com` via their relay-stored
badge, as long as both servers share a trusted issuer agreement (see below).

**Response `200`:** Binary game archive stream.  
**Response `304`:** Not modified (ETag / conditional GET support RECOMMENDED).

---

## Trust Model for Portable Downloads

For Path B (badge check) to work across servers, the target server must consider
the issuing server a **trusted issuer** for that game. Trust is established via the
game listing's `server` tags: any server pubkey listed there is a trusted issuer.

This means:
- A buyer purchases through `dist.arcadestr.io` → badge issued by arcadestr server
  pubkey.
- Buyer tries to download from `games.otherhoster.com`.
- `games.otherhoster.com` fetches the NIP-99 listing, reads the `server` tags, sees
  arcadestr's pubkey listed, considers the badge valid.
- Download proceeds.

No out-of-band server-to-server communication is required. The listing event on
relays is the trust anchor.

---

## Purchase Flow (Full Sequence)

```
Buyer Client                    ADP Server                  Nostr Relays
─────────────────────────────────────────────────────────────────────────
1. Pay Lightning invoice
   (existing NIP-57 zap flow)

2. Poll for kind 9735
   zap receipt                                          ← kind 9735 appears

3. Construct NIP-98 token
   (kind 27235, signed by buyer)

4. POST /purchase/confirm ──────────────────────────→
   { game_coordinate,
     listing_event,
     zap_receipt_event }

                               5. Verify NIP-98 token
                               6. Verify listing sig
                               7. Verify zap receipt
                               8. Record purchase in DB
                               9. Publish kind 8 badge ──────────────────→

                               10. Return response ────────────────────→
                               { badge_award,
                                 badge_definition,
                                 download_token }

11. Construct EarnedBadgeSummary
    from response
12. Show BadgeEarnedModal

13. GET /game/:coordinate ──────────────────────────→
    ?token=<download_token>

                               14. Verify token ──────────────────────→
                               15. Stream game archive ────────────────→

16. Save to disk
```

---

## Download Flow — Portable (Different Server)

```
Buyer Client                    New ADP Server              Nostr Relays
─────────────────────────────────────────────────────────────────────────
1. Construct NIP-98 token

2. GET /game/:coordinate ──────────────────────────→
   Authorization: Nostr <token>

                               3. No download token,
                                  use badge check path
                               4. Fetch NIP-99 listing ──────────────→
                                  (to get trusted issuers)          ←──
                               5. Query for kind 8 badge ──────────→
                                  by buyer pubkey + coordinate      ←──
                               6. Badge found, issuer trusted
                               7. Stream game archive ─────────────────→

3. Save to disk
```

---

## Security Considerations

**Replay attacks:** The server MUST store `zap_receipt_event_id` in the `purchases`
table and reject any second submission of the same receipt ID, even for a different
buyer (a receipt can only confirm one purchase).

**Receipt forgery:** A zap receipt is signed by the merchant's LSP. The server
verifies this signature. An attacker cannot forge a receipt without the LSP's private
key. The server does not trust the buyer's assertion of payment — it verifies the
receipt independently.

**Badge trust scope:** Badges are only valid proof for servers that appear in the
corresponding game's `server` tags. A server MUST NOT accept badges issued by
pubkeys not listed in the game's listing event. This prevents a rogue server from
issuing badges for games it doesn't distribute.

**NIP-98 token scope:** The `u` tag must exactly match the request URL including
path. A token generated for `/purchase/confirm` cannot be reused for `/game/:id`.
Servers MUST enforce this.

**File integrity:** The `file_hash` tag in the NIP-99 listing is a commitment by
the developer. Servers SHOULD re-verify SHA-256 of stored files periodically and
alert if drift is detected.

---

## Open Questions

- **Multi-server badge issuance:** Should all listed servers issue a badge, or only
  the server that processed the purchase? Current spec: only the processing server
  issues the badge. Other servers accept it via the trust model.

- **Refunds:** No refund flow is defined in v0.1. This is intentional — refund
  logic is out of scope for the protocol and left to individual server operators.

- **Server discovery relay:** Should there be a recommended relay for kind `30403`
  server announcement events? Or is relay-agnostic discovery sufficient?

- **Version negotiation:** How should clients handle a server running an older ADP
  version? The `supported_adp` field in `/.well-known/adp` is a starting point but
  a full negotiation protocol is not yet defined.