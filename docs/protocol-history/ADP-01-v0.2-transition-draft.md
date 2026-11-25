# ADP-01 v0.2: Receipt-Based Distribution with Delegated Fulfillment Keys

**Status:** Draft
**Supersedes:** ADP-01 v0.1 badge-based purchase flow
**Depends on:** NIP-102 (extended below), NIP-99, NIP-98, NIP-44

---

## Summary of changes from v0.1

| | v0.1 | v0.2 |
|---|---|---|
| Ownership credential | `kind:8` NIP-58 badge award, issued by the processing server | `kind:1020` NIP-102 receipt, signed by a seller-delegated fulfillment key |
| Signing authority | Server's own pubkey, trusted via listing `server` tags | A key the seller explicitly delegates, scoped and revocable |
| Cross-server portability | Badge issuer must appear in listing's `server` tags | Any server can verify the receipt directly against the listing's delegation tag |
| Distribution authorization | Conflated with ownership (only listed servers could badge) | Split out: `server` tags now mean "authorized to serve this file_hash," nothing more |

The driving idea: the seller runs (or delegates to a hosted instance of) ADP server software, using a key of their choosing for game fulfillment. That server completes the NIP-102 flow on the seller's behalf, producing a single canonical, self-sovereign ownership artifact — the `kind:1020` receipt — instead of a separate badge system layered on top.

---

## Part 1 — NIP-102 Extension: Fulfillment Key Delegation

NIP-102 §"Proof of Ownership" requires the receipt to be signed by "a pubkey the buyer trusts as the merchant." This extension formalizes a way for that trusted pubkey to be a **delegate**, distinct from the seller's main identity key, declared on the listing itself and independently rotatable/revocable.

### 1.1 Delegation tag

Added to the `kind:30402` listing event (NIP-99), one tag per active or historical fulfillment key:

```json
["fulfillment_pubkey", "<hot_key_hex>", "<valid_from_unix>", "<revoked_at_unix>"]
```

- **`hot_key_hex`** — the delegated key authorized to sign `kind:1020` receipts for this listing.
- **`valid_from_unix`** — unix timestamp; the key is authorized starting at this instant. Required.
- **`revoked_at_unix`** — unix timestamp; the key's authorization ends at this instant, exclusive. **Optional** — omit or use empty string `""` while the key is still active.

A listing MAY carry multiple `fulfillment_pubkey` tags to represent rotation history. Each tag is evaluated independently; there is no ordering dependency between tags.

```json
{
  "kind": 30402,
  "pubkey": "<seller_identity_pubkey>",
  "tags": [
    ["d", "my-game-v1"],
    ["fulfillment_pubkey", "a1b2...", "1700000000", "1735000000"],
    ["fulfillment_pubkey", "c3d4...", "1735000000", ""],
    ["server", "https://dist.arcadestr.io"],
    ["file_hash", "..."],
    ["version", "1.0.0"]
  ],
  "sig": "<seller_identity_signature>"
}
```

This example shows a rotation: key `a1b2...` was authorized from `1700000000` until it was retired (not necessarily compromised — could be routine rotation) at `1735000000`, at which point `c3d4...` took over and remains active.

To **revoke without rotating**, the seller republishes the listing with `revoked_at` set on the current key and no replacement tag added yet. This is the operationally urgent case (suspected compromise) and does not require having a new key ready.

### 1.2 Validity interval semantics

A fulfillment key `K` is considered authorized to sign a receipt with `created_at = T` if and only if there exists a `fulfillment_pubkey` tag for `K` such that:

```
valid_from <= T   AND   (revoked_at is absent OR revoked_at > T)
```

This is a closed-open interval `[valid_from, revoked_at)`. Because the check is against the receipt's own `created_at`, a compromised key's *historical* receipts (signed before revocation) remain valid — which is correct: those purchases genuinely happened. Only receipts claiming to be signed *after* `revoked_at` are rejected, which also correctly blocks a compromised key from being used to backdate forged receipts, since `created_at` in the past for a not-yet-existing tag would fail the `valid_from` bound at verification time regardless.

