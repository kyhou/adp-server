# ADP-01 Amendment: Free, Promotional, and Non-Monetary Acquisitions

`draft` `proposed amendment to ADP-01` `v3` `consumes Entitlement Grant Draft`

## Motivation

NIP-102 requires payment proof for every marketplace receipt and remains unchanged. ADP therefore needs a separate credential for permanent entitlement issued without payment.

This amendment distinguishes four cases:

1. **Paid acquisition** — existing NIP-102 receipt flow.
2. **Explicit public access** — anyone may download while current listing policy says public.
3. **Timed access** — anyone may download during a publisher-defined interval, with no durable entitlement.
4. **Claim-and-keep campaign** — an authenticated buyer claims during an active publisher-signed campaign and receives a durable Entitlement Grant.

Commercial policy is controlled only by the developer/publisher identity key. Fulfillment keys execute authorized operational actions but cannot create, modify, cancel, or revoke campaigns and cannot revoke buyer entitlements.

## Event-kind allocation

This amendment requires two unallocated regular-event kinds:

- `<entitlement-grant-kind>` — defined by the Entitlement Grant Draft;
- `<adp-campaign-kind>` — defined by this amendment.

Neither symbolic placeholder may be replaced with an already assigned kind. Implementations MUST NOT deploy experimental values as interoperable production allocations.

The use of two event kinds is intentional. Campaign policy and buyer-specific entitlement are different protocol objects: campaigns are one-to-many publisher policy with publisher-only control and coordinate-based discovery; grants are one-to-one buyer credentials with delegated issuance and recipient-based discovery. Combining them would blur signer authority, lifecycle, revocation, indexing, and verification semantics.

## Authority model

| Action | Developer key | Authorized fulfillment key |
|---|---:|---:|
| Publish or update listing | Yes | No |
| Create campaign | Yes | No |
| Update future campaign terms | Yes | No |
| Cancel campaign | Yes | No |
| Delegate or revoke fulfillment key | Yes | No |
| Issue NIP-102 paid receipt | Yes | Yes |
| Issue Entitlement Grant under valid campaign | Yes | Yes |
| Revoke buyer entitlement | Yes | No |
| Issue download token | Yes | Yes |
| Upload build | Yes | Yes, when delegated |
| Serve authorized file | Yes | Yes, when delegated |

The developer defines policy. Fulfillment infrastructure executes policy.

## Listing extensions

### Current access policy

The `kind:30402` listing gains an optional `acquisition` tag only for current ungated access policy:

| Tag | Value | Meaning |
|---|---|---|
| `acquisition` | `public` | Anyone may download while this is the current listing policy. No durable credential is issued. |
| `acquisition` | `timed-access`, `<starts_at>`, `<ends_at>` | Anyone may download while `starts_at <= now < ends_at`. No durable credential is issued. |

Claim-and-keep campaigns are not defined by replaceable listing state. They use separate publisher-signed campaign events.

A zero or missing price MUST NOT implicitly create public ADP access. Public access requires an explicit `acquisition: public` tag. A client may still expose a direct-download URL outside ADP for a free listing.

A listing with no `acquisition` tag follows the existing gated behavior.

### Optional campaign pointers

A publisher MAY add one or more repeatable campaign-pointer tags to the current listing:

| Tag | Value | Meaning |
|---|---|---|
| `campaign` | `<campaign-root-event-id>`, `<relay-hint>` | Advisory pointer to the immutable root event of an active or upcoming campaign for this game. The relay hint is optional. |

Example:

```jsonc
["campaign", "<campaign-root-event-id>", "wss://relay.example.com"]
```

Publishing clients SHOULD add these pointers when creating or associating active or upcoming campaigns with a listing. They SHOULD remove pointers to cancelled or expired campaigns when the listing is next republished, but immediate removal is not required.

Campaign pointers are discovery hints only:

- their presence MUST NOT prove that a campaign is valid or active;
- their absence MUST NOT prove that no campaign exists;
- clients and servers MUST independently fetch and validate the referenced campaign chain;
- stale pointers are harmless when campaign-chain verification is performed;
- security MUST NOT depend on promptly republishing the listing after cancellation or expiration.

The campaign chain remains the authoritative source of campaign terms and status.

## Publisher-signed campaign event

A claim-and-keep promotion is represented by a regular, non-replaceable event of `<adp-campaign-kind>`.

Only the developer key identified by the campaign's game coordinate may sign a valid campaign event or campaign status update. Fulfillment keys MUST NOT create, edit, or cancel campaigns.

### Campaign tags