### 1.3 Updated Proof-of-Ownership check

NIP-102's original PoO condition #1 ("signed by a pubkey the buyer trusts as the merchant") becomes:

```
S(E_1020) is valid
  AND (
        E_1020.pubkey == listing.pubkey                       // direct signing, unchanged from v1
        OR
        ∃ tag ["fulfillment_pubkey", K, valid_from, revoked_at] ∈ listing.tags
          such that E_1020.pubkey == K
          AND valid_from <= E_1020.created_at
          AND (revoked_at is absent OR revoked_at > E_1020.created_at)
      )
```

Direct signing by the seller's identity key remains valid (a seller running fulfillment themselves, no delegation needed). Delegation is additive, not a replacement requirement.

### 1.4 Freshness requirement

Verifiers **MUST** fetch the current listing event from relays (not a locally cached copy) before evaluating delegation for any security-relevant action — specifically, unlocking a download or displaying an ownership-gated UI state. A stale cached listing may not yet reflect a revocation, silently reinstating a compromised key's authority. This applies to both Arcadestr's own `MarketplaceCache` and any ADP server's listing lookups.

### 1.5 Rust verification signature (`core/src/purchases.rs`)

```rust
/// Result of checking whether a receipt's signer was authorized to sign it,
/// either directly (seller's own key) or via delegation at the time of signing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignerAuthorization {
    DirectSeller,
    Delegated { fulfillment_pubkey: PublicKey, valid_from: u64 },
}

#[derive(Debug, Error)]
pub enum DelegationError {
    #[error("receipt signer does not match seller key or any fulfillment delegation")]
    UnauthorizedSigner,
    #[error("fulfillment key was revoked at {revoked_at}, receipt created_at {created_at} is after revocation")]
    RevokedAtSigningTime { revoked_at: u64, created_at: u64 },
    #[error("listing lookup failed: {0}")]
    ListingFetchError(String),
}

/// Verifies that `receipt.pubkey` was authorized to sign `receipt` at `receipt.created_at`,
/// either as the seller's own identity key or via an active `fulfillment_pubkey` delegation
/// on the (freshly fetched, not cached) listing referenced by the receipt's `a` tag.
///
/// MUST be called with a listing fetched fresh from relays for any security-relevant
/// action (install/download unlock). Callers displaying non-security UI (e.g. purchase
/// history) MAY use a cached listing with the understanding that revocation may lag.
pub async fn verify_signer_authorization(
    receipt: &Event,
    listing: &Nip99Listing,
) -> Result<SignerAuthorization, DelegationError> {
    if receipt.pubkey == listing.pubkey {
        return Ok(SignerAuthorization::DirectSeller);
    }

    for tag in listing.fulfillment_pubkey_tags() {
        if tag.pubkey != receipt.pubkey {
            continue;
        }
        if tag.valid_from > receipt.created_at {
            continue;
        }
        if let Some(revoked_at) = tag.revoked_at {
            if revoked_at <= receipt.created_at {
                return Err(DelegationError::RevokedAtSigningTime {
                    revoked_at,
                    created_at: receipt.created_at,
                });
            }
        }
        return Ok(SignerAuthorization::Delegated {
            fulfillment_pubkey: tag.pubkey,
            valid_from: tag.valid_from,
        });
    }

    Err(DelegationError::UnauthorizedSigner)
}
```

Integration point: this replaces the flat `receipt.pubkey == merchant_pubkey_from_a_tag` check currently in `parse_and_validate_receipt`. It should be called with a listing obtained via a forced-fresh fetch path, not `MarketplaceCache::load_listings`, whenever the result gates `install_game`.

---

## Part 2 — Updated ADP Server Flow (Receipt-Based)

### 2.1 Key custody

The ADP server instance (self-hosted by the seller, or a hosted multi-tenant service) holds the **fulfillment private key**, not the seller's identity key. This is a deliberate, narrower custody boundary:

- The fulfillment key's only capability is signing `kind:1020` receipts (and NIP-44 encrypting their `.content` to buyers). It cannot publish listings, cannot impersonate the seller's social identity, and its blast radius on compromise is bounded to "can mint fraudulent purchase receipts for this seller's catalog" — serious, but recoverable via `revoked_at`, and does not touch the seller's main key at all.
- Sellers generate the fulfillment key themselves at server setup time and declare it via the `fulfillment_pubkey` tag (Part 1.1) on each listing they want that server instance to fulfill. A hosted multi-tenant service issues one fulfillment key per seller (or per seller's isolated tenant record) — never a key shared across unrelated sellers.
- The key must remain hot/live on the server process to support fully automated, non-interactive fulfillment (no bunker approval round-trip at purchase time). This is the explicit tradeoff that makes instant delivery possible; it is why the key is scoped down rather than reused, and why revocation is a first-class operation rather than an afterthought.

### 2.2 `POST /purchase/confirm` (revised)

**Auth:** NIP-98 token, buyer-signed, unchanged from v0.1.

**Request body:** unchanged — `game_coordinate`, `listing_event`, `zap_receipt_event` (or `bolt11`/`preimage` pair, per NIP-102's payment-proof flexibility).

**Server verification steps (1–8):** unchanged from v0.1 — NIP-98 token validity, listing signature, listing pubkey matches claimed coordinate's developer, zap receipt validity and buyer/coordinate match, amount match, replay check on the payment proof event id.

**Server MUST on success (revised):**

1. Generate an `order` id (UUID v4) for this purchase.
2. Construct the `kind:1020` receipt:
   - `order` tag: the generated order id.
   - `p` tag: buyer's pubkey (from NIP-98 token).
   - `a` tag: the `game_coordinate`.
   - `e` tag → the zap receipt event id, **or** `bolt11` + `preimage` tags if that path was used.
   - `status`: `paid`.
   - `amount` / `currency`: from the listing's price.
3. Encrypt the itemized order JSON (per NIP-102 content schema) with NIP-44 to the buyer's pubkey, using the fulfillment key's conversation key.
4. Sign the event with the **fulfillment key**, not a badge-issuer key.
5. Publish the `kind:1020` event to relays.
6. Record `(order_id, zap_receipt_event_id, buyer_pubkey, game_coordinate, awarded_at)` in the server's local `purchases` table, with a **unique constraint on `order_id`** to guarantee the idempotency NIP-102 requires — this is now enforced by adp-server itself, since it is the issuing party.
7. Return the signed receipt and a short-lived download token.

**Response `200`:**
```json
{
  "receipt": { /* signed kind 1020 event */ },
  "download_token": "<opaque_token>",
  "token_expires_at": 1234567890
}
```

`badge_award` and `badge_definition` are removed from this response; NIP-58 badges are no longer part of the purchase-confirmation flow. (Badges remain available as a *separate*, optional feature — e.g. NIP-58 achievement badges for playtime or milestones — but are decoupled from purchase proof entirely.)

**Error responses:** unchanged from v0.1 (`400`/`401`/`402`/`409`/`404`), plus:

| Status | Reason |
|---|---|
| `500` | Server's fulfillment key is not currently authorized per the listing's `fulfillment_pubkey` tags (misconfiguration — the seller revoked or never delegated to this server's key) |

### 2.3 `GET /game/:game_coordinate` (revised)

Two checks now run independently rather than one combined check:

**Check A — Ownership (is this buyer entitled to this game at all):**

- **Path A1 — download token:** unchanged from v0.1, fast path immediately after purchase.
- **Path A2 — receipt query (portable, cross-server):** query relays for `kind:1020` events with `#p` = buyer pubkey and `#a` = `game_coordinate`. For each candidate, fetch the listing fresh (per Part 1.4) and run `verify_signer_authorization` (Part 1.5). Take the latest valid `paid`/`fulfilled` status per the order chain (not simple `created_at` — traverse `e`-tag references per NIP-102's status lifecycle, consistent with what `core::purchases::is_owned()` already does).

This is the portability win: a buyer who purchased through `dist.arcadestr.io` can download from `games.otherhoster.com` purely by presenting a receipt whose signer resolves against the listing's delegation tags — no relationship between the two servers is required, and no badge-issuer allowlist needs to be checked.

**Check B — Distribution authorization (is *this server* allowed to serve *this file*):**

- Independent of Check A. The requesting server verifies its own pubkey (or, for non-fulfillment file-serving nodes, its operator identity) appears in the listing's `server` tags for this `game_coordinate`.
- This check exists so that ownership proof (self-sovereign, checkable by anyone) does not accidentally imply "any server that sees a valid receipt may serve the file to anyone." A rogue mirror holding a copy of the binary but not named in `server` tags must not serve it, even to a buyer with a perfectly valid receipt.

Both checks MUST pass. Fold their failure modes into distinct error responses so operators can distinguish "buyer doesn't own this" from "this server isn't authorized to distribute this build":

| Status | Reason |
|---|---|
| `403` | Ownership check failed (no valid receipt found or signer unauthorized) |
| `451` | Distribution authorization failed (server not listed in `server` tags for this coordinate) |

### 2.4 Removed from v0.1

- Badge Definition (`kind:30009`) and Badge Award (`kind:8`) publishing as part of the purchase flow.
- The "trusted issuer" model tying badge validity to `server` tag membership — replaced by direct delegation verification against the listing.

Badges are not deprecated as a Nostr primitive in Arcadestr generally (e.g., achievement/NIP-58 badge infrastructure on your roadmap for playtime, milestones, etc.) — they are simply no longer the ownership credential.

---

## Part 3 — Updated Sequence Diagram

```
Seller                Buyer Client            ADP Server (fulfillment key)     Nostr Relays
──────────────────────────────────────────────────────────────────────────────────────────
1. Publishes kind:30402
   listing with
   fulfillment_pubkey
   tag(s) ─────────────────────────────────────────────────────────────────→
                                                                          (listing stored)

                        2. Pay Lightning
                           invoice
                        3. Poll for kind:9735
                           zap receipt ────────────────────────────────────→ (poll)
                        4. Construct NIP-98
                           token
                        5. POST /purchase/confirm ──→
                           { game_coordinate,
                             listing_event,
                             zap_receipt_event }
                                                    6. Verify NIP-98, listing
                                                       sig, zap receipt
                                                    7. Build kind:1020 receipt
                                                    8. NIP-44 encrypt content
                                                       to buyer pubkey
                                                    9. Sign with fulfillment key
                                                    10. Publish ─────────────────────────→
                                                    11. Return receipt +
                        ←── download_token ─────────── download_token

                        12. GET /game/:coordinate ──→
                            ?token=<download_token>
                                                    13. Verify token (Path A1)
                                                    14. Verify server tags (Check B)
                                                    15. Stream archive ──────────────────→
                        16. Save to disk


                        ── later, different server, no prior relationship ──

                        17. GET /game/:coordinate ──→   (games.otherhoster.com)
                            Authorization: Nostr <token>
                                                    18. No download token, use
                                                        Path A2 (receipt query)
                                                    19. Fetch kind:1020 receipts
                                                        by #p + #a ──────────────────────→
                                                                                       ←──
                                                    20. Fetch listing FRESH ─────────────→
                                                        (not cached)                   ←──
                                                    21. verify_signer_authorization()
                                                        against fulfillment_pubkey tags
                                                    22. Check B: is otherhoster.com
                                                        listed in server tags?
                                                    23. Stream archive ──────────────────→
```

---

## Open questions carried forward

- **Multi-tenant hosted service key issuance UX.** If Arcadestr offers a hosted ADP service, the seller-facing flow for "generate my fulfillment key and get the `fulfillment_pubkey` tag to add to my listings" needs a concrete UI/CLI path — not specified here.
- **Revocation propagation latency.** Part 1.4's freshness requirement mitigates but doesn't eliminate the window between a seller publishing a revocation and all relays/servers observing it. No SLA is defined.
- **`file_hash` versioning vs. fulfillment key versioning.** These are currently independent tag sets on the same listing (`version`/`file_hash` vs. `fulfillment_pubkey`). Worth confirming that key rotation is never implicitly tied to build version bumps, since they represent unrelated concerns.