| Tag | Value | Description |
|---|---|---|
| `d` | `<campaign-id>` | Stable campaign identifier. Required and relay-indexable under NIP-01. SHOULD be a UUID v4 or publisher-unique opaque string. |
| `a` | `30402:<developer-pubkey>:<d-tag>` | Game coordinate. Required. |
| `mode` | `claim` | Campaign grants permanent entitlement to authenticated claimants. Required on root and pre-start updates. |
| `starts` | `<unix-seconds>` | Inclusive campaign start. Required on root and pre-start updates. |
| `ends` | `<unix-seconds>` | Exclusive campaign end. Required on root and pre-start updates. MUST be greater than `starts`. |
| `status` | `active` \| `cancelled` | Campaign state. Root MUST use `active`. |
| `e` | `<preceding-campaign-event-id>` | Required exactly once on updates and cancellation; absent on root. |

The `d` tag is used as the campaign identifier because NIP-01 guarantees filter syntax only for single-letter indexed tags. Campaign discovery and chain lookup therefore use `#a` and `#d`, not non-standard `#campaign` filters.

### Root example

```jsonc
{
  "kind": "<adp-campaign-kind>",
  "pubkey": "<developer-pubkey>",
  "created_at": 1759900000,
  "tags": [
    ["d", "launch-week-2026"],
    ["a", "30402:<developer-pubkey>:my-game-v1"],
    ["mode", "claim"],
    ["starts", "1760000000"],
    ["ends", "1760600000"],
    ["status", "active"]
  ],
  "content": "",
  "sig": "<developer-signature>"
}
```

## Campaign discovery

Clients SHOULD use listing `campaign` pointers as a fast discovery path when available. They MUST still support coordinate-based relay discovery as the complete fallback.

To discover all campaign events associated with a game coordinate:

```jsonc
["REQ", "game-campaigns", {
  "kinds": ["<adp-campaign-kind>"],
  "#a": ["30402:<developer-pubkey>:<d-tag>"]
}]
```

Clients MUST group returned events by the indexed `d` tag, require the event author to equal the developer pubkey encoded in `a`, and resolve each campaign chain independently.

To fetch a specific campaign chain after discovering its root or campaign identifier:

```jsonc
["REQ", "campaign-chain", {
  "kinds": ["<adp-campaign-kind>"],
  "authors": ["<developer-pubkey>"],
  "#d": ["<campaign-id>"]
}]
```

When a client already has the immutable root event ID, it MAY fetch the root directly:

```jsonc
["REQ", "campaign-root", {
  "ids": ["<campaign-root-event-id>"]
}]
```

Every campaign-chain event MUST carry exactly one `d` tag and exactly one `a` tag, preserving the root values. This supports reliable indexed lookup and prevents chain events from changing campaign identity or game association.

Discovery precedence is:

1. listing campaign pointers as a preferred optimization;
2. coordinate-based `#a` queries as the authoritative fallback;
3. campaign validity always determined from the publisher-signed campaign chain.

## Campaign lifecycle

### Before `starts`

The developer may publish an update that changes campaign terms. The update MUST:

- use the same `d` and `a` values;
- contain exactly one `e` tag referencing the immediately preceding campaign event;
- be signed by the developer key from the `a` coordinate;
- be created before the currently effective `starts` value;
- provide the complete replacement `mode`, `starts`, `ends`, and `status` fields.

### At or after `starts`

Campaign terms are immutable. The developer may only publish `status: cancelled`.

A post-start event that changes `mode`, `starts`, or `ends` is invalid.

### Cancellation

A valid cancellation:

- is signed by the developer key;
- preserves `d` and `a`;
- references the immediately preceding campaign event;
- uses `status: cancelled`;
- prevents grants whose root `created_at` is equal to or later than the cancellation event's `created_at`.

Cancellation does not revoke grants issued before cancellation. Individual entitlement revocation requires a separate developer-signed grant status update.

### Chain resolution

Campaign verification uses the same conservative rules as grant-chain verification:

- every signature must be valid;
- invariant `d` and `a` values must remain unchanged;
- updates must have exactly one predecessor;
- cycles, self-references, and forks are invalid;
- verifiers MUST NOT resolve forks by `created_at`.

The effective campaign state at time `T` is determined by traversing the valid chain in order and applying only events with `created_at <= T`.

A campaign is claimable at time `T` when:

```text
status_at(T) == active
AND starts <= T
AND T < ends
```

## ADP extension of Entitlement Grants

### Required tags

A grant accepted by ADP MUST contain exactly one:

- `d` (grant identifier);
- `p`;
- `a`;
- `source_event`;
- `status`.

For promotional claims, `source_event` MUST reference the campaign root event ID, not a replaceable listing event.

### Issuance authority

The root grant MAY be signed by:

- the developer key from the game coordinate; or
- a fulfillment key authorized by the current listing's `fulfillment_pubkey` delegation at the grant's `created_at`, using ADP-01's existing `effective_revoked_at` rule.

Fulfillment delegation authorizes issuance only. It does not authorize campaign control or entitlement revocation.

### Campaign anchoring

A verifier MUST fetch the campaign chain rooted at `source_event` and verify:

1. The root event exists, has a valid signature, and uses `<adp-campaign-kind>`.
2. The root signer equals the developer pubkey encoded in the grant's `a` coordinate.
3. The campaign `a` equals the grant `a`.
4. The campaign chain is valid and unambiguous.
5. The campaign was active at the root grant's `created_at`.
6. `starts <= grant.created_at < ends`.
7. No valid cancellation had become effective by `grant.created_at`.
8. The root grant signer was authorized to issue at `grant.created_at`.

A later campaign cancellation does not invalidate an earlier grant.

### Grant revocation authority

Under ADP, only the developer key encoded in the grant's `a` coordinate may publish a valid `status: revoked` update.

A fulfillment key MUST NOT revoke a grant, including a grant it originally issued.

Every revocation event MUST satisfy the Entitlement Grant chain invariants and preserve the root's `d`, `p`, `a`, and `source_event` values.

## Claim endpoint

### `POST /entitlement/claim`

Buyer-authenticated. Claims an active publisher-signed campaign and receives an Entitlement Grant plus a short-lived download token.

**Auth:** NIP-98. Buyer identity comes only from the token pubkey.

**Request body:**

```jsonc
{
  "game_coordinate": "30402:<developer-pubkey>:<d-tag>",
  "campaign_event_id": "<campaign-root-event-id>"
}
```

### Server MUST verify

1. NIP-98 token kind, signature, `u`, method, and 60-second clock bound.
2. The current listing for `game_coordinate` from at least two independent relays, selecting the highest valid `created_at` result.
3. The server is currently authorized in the listing's `server` tags and hosts the requested game.
4. The campaign root and complete campaign chain are retrievable and valid.
5. The campaign developer equals the listing developer and coordinate developer.
6. The campaign is active at current server time using `starts <= now < ends`.
7. No cancellation is effective at current server time.
8. The server's signing key is authorized under the current listing delegation at current server time, or the server holds the developer key.
9. Local idempotency for `(buyer_pubkey, game_coordinate, campaign_id)`.

A campaign does not need to be referenced by the current replaceable listing. Listing pointers are optional discovery hints only. The server MUST resolve the supplied root event and independently fetch and validate the complete publisher-signed campaign chain.

### Idempotency

The server MUST persist at least:

```text
buyer_pubkey
game_coordinate
campaign_id
campaign_root_event_id
grant_event_id
```

with a local uniqueness constraint equivalent to:

```sql
UNIQUE (buyer_pubkey, game_coordinate, campaign_id)
```

If a matching local grant exists, the server MUST return it instead of issuing another.

Independent authorized servers may concurrently issue redundant grants for the same claim. Such grants remain individually valid. Global uniqueness is not guaranteed.

An implementation MAY calculate an internal idempotency key:

```text
SHA256(buyer_pubkey || game_coordinate || campaign_id)
```

This value is bookkeeping only and is not proof of entitlement.

### Success behavior

The server MUST:

1. Generate a unique grant ID.
2. Construct the root Entitlement Grant with:
   - recipient `p`;
   - game coordinate `a`;
   - campaign root ID in `source_event`;
   - `reason: promotional-claim`;
   - `status: granted`.
3. Sign with the developer key or an authorized fulfillment key.
4. Publish the grant to relays.
5. Persist the idempotency record.
6. Issue a short-lived download token using the existing ADP token mechanism.

**Response `200`:**

```jsonc
{
  "grant": { "...": "signed entitlement grant event" },
  "download_token": "<opaque-token>",
  "token_expires_at": 1234567890
}
```

### Errors

| Status | Reason |
|---|---|
| `400` | Malformed request, invalid campaign chain, or coordinate mismatch. |
| `401` | Invalid or expired NIP-98 token. |
| `403` | Campaign not started, ended, or cancelled. |
| `404` | Campaign, listing, or hosted game not found. |
| `409` | Conflicting local issuance state that cannot be resolved idempotently. |
| `451` | Server is not currently authorized to distribute the game. |
| `500` | No server-held key is currently authorized to issue the grant. |

## Extended download authorization

### `GET /game/:game_coordinate`

The server first fetches the current listing for current distribution and file policy.

It MUST verify current:

- listing signature and coordinate;
- server distribution authorization;
- file hash and hosted file integrity;
- file/version metadata used for the response.

Ownership or access is then evaluated in this order:

```text
1. Valid unexpired download token scoped to buyer and coordinate -> allow.

2. Valid Entitlement Grant for buyer and coordinate -> allow,
   regardless of current listing price, acquisition tag, or campaign state.

3. Valid NIP-102 receipt for buyer and coordinate -> allow,
   regardless of current listing price or acquisition tag.

4. Current listing has acquisition: public -> allow.

5. Current listing has acquisition: timed-access and
   starts <= now < ends -> allow.

6. Otherwise -> deny.
```

Durable credentials are checked independently of current acquisition mode. Current listing policy governs only users without an existing durable credential.

Distribution authorization and file-hash verification remain mandatory in every branch.

### Portable grant path

For an authenticated buyer without a local token, the server queries relays for entitlement-grant events with:

```text
#p = buyer_pubkey
#a = game_coordinate
```

For each candidate grant chain, it applies:

- the Entitlement Grant Draft's chain and Proof-of-Entitlement checks;
- ADP campaign anchoring;
- ADP issuance delegation;
- ADP publisher-only revocation authority.

Any one valid chain with a `granted` tip proves entitlement.

## Current listing versus historical authorization

The current listing is used for:

- current server authorization;
- current file hash;
- current build/version;
- current public or timed-access policy;
- current fulfillment delegation when issuing a new credential.

The campaign chain and grant chain are used for:

- historical campaign authorization;
- campaign state at grant issuance;
- grant signer authorization at issuance;
- durable buyer entitlement;
- later publisher-signed revocation.

A current listing update MUST NOT retroactively invalidate a valid paid receipt or Entitlement Grant.

## Ownership scope across listing versions

This is a general ADP ownership rule, not a rule specific to free acquisition. It applies equally to NIP-102 receipts and Entitlement Grants and SHOULD be incorporated into the base ADP-01 ownership semantics when this amendment is merged.

Unless a future listing or credential explicitly declares a narrower scope, a valid ownership credential for a game coordinate authorizes access to the current build published under that coordinate, including later versions.

A developer requiring separately purchased major versions SHOULD publish them under distinct `d` tags and therefore distinct game coordinates.

Entitlement Grants defined by this amendment inherit this coordinate-level ownership scope; they do not introduce a separate versioning rule.

## Security considerations

- **Publisher policy authority:** only the developer key may create, update, or cancel campaigns and revoke buyer grants.
- **Advisory listing pointers:** listing `campaign` tags improve discovery but never establish campaign validity or status.
- **Deliberate two-kind model:** campaign policy and buyer entitlement remain separate despite the additional allocation and implementation cost.
- **Fulfillment capability limitation:** fulfillment keys may issue credentials and deliver files but cannot alter policy or destroy entitlements.
- **No implicit public access:** zero or malformed price data never bypasses ownership checks.
- **Campaign immutability after start:** terms cannot be edited once the campaign begins; only cancellation is allowed.
- **Cancellation is prospective:** cancellation blocks new grants from its timestamp and does not invalidate prior grants.
- **Grant revocation is explicit:** prior grants remain valid until individually revoked by the developer through the grant chain.
- **Forks fail closed:** ambiguous campaign or grant chains are invalid.
- **Server-local idempotency:** independent servers may issue redundant but valid grants.
- **Durable proof precedence:** current listing mode cannot erase previously acquired rights.
- **Timed access is non-durable:** servers MUST NOT infer access after `ends`.
- **Distribution remains independent:** public access and ownership never authorize an unlisted server to distribute the file.

## Reference implementation additions

A compliant implementation of this amendment SHOULD:

- [ ] Discover campaigns from optional listing pointers when present.
- [ ] Support coordinate-based `#a` campaign discovery when pointers are absent or stale.
- [ ] Fetch specific campaign chains by developer author and indexed `d` tag.
- [ ] Treat listing campaign pointers as advisory only.
- [ ] Preserve indexed `d` and `a` across every campaign-chain event.
- [ ] Validate durable credentials before evaluating current ungated access policy.
- [ ] Never infer public ADP access from zero or malformed price data.
- [ ] Apply coordinate-level ownership scope consistently to receipts and grants.

## Open questions

- Formal allocation of `<entitlement-grant-kind>`.
- Formal allocation of `<adp-campaign-kind>`.
- Discount campaigns remain in the paid NIP-102 path and require separate pricing semantics if standardized later.

## Relationship to other specifications

| Specification | Role |
|---|---|
| Entitlement Grant Draft | Generic non-payment entitlement event and lifecycle. |
| NIP-102 | Payment-backed receipt; unchanged. |
| ADP-01 | Base distribution, delegation, upload, payment, and download protocol. |
| NIP-99 | Game listing coordinate and current listing metadata. |
