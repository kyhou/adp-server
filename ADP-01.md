# ADP-01: Arcadestr Distribution Protocol

`draft` `optional` `consolidated`

A minimal, open HTTP protocol for serving gated digital game files, using Nostr identities and event kinds as the authentication and ownership layer. Any operator can run a compliant ADP server; game developers choose which servers carry their builds.

Durable ownership is proven by one of two portable credentials:

- a [NIP-102](NIP-102.md) `kind:1020` receipt for a paid acquisition; or
- a [NIP-103](NIP-103.md) Entitlement Grant for a non-monetary acquisition issued under a publisher-signed claim-and-keep campaign.

Either credential is signed by the developer's own key or by a key the developer has delegated through a coordinate-scoped, capability-limited `kind:30406` authorization. Credentials are portable across servers and do not depend on any single server remaining online. A developer may additionally make a listing downloadable without a credential, through explicit public or timed access policy that creates no durable ownership.

ADP also defines developer-signed package and launch metadata, so that a verified archive can be installed and started by a client without trusting any server, and so that an installed game remains launchable when every server that ever carried it is gone.

ADP is modeled after [Blossom](https://github.com/hzrd149/blossom): servers are interchangeable, the protocol is the contract, and no central operator is privileged.

## Status of this document

ADP-01 is a draft and has not entered production. This document is the single authoritative ADP-01 specification. It consolidates the former base specification and the following amendments, which are retained only as superseded historical records in [docs/protocol-history/](docs/protocol-history/README.md):

- *ADP-01 Amendment: Free, Promotional, and Non-Monetary Acquisitions* (v3);
- *ADP-01 Amendment: Package Format, Launch Profiles, and Execution* (v1, including target-scoped `launch_default`).

Those documents MUST NOT be treated as competing normative specifications.

The canonical copy of this document is maintained in the Arcadestr repository. Copies elsewhere, including the `adp-server` reference implementation's, are byte-for-byte mirrors and are not edited independently. Conformance gaps of each implementation are tracked separately from this specification.

This consolidated draft is ADP protocol version **`0.3.0`**. Servers advertise it as `supported_adp` in `kind:30403` and `adp_version` in `/.well-known/adp`. Independently versioned schemas, such as the [split-settlement profile](#experimental-split-settlement-profile-version-1) (version `1`), keep their own version numbers and do not change the ADP protocol version.

Event kinds marked **provisional** or **experimental** in [Event kinds](#event-kinds) are not standardized allocations. Decisions taken during consolidation are recorded in [Resolved decisions](#resolved-decisions); remaining unresolved questions are listed in [Open Questions](#open-questions).

The key words MUST, MUST NOT, REQUIRED, SHOULD, SHOULD NOT, and MAY are to be interpreted as described in RFC 2119.

## Terminology

| Term | Definition |
|------|------------|
| **ADP server** | An HTTP server implementing this spec |
| **Developer** | The Nostr keypair that signed the game's [NIP-99](https://github.com/nostr-protocol/nips/blob/master/99.md) listing event. Also called the publisher. It is the only key with policy authority over the listing; see [Authority model](#authority-model). |
| **Fulfillment key** | A Nostr keypair authorized through a developer-signed `kind:30406` lifecycle for explicit operational capabilities |
| **Operator** | The party running an ADP server. May serve a single developer (including a developer running it for themselves) or many — an operator serving multiple developers issues each a distinct fulfillment key via [Provisioning](#provisioning-endpoints), never a key shared across unrelated developers. Identified by the same pubkey that signs its `kind:30403` announcement. |
| **Scope** | An optional label a developer attaches to a provisioning request, letting the same operator hold multiple independent fulfillment keys for the same developer (e.g. one per game, for isolation). Bookkeeping only — has no bearing on credential verification. |
| **Game coordinate** | `30402:<developer-pubkey>:<d-tag>` — the stable identifier for a listing |
| **Buyer** | The Nostr keypair acquiring or holding access: the payer of a paid acquisition, the claimant and recipient (`p`) of an Entitlement Grant, or the requester of a download. |
| **LSP** | Lightning Service Provider — the operator of the LNURL-pay endpoint that signs `kind:9735` zap receipts on a developer's behalf, resolved from a listing's `lud16` tag |
| **Durable credential** | A valid NIP-102 receipt or a valid Entitlement Grant for a buyer and game coordinate. Survives listing replacement, price changes, policy changes, and campaign cancellation. |
| **Download token** | An opaque, short-lived, server-issued token scoped to a buyer and game coordinate, returned by `/purchase/confirm` or `/entitlement/claim`. It is not a durable credential. |
| **Acquisition policy** | The current listing's ungated access policy: explicit public access or timed access. Absent policy means gated. It creates no durable credential. |
| **Campaign** | A publisher-signed, append-only claim-and-keep policy event chain for one game coordinate. |
| **Authorization root** | The immutable first event of a `kind:30406` lifecycle; its event ID is the anchor referenced by listings and delegated credentials. |
| **Package** | The archive identified by a listing's `file_hash`, in the format declared by its `package` tag |
| **Installation root** | The directory into which a package is extracted on the buyer's device |
| **Launch profile** | A publisher-signed, named statement of one way to start the game: an executable, a working directory, arguments, and a type |
| **Relative path** | A path expressed relative to the installation root, subject to [Path rules](#path-rules) |
| **Resolved snapshot** | The exact set of values a client used to perform one installation, persisted locally at install time |

## Canonical Hex Encoding

Every hex-encoded value controlled by this specification — `file_hash`, `fulfillment_pubkey`, authorization root and attestation event IDs, campaign and grant event IDs, payment-proof event IDs, the `payment_hash` used for [replay protection](#replay-protection), and any other field documented as `<...-hex>` or `<hex>` — MUST be **lowercase**. SHA-256 digests, event IDs, and x-only public keys are 64 lowercase hex characters.

This matches NIP-01, under which event IDs and public keys are already lowercase hex, and requires no change from any implementation using conventional hex encoding.

**Consumers MUST reject a non-lowercase value rather than normalizing it.** Uppercase is not an alternative encoding of the same value; it is invalid input. Servers, clients, and relays MUST NOT case-fold on read, MUST NOT compare case-insensitively, and MUST NOT accept a value that differs from its canonical form.

The reason is identity, not tidiness. `file_hash` keys stored artifacts and is compared byte-for-byte during upload verification, periodic re-verification, and post-download integrity checks. If two spellings of one digest are both accepted, they are two identities for one artifact: deduplication fails, one stored copy can satisfy a lookup while the other does not, and an implementation that normalizes on read interoperates inconsistently with one that does not. Rejecting is the only rule under which all participants agree on what a value *is*.

Normalization belongs at the human input boundary and nowhere else. A publishing client MAY lowercase a hash pasted or typed by a person, or carried over from a prior listing revision, before validating it. Having validated, it MUST emit the canonical form. Values arriving from another participant — a signed event, an HTTP request or response, a database — are never normalized; they are accepted as canonical or rejected.

This rule applies only to fields this specification explicitly defines as hexadecimal. It does not constrain identifiers whose format is defined as a UUID, an opaque string, or another non-hex form — including campaign `d` (`<campaign-id>`), grant `d` (`<grant-id>`), authorization `d` (`<authorization-id>`), `order` IDs, `scope`, listing `d` tags, and download tokens. Such identifiers MAY contain uppercase characters or non-hex characters, and are compared exactly as their own definitions require. A composite value applies the rule only to its hex components: in the `kind:30404` `d` tag `<developer-pubkey-hex>:<fulfillment-pubkey-hex>`, both pubkeys are lowercase hex.

This rule constrains hex-encoded values only. Bech32 identifiers (`npub`, `note`, `naddr`) have their own canonical form defined by NIP-19, and `lud16` is an address, not an encoding. Launch-profile platform values are not hex and follow their own alias normalization; see [Platform vocabulary](#platform-vocabulary).

This rule is normative for ADP. It changes no NIP-102 or NIP-103 requirement; it constrains how ADP participants emit, compare, and store values in ADP-controlled fields.

## Event kinds

| Kind | Name | Signer | Type | Allocation status |
|---|---|---|---|---|
| `30402` | Game listing ([NIP-99](https://github.com/nostr-protocol/nips/blob/master/99.md)) with ADP extensions | Developer | Parameterized replaceable | NIP-99 |
| `30403` | Server announcement | Operator | Parameterized replaceable | ADP-01 draft |
| `30404` | Fulfillment provisioning attestation | Operator | Parameterized replaceable | ADP-01 draft |
| `30406` | Fulfillment authorization lifecycle | Developer | Regular (non-replaceable) | ADP-01 draft |
| `1020` | Marketplace receipt ([NIP-102](NIP-102.md)) | Developer or authorized fulfillment key | Regular | NIP-102 draft |
| `<entitlement-grant-kind>` | Entitlement Grant ([NIP-103](NIP-103.md)) | Developer or authorized fulfillment key (root); developer only (revocation) | Regular | **Unallocated.** Implementations currently use `1030` as an **experimental, provisional** value. |
| `<adp-campaign-kind>` | ADP claim-and-keep campaign | Developer only | Regular | **Unallocated.** Implementations currently use `1031` as an **experimental, provisional** value. |
| `1032` | Commercial-terms root (split-settlement profile) | Operator | Regular | **Experimental**, not a standardized allocation |
| `1033` | Private processor quote (split-settlement profile) | Operator | Regular, never published to public relays | **Experimental**, not a standardized allocation |
| `1034` | Incomplete-settlement / refund evidence (split-settlement profile) | Operator or buyer | Regular | **Experimental**, not a standardized allocation |
| `27235` | HTTP Auth ([NIP-98](https://github.com/nostr-protocol/nips/blob/master/98.md)) | Requesting party | Ephemeral token | NIP-98 |
| `9735` | Zap receipt ([NIP-57](https://github.com/nostr-protocol/nips/blob/master/57.md)) | LSP | Regular | NIP-57 |

`<entitlement-grant-kind>` and `<adp-campaign-kind>` are symbolic placeholders. Neither may be replaced with an already assigned kind. Implementations MUST NOT deploy experimental values as interoperable production allocations. The previously proposed `kind:1021` MUST NOT be used for Entitlement Grants because it is already assigned by NIP-15. The provisional values `1030` and `1031` used by current implementations are development values only; they are not formally standardized allocations and this specification does not allocate them.

The use of two separate kinds for campaigns and grants is intentional. Campaign policy and buyer-specific entitlement are different protocol objects: campaigns are one-to-many publisher policy with publisher-only control and coordinate-based discovery; grants are one-to-one buyer credentials with delegated issuance and recipient-based discovery. Combining them would blur signer authority, lifecycle, revocation, indexing, and verification semantics.

## Authority model

Commercial policy is controlled only by the developer identity key. Fulfillment keys execute authorized operational actions but cannot create, modify, cancel, or revoke policy, and cannot revoke buyer entitlements.

| Action | Developer key | Authorized fulfillment key |
|---|---:|---:|
| Publish or update listing (including `server`, `file_hash`, package and launch metadata, acquisition policy, campaign pointers, `adp_terms`) | Yes | No |
| Create campaign | Yes | No |
| Update future campaign terms | Yes | No |
| Cancel campaign | Yes | No |
| Delegate or cancel fulfillment authorization (`kind:30406`) | Yes | No |
| Issue NIP-102 paid receipt | Yes | Yes, with `issue_receipt` |
| Issue Entitlement Grant under valid campaign | Yes | Yes, with `issue_grant` |
| Revoke buyer entitlement | Yes | No |
| Upload build | Yes | Yes, with `upload_build` |
| Issue download token | Yes | Yes |
| Serve authorized file | Yes | Yes, when delegated |

The developer defines policy. Fulfillment infrastructure executes policy.

Two rows describe server functions rather than signing-key capabilities:

- **Issue download token.** A download token is issued by the ADP server after it has issued a receipt or grant under the authority above; it is not a signed credential and confers no authority outside that server.
- **Serve authorized file.** Whether a server may serve a file is governed solely by the listing's `server` tags; see [Distribution authorization](#distribution-authorization). No key capability authorizes distribution, and no capability is required to serve.

## Listing Extensions (`kind:30402`)

A developer distributing a game via ADP adds the following tags to their [NIP-99](https://github.com/nostr-protocol/nips/blob/master/99.md) classified listing:

| Tag | Value | Description |
|-----|-------|--------------|
| `server` | `<https-url>` | Declares a server authorized to distribute this listing's file. Repeatable. |
| `file_hash` | `<sha256-hex>` | SHA-256 of the game archive, 64 lowercase hex characters — see [Canonical Hex Encoding](#canonical-hex-encoding). Servers MUST verify stored files against this hash before serving. |
| `version` | `<semver-string>` | Build version. Display only; MUST NOT be used for update detection — see [Updates](#updates). |
| `platform` | `<os>-<arch>` | Target platform. Repeatable for an archive that genuinely runs on several targets; a listing per platform is the simplest model. Required for execution; see [Eligibility](#eligibility) and [Platform vocabulary](#platform-vocabulary). |
| `package` | `<format>` | Container format of the file identified by `file_hash`. Exactly one per listing when launch profiles are present. See [The `package` tag](#the-package-tag). |
| `launch` | `<profile-id>`, `<executable>`, `<working-dir>`, `<arguments>`, `<type>`, `<display-name>` | One launch profile. Repeatable. See [The `launch` tag](#the-launch-tag). |
| `launch_default` | `<profile-id>`, `<target>`... | Publisher's automatic Play profile for the listed targets. Repeatable. See [The `launch_default` tag](#the-launch_default-tag). |
| `fulfillment_authorization` | `<authorization-root-event-id>`, `<fulfillment-pubkey>`, `<relay-hint>` | Enables new operations under the referenced `kind:30406` authorization. Repeatable; relay hint optional. The authorization chain, not the listing, owns validity and capabilities. |
| `lud16` | `<name@domain>` | The Lightning address whose LNURL-pay endpoint is authoritative for this listing's purchases. Required if the listing accepts the zap-receipt payment proof path (not required if only bolt11/preimage proof is ever used). MUST be read from this tag, never from the developer's `kind:0` profile — see [LSP Verification](#lsp-verification-lud16). |
| `acquisition` | `public` \| `timed-access`, `<starts_at>`, `<ends_at>` | Optional current ungated access policy; at most one. See [Acquisition policy](#acquisition-policy). |
| `campaign` | `<campaign-root-event-id>`, `<relay-hint>` | Advisory pointer to an active or upcoming campaign for this game. Repeatable; relay hint optional. See [Campaign pointers](#campaign-pointers). |
| `adp_terms` | `<operator-pubkey>`, `<kind-1032-root-event-id>` | Developer acceptance of one exact immutable experimental commercial-terms root. Repeatable once per distinct sale-time operator. Separate from `kind:30406` operational authority. See [Experimental split-settlement profile](#experimental-split-settlement-profile-version-1). |

`package`, `launch`, `launch_default`, `acquisition`, `campaign`, `fulfillment_authorization`, and `adp_terms` use multi-character names. This is intentional and consistent with NIP-01: only single-letter tag names are guaranteed relay-filterable, and none of these is ever a filter subject. They are payload, not index.

### Example Event

A gated, priced, launchable listing with delegated fulfillment:

```jsonc
{
  "kind": 30402,
  "pubkey": "<developer-pubkey>",
  "tags": [
    ["d", "my-game-v1"],
    ["server", "https://dist.arcadestr.io"],
    ["file_hash", "a3f5..."],
    ["version", "1.2.0"],
    ["platform", "windows-x86_64"],
    ["package", "zip"],
    ["launch", "default", "bin/Game.exe", "bin", "[]", "game", "Play"],
    ["launch", "vulkan", "bin/Game.exe", "bin", "[\"--renderer\",\"vulkan\"]", "game", "Play with Vulkan"],
    ["launch", "config", "ConfigTool.exe", ".", "[]", "config", "Configuration tool"],
    ["launch_default", "default", "windows-x86_64"],
    ["fulfillment_authorization", "<authorization-root-1>", "a1b2...", "wss://relay.example.com"],
    ["fulfillment_authorization", "<authorization-root-2>", "c3d4..."],
    ["lud16", "studio@getalby.com"]
  ],
  "content": "...",
  "sig": "<developer-signature>"
}
```

Optional policy and discovery tags, shown in isolation:

```jsonc
["acquisition", "public"]
["acquisition", "timed-access", "1760000000", "1760600000"]
["campaign", "<campaign-root-event-id>", "wss://relay.example.com"]
```

### Distribution authorization

`server` tags declare **distribution authorization only** — which servers may serve the file. They are independent of ownership, acquisition policy, and fulfillment-key authorization.

A server MUST confirm its own URL appears in the current listing's `server` tags for the requested `game_coordinate` before serving. A server not listed there MUST NOT serve the file, regardless of a presented credential's validity or of public or timed access policy. Distribution authorization and file-hash verification are mandatory for every access basis.

### Acquisition policy

The optional `acquisition` tag expresses only current ungated access policy:

| Tag | Value | Meaning |
|---|---|---|
| `acquisition` | `public` | Anyone may download while this is the current listing policy. No durable credential is issued. |
| `acquisition` | `timed-access`, `<starts_at>`, `<ends_at>` | Anyone may download while `starts_at <= now < ends_at`. No durable credential is issued. |

A listing with no `acquisition` tag follows gated behavior: access requires a download token or a durable credential.

A zero or missing price MUST NOT implicitly create public ADP access. Public access requires an explicit `acquisition: public` tag. A client may still expose a direct-download URL outside ADP for a free listing.

Timed access is non-durable: servers MUST NOT infer access after `ends_at`, and timed access creates no credential that survives the interval.

Public and active timed access permit anonymous download: a server MUST NOT require NIP-98 authentication or a download token to serve a request that the current listing's public or active timed-access policy authorizes. See [`GET /game/:game_coordinate`](#get-gamegame_coordinate).

Claim-and-keep campaigns are not defined by replaceable listing state. They use separate publisher-signed campaign events; see [Claim-and-keep campaigns](#claim-and-keep-campaigns).

#### Malformed acquisition policy

Policy parsing fails closed. A listing's acquisition policy is:

- **gated** when it has no `acquisition` tag;
- **public** when it has exactly one `acquisition` tag, exactly `["acquisition", "public"]`;
- **timed access** when it has exactly one `acquisition` tag, exactly `["acquisition", "timed-access", "<starts_at>", "<ends_at>"]`, with both values unsigned integer Unix seconds and `starts_at < ends_at`;
- **invalid** otherwise — including duplicate `acquisition` tags, an unknown mode, wrong element count, non-numeric or out-of-range timestamps, or `starts_at >= ends_at`.

Invalid policy MUST grant no policy access; it is treated as gated for access evaluation. Implementations MAY report it distinctly for diagnostics. Invalid policy MUST NOT invalidate the listing itself, its distribution metadata, durable credentials, purchases, or campaign claims.

### Campaign pointers

A publisher MAY add one or more repeatable campaign-pointer tags to the current listing:

| Tag | Value | Meaning |
|---|---|---|
| `campaign` | `<campaign-root-event-id>`, `<relay-hint>` | Advisory pointer to the immutable root event of an active or upcoming campaign for this game. The relay hint is optional. |

Publishing clients SHOULD add these pointers when creating or associating active or upcoming campaigns with a listing. They SHOULD remove pointers to cancelled or expired campaigns when the listing is next republished, but immediate removal is not required.

Campaign pointers are discovery hints only:

- their presence MUST NOT prove that a campaign is valid or active;
- their absence MUST NOT prove that no campaign exists;
- clients and servers MUST independently fetch and validate the referenced campaign chain;
- stale pointers are harmless when campaign-chain verification is performed;
- security MUST NOT depend on promptly republishing the listing after cancellation or expiration.

The campaign chain remains the authoritative source of campaign terms and status.

### Mandatory fields for client-published listings

Nothing in this section is a protocol-level restriction — any Nostr client can publish a `kind:30402` event with any subset of these tags, and relays MUST accept it regardless. This is guidance for a **publishing client's own UI**, not a wire-format requirement: a client that wants a listing to be immediately usable through ADP SHOULD enforce the following before allowing publish, and MUST clearly communicate to the buyer-facing side of the client when a listing lacks them, rather than silently degrading.

Three tiers, since they fail differently:

- **Payment-address tier — required for any priced listing, independent of ADP entirely:** `lud16`. Every priced listing needs a payment address regardless of whether ADP is involved; ADP additionally requires it be resolvable and stable per [LSP Verification](#lsp-verification-lud16), so a client SHOULD treat it as mandatory at publish time rather than optional.
- **Fulfillment tier — required only for listings that want automated install/download:** `file_hash`, `server` (≥1), and at least one applicable `fulfillment_authorization` unless the developer signs every operation directly. A listing missing these can still be a valid, purchasable listing — it simply cannot support one-click install, and a client SHOULD render it accordingly (e.g. Buy-only, no Install action), the same way an unsupported platform is already handled.
- **Launch tier — required only for listings that want the buyer to be able to play:** `package`, at least one `platform`, at least one `launch` profile of type `game`, and `launch_default` mappings that cover every declared target exactly once. A listing carrying `file_hash` and `server` but no launch tier remains valid and purchasable; the buyer can acquire and download it, and the client SHOULD present it as downloadable-but-not-launchable rather than silently offering a Play action that cannot work. Legacy launchable listings without `launch_default` remain consumable as described in [The `launch_default` tag](#the-launch_default-tag).

A publishing client enforcing the fulfillment tier SHOULD, before allowing submit:

1. Compute `file_hash` client-side from the selected build file rather than accepting it as freeform text — a hand-typed hash is a guaranteed source of publish/upload mismatches.
   Where a hash is nonetheless carried over from a prior revision or supplied by a person, lowercase it at capture and then validate it strictly; a listing published with a non-canonical hash will be rejected by both the upload path and the buyer's verification, having appeared valid at publish time.
2. For direct developer signing, no fulfillment authorization is required. For delegated operation, the client calls `POST /provision`, verifies the matching operator-signed `kind:30404`, creates or reuses a coordinate-scoped `kind:30406` authorization root with explicit capabilities, publishes it, and adds the corresponding `fulfillment_authorization` listing tag.
3. For each declared `server` URL, fetch `GET <server>/.well-known/adp` live as a basic reachability check. This confirms the URL is live and *a* server, but is not a substitute for step 2 — a server's single announced identity pubkey has no fixed relationship to any individual developer's delegated fulfillment key once an operator serves more than one developer. Clients SHOULD also check every intended server's advertised upload capacity; see [Upload capacity discovery](#upload-capacity-discovery).

A publishing client enforcing the launch tier SHOULD, before allowing submit:

1. Read the archive index and present the file listing. Executable candidates MAY be ranked — `.exe` on Windows targets, executable ELF files on Linux targets, known engine output layouts — but the client MUST NOT select one silently. The publisher selects; the client suggests.
2. Validate every profile path against [Path rules](#path-rules) and against the archive index, so a nonexistent path is caught before publication rather than after a buyer's download.
3. Report archive contents that will fail on a platform the listing declares: reserved device names, case-collisions, trailing dots or spaces, backslashes in entry names, symlinks. These originate in the publisher's build and are cheap to fix there and expensive to fix after distribution.
4. Warn when an edit removes a `profile-id` that a previous revision published, since installed clients may have it pinned. A client MUST NOT warn merely because a profile's executable path changed between revisions; per [the `launch` tag](#the-launch-tag), that is ordinary build reorganization.
5. Present every game profile as a candidate for Automatic Play, require every canonical target to be assigned exactly once, and show the exact target-to-profile mappings the resulting `launch_default` tags will encode. When only one canonical target exists, selecting an Automatic Play profile MAY assign that target immediately.

Independently of tier enforcement, publishing clients MUST assert the launch-metadata invariants in [Malformed tag sets](#malformed-tag-sets) before signing.

An embedded manifest inside the archive MAY be read as a source of publisher-facing defaults for automated or headless publishing. It MUST NOT be authoritative, and its values MUST NOT reach a published listing without the publisher reviewing and signing them.

A listing published through a different tool, without these fields, is not invalid — it is simply outside what this client's automated fulfillment or execution can act on. Sellers who wish to use a different publishing interface and still support ADP fulfillment need to supply these fields by whatever means that interface offers (manual tag entry, a companion tool, etc.); this spec does not mandate any particular publishing UI.

## Package Metadata and Launch Profiles

A verified archive is otherwise an opaque blob: nothing would state which file inside it is the game, what working directory it expects, or what arguments it takes. ADP therefore defines developer-signed launch metadata in the listing. No ADP server is privileged, servers are interchangeable, and an installed game must remain launchable when every server that ever carried it is gone; launch configuration is therefore developer-signed Nostr metadata, cached and served by nothing that has authority over it.

This version covers the one-listing-to-one-archive model. It does **not** define multiple artifacts per listing, immutable release manifests, or release channels; those are deferred to a future revision (see [Forward compatibility](#forward-compatibility)).

Package and launch requirements bind clients. Servers continue to store and serve one file per coordinate, verified against `file_hash`, and do not interpret `package`, `launch`, or `launch_default`.

### The `package` tag

| Tag | Value | Description |
|-----|-------|-------------|
| `package` | `<format>` | Declares the container format of the file identified by `file_hash`. Exactly one per listing when launch profiles are present. |

Defined values:

| Value | Format |
|---|---|
| `zip` | ZIP archive, including ZIP64 |
| `tar.zst` | tar stream, Zstandard-compressed |
| `tar.gz` | tar stream, gzip-compressed |

`zip` denotes ZIP with or without the ZIP64 extensions. There is no separate token for ZIP64, and a publisher whose archive exceeds the 4 GiB limits of the original format does not change how the package is declared.

Every defined format is an archive. A bare executable is not a package in this version: it would require a parallel install path with no extraction, no internal paths, and undefined profile semantics, for a case that occurs almost exclusively with single-binary Linux builds. Such a build is publishable by placing the executable in an archive. A `raw` format may be added later without breaking any listing published under this version.

Clients MUST NOT infer format by content sniffing when a `package` tag is present. Clients MUST refuse to extract a package whose declared format they do not implement, and MUST report this as an unsupported-package state rather than attempting a fallback.

A listing with no `package` tag and no `launch` tags is downloadable and verifiable, but the client offers no execution. A listing without a `package` tag offers no execution in this version; see [Open Questions](#open-questions) on legacy package detection.

### The `launch` tag

Repeatable. Each occurrence defines one launch profile.

```
["launch", "<profile-id>", "<executable>", "<working-dir>", "<arguments>", "<type>", "<display-name>"]
```

All six profile elements are REQUIRED and positional (seven tag elements including the `launch` name). Elements beyond the seventh MUST be ignored by current implementations and MUST NOT be treated as an error; this reserves room for additive extension.

| Position | Element | Rules |
|---:|---|---|
| 1 | `profile-id` | Stable publisher-assigned identifier. `[a-z0-9][a-z0-9_-]{0,63}`. MUST be unique within the listing. MUST NOT be reassigned to a different profile in a later listing revision. |
| 2 | `executable` | Relative path to the file to execute. Subject to [Path rules](#path-rules). MUST NOT be empty. |
| 3 | `working-dir` | Relative path to the process working directory. Subject to [Path rules](#path-rules). `.` denotes the installation root. |
| 4 | `arguments` | A JSON array of strings. `[]` when there are no arguments. Subject to [Argument rules](#argument-rules). |
| 5 | `type` | One of `game`, `config`, `server`, `editor`, `tool`. Unrecognized values MUST be treated as `tool`. |
| 6 | `display-name` | Human-readable label, 1–64 characters, no control characters. |

See [Example Event](#example-event).

#### Malformed tag sets

Malformed metadata is handled at two levels.

A listing MUST be treated as having no launch tier — downloadable but not launchable — when it carries more than one `package` tag, or when two `launch` tags contain the same syntactically valid `profile-id`. The duplicate rule applies even when one of those tags is invalid for some other reason: once two tags claim the same valid identifier, their intended relationship is ambiguous and the client MUST NOT resolve that ambiguity by tag order.

First-occurrence-wins is explicitly rejected for both cases. It would make which executable runs depend on tag ordering within a signed event, and it would let a partially-correct edit silently repoint an identifier that installed clients already hold. Failing the launch tier is visible; resolving by position is not.

An individual `launch` tag is invalid when a required positional element is missing, `profile-id` does not match its grammar, `executable` or `working-dir` violates [Path rules](#path-rules), `arguments` is not a JSON array of strings or violates [Argument rules](#argument-rules), or `display-name` violates its length or control-character rule. An invalid individual profile MUST be ignored for execution and MUST NOT make an otherwise unambiguous launch tier fail. Unrecognized `type` values are not malformed; they are treated as `tool` as specified above.

Clients SHOULD surface discarded profiles and the reason each profile was discarded rather than silently omitting them. Publishing clients SHOULD present this before signing; consuming clients SHOULD make it available when inspecting a listing or diagnosing why an expected launch option is absent.

After invalid individual profiles are discarded, a listing with no valid profile of type `game` MUST NOT be offered for execution. It remains downloadable.

This case is reachable in practice. `kind:30402` is replaceable, so an editing client rebuilds the entire tag set on every revision; a client that carries unmanaged tags forward without filtering the `launch` family will emit the previous revision's profiles alongside the new ones. Publishing clients MUST assert, before signing, that exactly one `package` tag, no duplicate syntactically-valid `profile-id`, and no invalid `launch` tag are present.

#### Profile identity

`profile-id` identifies a **semantic launch option**, not an executable path. It answers "which way of starting this game is this," and its meaning must remain constant across listing revisions.

Changing which file a profile points at is therefore ordinary. A build reorganization moving `Game.exe` to `bin/Game.exe` keeps `profile-id` `default`: the option is still "play the game," and only its implementation moved. Repurposing `default` from the main game to a dedicated server is reassignment and is prohibited — the correct action is a new `profile-id`.

This matters because a local installation pins the `profile-id` the buyer selected, and re-pins it after an update. Stable semantics mean the buyer's choice survives; unstable semantics mean it silently becomes a different program.

### The `launch_default` tag

A launchable listing SHOULD identify the publisher's automatic Play action for every declared target. The tag is repeatable and target-scoped:

```json
["launch_default", "<profile-id>", "<target>", "<target>", ...]
```

For example:

```json
["launch_default", "windows", "windows-x86_64"],
["launch_default", "native-unix", "linux-x86_64", "macos-aarch64"]
```

Each referenced profile MUST exist in the same listing and MUST have type `game`. Each target MUST be a canonical full `<os>-<arch>` value declared by a `platform` tag. A profile MAY cover several targets, but every declared target MUST be covered by exactly one `launch_default` tag and a profile MUST appear in at most one such tag. Publishing clients MUST reject uncovered targets, overlapping assignments, undeclared targets, empty target lists, and mixed global and scoped forms.

The two-element form remains the legacy global form:

```json
["launch_default", "<profile-id>"]
```

Consumers MUST interpret it as applying to every declared target. Publishing clients MAY emit it only when the listing declares exactly one canonical target; multi-target publications MUST use explicit scoped targets.

Listings published without the tag remain valid. A consumer resolves only claims for its complete normalized OS and architecture. Exactly one valid matching claim selects the publisher default. Ambiguous, malformed, dangling, or non-game claims for that target remove the publisher preference without invalidating the launch tier; defects that concern only another target do not suppress a valid local claim. Unknown future targets are ignored. Mixing legacy-global and scoped claims is ambiguous and removes the publisher preference for every target. The fallback remains the first available `game` profile in signed tag order.

The publisher preference is not a buyer preference. A client MAY let the buyer save another default locally. A valid, available buyer preference takes precedence; clearing it restores the publisher preference. If either preference is missing or unavailable, the client MUST NOT execute a tool or unavailable profile as a fallback.

### Eligibility

In this version, a listing's `platform` tags govern whether the client offers execution at all. Launch profiles carry no eligibility of their own: every profile in a listing is eligible on every platform that listing declares. A target on `launch_default` chooses the automatic Play preference for that target; it does not prohibit manually selecting that profile elsewhere.

This follows from the one-archive-per-listing model. Per-profile eligibility becomes meaningful only when one listing carries artifacts for several platforms, which this version does not permit.

#### Platform vocabulary

A `platform` tag value is `<os>-<arch>`, split on the first hyphen. Neither defined component contains a hyphen.

| Component | Canonical values |
|---|---|
| `os` | `windows`, `linux`, `macos` |
| `arch` | `x86_64`, `aarch64` |

These are the values of `std::env::consts::OS` and `std::env::consts::ARCH`. Naming them by reference rather than inventing a vocabulary means a client's host detection and its listing comparison use the same strings without a translation layer.

Publishing clients MUST emit canonical values. Consuming clients MUST normalize the following aliases before comparison, since listings published by other tools will use them:

| Alias | Canonical |
|---|---|
| `win`, `win32`, `win64` | `windows` |
| `osx`, `darwin`, `mac` | `macos` |
| `amd64`, `x64`, `x86-64` | `x86_64` |
| `arm64` | `aarch64` |

Comparison is case-insensitive after normalization.

A `platform` value that does not normalize to a defined pair MUST be treated as not matching the host, and MUST NOT be treated as a parse error or as matching everything. A listing declaring only unrecognized platforms is surfaced as unsupported on this device — the same state as a genuine platform mismatch. A listing declaring only unrecognized platform values has a complete launch tier and is launchable nowhere. This is what allows a target this version does not define to be published now and become launchable later without any listing change.

A listing that declares no `platform` tag makes no claim about where it runs. Such a listing MUST NOT be offered for execution: a client MUST treat it as having no launch tier, regardless of whether `package` and `launch` tags are present.

This differs from how a `platform`-less listing behaves for download, which is unrestricted. The asymmetry is deliberate. The permissive reading has a concrete failure mode for execution: a Windows executable offered on a Linux host. A publisher whose archive genuinely runs on several targets declares each of them.

## Server Discovery

### The `kind:30403` Server Announcement Event

ADP servers publish a parameterized replaceable event to announce themselves. Clients and developers query relays for `kind:30403` to discover available servers.

| Tag | Value | Description |
|-----|-------|--------------|
| `d` | `<server-slug>` | Replaceable-event identifier |
| `name` | Human-readable server name | |
| `url` | `<https-url>` | Base URL for HTTP endpoints |
| `supported_adp` | `<semver-string>` | ADP protocol version supported |
| `contact` | Free text | Operator contact |

A server implementing the [Experimental split-settlement profile](#experimental-split-settlement-profile-version-1) additionally advertises the fields listed there.

#### Example Event

```jsonc
{
  "kind": 30403,
  "pubkey": "<server-pubkey>",
  "tags": [
    ["d", "arcadestr-dist"],
    ["name", "Arcadestr Official Distribution"],
    ["url", "https://dist.arcadestr.io"],
    ["supported_adp", "0.3.0"],
    ["contact", "ops@arcadestr.io"]
  ],
  "content": "",
  "sig": "..."
}
```

`GET /.well-known/adp` provides the same identity over HTTP; see [HTTP Endpoints](#get-well-knownadp).

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

No capability authorizes campaign creation, update, or cancellation; listing publication; `kind:30406` creation or cancellation; or Entitlement Grant revocation. Those actions are reserved to the developer key; see [Authority model](#authority-model).

### Current listing versus historical authorization

The current listing is used for:

- current server distribution authorization;
- current file hash and build/version metadata;
- current public or timed-access policy;
- enabling a fulfillment authorization for a new receipt, grant, or upload.

Historical delegated credential verification uses:

- the credential's immutable `authorization` root;
- the `kind:30406` lifecycle state and required capability at credential creation time;
- the campaign chain and grant chain where applicable.

A new delegated operation at time `T` requires both:

1. the current listing contains a matching `fulfillment_authorization` tag; and
2. the referenced authorization chain is active at `T` and contains the required capability.

A previously issued delegated credential remains valid without current listing membership when:

1. it contains `['authorization', '<root-id>']`;
2. its signer matches the fulfillment key in that root;
3. the root is bound to the same developer and game coordinate;
4. the authorization was active and had the required capability at the credential root's `created_at`.

A current listing update MUST NOT retroactively invalidate a valid receipt or Entitlement Grant. Removing a `fulfillment_authorization` reference from a replacement listing prevents new operations under it but MUST NOT retroactively invalidate credentials created while it was active.

Direct developer-signed receipts, grants, and uploads require no `kind:30406` event. Delegated credentials without an `authorization` tag are invalid. No legacy listing-delegation fallback is defined.

### Fulfillment Provisioning Attestation (`kind:30404`)

`kind:30404` is an operator-signed, parameterized replaceable attestation that the operator provisioned and claims possession of a fulfillment key for a developer. It is checked during provisioning and authorization creation, but ordinary historical credential verification relies on the developer-signed `kind:30406` lifecycle.

#### Attestation tags

| Tag | Value | Description |
|---|---|---|
| `d` | `<developer-pubkey-hex>:<fulfillment-pubkey-hex>` | Replaceable-event identifier. Required. MUST equal the developer pubkey and fulfillment pubkey from the `p` and `fulfillment_pubkey` tags, joined by `:`. |
| `p` | `<developer-pubkey-hex>` | Developer for whom the key was provisioned. Required, exactly one. |
| `fulfillment_pubkey` | `<fulfillment-pubkey-hex>`, `<valid_from>`, `<revoked_at>` | Provisioned key, inclusive start of the operator's attestation in Unix seconds, and revocation time in Unix seconds. Required, exactly one. `revoked_at` is the empty string while the key is not revoked. |
| `scope` | `<scope>` | Present when the provisioning request named a non-empty scope. |

The event `pubkey` is the operator; it MUST equal the pubkey of the operator's `kind:30403` announcement and `/.well-known/adp`. `content` is empty. The event's `created_at` equals `valid_from`. Revocation republishes the same `d` with `revoked_at` set; because the event is replaceable, the revoked revision supersedes the active one.

```jsonc
{
  "kind": 30404,
  "pubkey": "<operator-pubkey>",
  "created_at": 1700000000,
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

A matching `30404` MUST name the same operator, developer, and fulfillment key before a publishing client creates a delegated authorization. The attestation is active at `T` when `valid_from <= T` and either `revoked_at` is empty or `T < revoked_at`. Operator revocation means the operator claims it no longer holds or uses the key; it does not retroactively invalidate credentials and does not replace developer cancellation of `30406`.

### Fulfillment Authorization (`kind:30406`)

`kind:30406` is a **regular, non-replaceable** developer-signed authorization event. A root and its chained updates form an append-only lifecycle. The operator never constructs or publishes `kind:30406`.

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

Cancellation is terminal. No valid event may succeed a cancellation; a successor of a cancellation is an invalid transition. Restoring delegated authority requires a new authorization root.

#### Chain resolution

Authorization chains use the [valid-prefix model](#valid-prefix-chain-resolution).

The authorization is active at time `T` when `valid_from <= T` and no valid cancellation has `created_at <= T`.

#### No early activation

Activation boundaries are exact. A verifier MUST NOT apply a clock-skew or observer allowance that treats a `kind:30406` authorization or a `kind:30404` attestation as active before its `valid_from`. A credential whose root `created_at` precedes `valid_from` is unauthorized.

## Valid-prefix chain resolution

`kind:30406` authorization chains and campaign chains use the valid-prefix model:

- an event joins the valid chain only when its signature, invariant fields, transition, (for campaigns) timing, and immediate predecessor are valid;
- an invalid event does not alter effective state and is excluded from valid-successor fork analysis;
- descendants referencing an invalid event are unreachable;
- the developer may recover with a valid sibling referencing the last valid tip;
- multiple valid successors of one valid predecessor are a fork and fail closed;
- cycles and self-references are invalid;
- verifiers MUST NOT resolve ambiguity by `created_at`.

At historical time `T`, only valid reachable events with `created_at <= T` are considered. A later malformed event cannot retroactively invalidate an earlier credential.

NIP-102 receipt chains and NIP-103 grant chains follow the chain rules of their own specifications; ADP adds only the signer-authority rules in [Paid acquisitions](#paid-acquisitions-nip-102) and [Entitlement Grants](#entitlement-grants-nip-103-profile).

## Relay availability

Event authenticity, event availability, and current-state freshness are separate properties. A valid signature proves who authored an event and that it is intact. Successful retrieval proves that the event exists. Neither proves global completeness, nor that no newer publisher-signed event exists on another relay.

A verifier MUST distinguish invalid evidence from insufficient evidence. An invalid, mismatched, or cancelled chain is rejected. Incomplete relay coverage or a missing required predecessor produces an indeterminate/unavailable result, not a declaration that the credential is cryptographically unauthorized. NIP-102 imposes the same rule for receipt chains.

### Positive evidence

**Positive evidence is self-validating.** A valid, signature-verified event from any single relay establishes the existence of that event. It does not become more true because a second relay also returned it, and a relay that did not answer cannot make it false. Two independent relays MUST NOT be required merely to establish that an event exists, and a verifier that has received a valid event satisfying a query MUST NOT treat it as unavailable because fewer relays responded than some threshold.

A verifier MUST select from valid candidates found among completed responses **before** applying any completeness threshold. Where multiple valid listing events are available, clients and servers MUST select the latest according to NIP-01 replaceable-event ordering. Selecting available positive evidence does not mean accepting the first response without considering others that completed.

Positive evidence of a credential never substitutes for a complete required chain: finding a root does not prove the absence of a later cancellation, revocation, or fork.

### Negative evidence and confirmed absence

**Negative evidence requires corroboration.** Absence proves nothing from one relay: a relay may not carry the event, may have dropped it, or may be selectively withholding it. An event, listing, or collection MUST NOT be declared nonexistent merely because one relay returned no matching event. Confirmed absence requires sufficient completed queries under the implementation's documented relay-evidence policy. The completeness threshold governs whether an absence is conclusive, never whether a present event counts.

### Freshness-sensitive operations

The following operations require **current** listing evidence:

- issuing a NIP-102 receipt;
- issuing an Entitlement Grant;
- accepting a build upload;
- validating current distribution authorization (the `server` tags) before serving a file.

For these operations, implementations MUST distinguish successful retrieval from sufficient freshness assurance: a single valid listing does not guarantee that no newer listing exists elsewhere. Implementations MUST apply a documented freshness policy, and MUST NOT silently accept potentially stale cached data when that policy cannot be satisfied.

Implementations MAY define a trusted relay set with explicit freshness and publication assumptions, and their freshness policy MAY rely on it. This specification does not define a universal relay quorum.

### Indeterminate results

When required relay evidence is unavailable or insufficient — including when a freshness-sensitive operation cannot satisfy its freshness policy — the operation MUST fail closed with an indeterminate/unavailable result rather than declaring the listing, credential, or campaign invalid or nonexistent. HTTP endpoints return `503 Service Unavailable`; see [HTTP errors](#http-errors).

### Relay selection

Implementations SHOULD prioritize the publisher's write relays advertised through NIP-65 when retrieving events authored by that publisher. They MAY additionally prioritize application-configured relays.

ADP MUST NOT require the use of any specific relay, including any relay operated by Arcadestr. Publishers and independent implementations remain free to use alternative relays.

### Availability trade-off for other reads

Reads that are not freshness-sensitive — for example, a client resolving the listing it will install from, or browsing — MAY act on the best valid evidence received. A relay that did not answer may hold a newer revision of a replaceable event, so such a reader may act on a superseded revision. That is the better failure for these reads: an installation pins the snapshot it resolved and a superseded revision is recoverable by updating, whereas an install blocked because one of several relays was unreachable is not recoverable at all, and makes availability depend on the least available relay rather than the most available one — inverting the redundancy relays exist to provide. This trade-off does not relax the freshness requirements above.

## Ownership and Access

### Access bases

ADP distinguishes durable ownership from temporary or policy-based access:

| Basis | Durable | Portable across servers | Created by | Survives listing/policy change |
|---|---|---|---|---|
| NIP-102 receipt | Yes | Yes | Paid acquisition | Yes, until the receipt chain tip is `refunded` (see [Receipt status under ADP](#receipt-status-under-adp)) |
| Entitlement Grant | Yes | Yes | Campaign claim | Yes, until individually revoked by the developer |
| Download token | No | No (opaque, issuing server only) | `/purchase/confirm`, `/entitlement/claim` | Until token expiry |
| `acquisition: public` | No | Policy is evaluated by every listed server; anonymous | Current listing | No |
| `acquisition: timed-access` | No | Policy is evaluated by every listed server; anonymous | Current listing | No; never after `ends_at` |

Durable credentials are checked independently of current acquisition mode. Current listing policy governs only users without an existing durable credential. Current listing mode cannot erase previously acquired rights.

Public or timed access and download tokens are not durable ownership. Clients MUST NOT present installation as evidence of durable ownership; see [Execution is DRM-free](#execution-is-drm-free).

### Ownership scope across listing versions

This rule applies equally to NIP-102 receipts and Entitlement Grants.

Unless a future listing or credential explicitly declares a narrower scope, a valid ownership credential for a game coordinate authorizes access to the current build published under that coordinate, including later versions.

A developer requiring separately purchased major versions SHOULD publish them under distinct `d` tags and therefore distinct game coordinates.

Entitlement Grants inherit this coordinate-level ownership scope; they do not introduce a separate versioning rule.

### Trust Model

Ownership proof (a `kind:1020` receipt or an Entitlement Grant) and distribution authorization (`server` tags) are deliberately separate. A credential is self-sovereign: any party can verify it against the listing, the authorization lifecycle, and (for grants) the campaign chain, without trusting the server that issued it. `server` tags are a distinct, developer-controlled allowlist of who may hold and serve copies of the file — a server MUST NOT infer distribution rights from a valid credential or from public or timed access policy alone, since that would let any node holding a leaked copy of the archive serve it to anyone, regardless of whether the developer sanctioned that node as a mirror.

Ownership verification across servers requires no relationship between the issuing server and the serving server: the credential and its delegation are independently verifiable by anyone. Download tokens are the only server-local access basis.

## Paid Acquisitions (NIP-102)

A paid acquisition produces a [NIP-102](NIP-102.md) `kind:1020` receipt. NIP-102 is unchanged by ADP and defines the receipt shape, payment binding, encrypted content, status chain, and Proof of Ownership. ADP adds:

- **Signer authority.** The root receipt MUST be signed by the developer encoded in the receipt's `a` coordinate, or by a fulfillment key whose `kind:30406` authorization contained `issue_receipt` and was active at the receipt root's `created_at` (see [Current listing versus historical authorization](#current-listing-versus-historical-authorization)).
- **Authorization anchor.** A delegated root receipt MUST contain an `authorization` tag referencing the immutable `kind:30406` root. A direct developer-signed receipt omits it.
- **New issuance.** A server issuing a new delegated receipt MUST additionally require that the current listing enables the authorization.
- **LSP resolution.** The consuming-protocol LSP-resolution rule that NIP-102 defers to is [LSP Verification](#lsp-verification-lud16).
- **No free receipts.** NIP-102 requires payment proof for every receipt and is unchanged. Zero-price or non-monetary acquisitions therefore cannot be represented as receipts; they use Entitlement Grants.

Discount campaigns remain in the paid NIP-102 path; see [Open Questions](#open-questions).

### Receipt status under ADP

NIP-102 leaves to the consuming protocol which chain-tip statuses constitute ownership and who may sign each status transition. ADP defines:

| Tip status | Ownership under ADP |
|---|---|
| `paid` | Retained |
| `fulfilled` | Retained |
| `disputed` | Retained |
| `refunded` | **Not** retained |

**Transition authority.** The developer key encoded in the receipt's `a` coordinate controls commercial status transitions and may sign any status update. A delegated fulfillment key may sign a status update only under explicit status-update authority granted by the developer in the referenced `kind:30406` root. Root-issuance capabilities (`issue_receipt`, `issue_grant`, `upload_build`) do not confer status-update authority. This version defines no status-update capability; therefore, under this version, a status update signed by a fulfillment key is not transition-authorized and MUST NOT alter the resolved status. See [Open Questions](#open-questions).

Status updates MUST otherwise satisfy NIP-102's chain invariants, including preservation of the root `authorization` value.

As with every commerce-layer status, a `refunded` tip affects future downloads and displayed ownership only; it never disables an existing installation (see [Execution is DRM-free](#execution-is-drm-free)).

### Replay protection

The replay identifier for every payment-proof format is the payment's canonical Lightning `payment_hash`, encoded as 64 lowercase hex characters:

- for a zap-receipt proof, the payment hash of the BOLT-11 invoice carried by the zap receipt;
- for a bolt11/preimage proof, the invoice's payment hash, which MUST equal `sha256(preimage)`;
- for the split-settlement profile, the payment hash of each leg.

A previously accepted `payment_hash` MUST NOT authorize a new purchase or produce a second receipt, even when the later submission uses a different proof format.

Replay protection distinguishes a legitimate retry from payment reuse. The **original purchase** of an accepted `payment_hash` is identified by the authenticated buyer, the `game_coordinate`, and that `payment_hash`.

- **Retry.** A submission by the same authenticated buyer, for the same `game_coordinate`, of the same accepted `payment_hash` repeats the original purchase. It covers a lost response and a failed or interrupted receipt publication. The server MUST return the original signed receipt unchanged, preserving its event ID. It MUST NOT sign another receipt. If the server does not know that the original receipt was published successfully, it MUST publish that same receipt again; it MAY republish it in any case. The server MAY issue a fresh download token, but only if the buyer remains entitled under the receipt's current status (see [Receipt status under ADP](#receipt-status-under-adp)) and the current listing authorizes this server under [Distribution authorization](#distribution-authorization). Otherwise the response carries the receipt without a token. A retry does not re-evaluate the current listing's price or acquisition policy.
- **Reuse.** Any other submission of an accepted `payment_hash` MUST be rejected with `409 Conflict`. This includes a different buyer, a different `game_coordinate`, and a conflicting purchase, such as a payment accepted as a leg of the [split-settlement profile](#experimental-split-settlement-profile-version-1). A `409` is never evidence of ownership.

The acceptance check, the record of acceptance and the stored receipt to be returned on retry MUST be committed atomically. For example, a unique constraint on the stored `payment_hash` can be enforced in the transaction that stores the signed receipt. Concurrent submissions of one payment, whether retries or reuse, then yield at most one receipt. The signed receipt MUST be durably stored with its acceptance before publication is attempted. An acceptance is never released, and a failed or interrupted publication remains recoverable by retry without issuing a second receipt.

### LSP Verification (`lud16`)

Confirming a `kind:9735` zap receipt's signature and its `p`/`a` tag references is not sufficient proof of payment on its own — those checks alone accept any self-signed event with the right shape. A server MUST additionally confirm the receipt was signed by the listing's actual Lightning Service Provider (LSP), resolved as follows:

1. Read the listing's `lud16` tag (`name@domain`).
2. Resolve it to an LNURL-pay endpoint per [LUD-16](https://github.com/lnurl/luds/blob/luds/16.md): `https://<domain>/.well-known/lnurlp/<name>`.
3. Fetch the endpoint and extract its declared `nostrPubkey`, per [NIP-57](https://github.com/nostr-protocol/nips/blob/master/57.md)'s zap metadata requirements.
4. Require `zap_receipt.pubkey == nostrPubkey`.

This resolution **MUST** use the `lud16` tag on the listing event itself — **never** a fallback to the developer's `kind:0` profile metadata. A listing's `lud16` is the single, self-contained source of payment-routing authority for that listing, consistent with how `file_hash`, `server`, and `fulfillment_authorization` are already scoped to the listing rather than to a separately mutable profile event. A developer who changes their profile `lud16` does not thereby change which LSP is authoritative for an already-published listing; they must republish the listing to change it.

If a listing has no `lud16` tag, the zap-receipt payment path MUST NOT be accepted for it — this is a hard failure (`402`), not a silent skip of the LSP check. The bolt11/preimage self-contained proof path has no LSP-trust dependency and remains available regardless.

### Experimental split-settlement profile (version 1)

This optional profile uses currently unallocated regular Nostr kinds. It is an
implementation experiment, **not a standardized kind allocation**:

- `kind:1032` is an immutable operator-signed commercial-terms root. It binds
  `version`, operator pubkey, Lightning destination, fee (`none`, positive fixed
  integer msat, or positive integer basis points with optional min/max), and
  limitations. A later revision is a new root. An operator-signed withdrawal
  references a root and stops only new quotes; it cannot alter issued quotes,
  receipts, or ownership.
- `kind:1033` is a private signed processor quote. It binds the buyer, exact
  listing event, accepted terms roots, processor, gross, destination commitment,
  ordered legs, and fixed deadlines. It MUST NOT be published to public relays.
- `kind:1034` is append-only signed incomplete-settlement/refund evidence. It
  never grants ownership. Sensitive recovery fields belong in NIP-44 ciphertext.

The aggregate receipt shape is defined by NIP-102's experimental aggregate-payment profile.

The HTTP endpoints used to create quotes, disclose invoices, submit proofs, and coordinate recovery and refunds are **outside this specification** until their interoperability requirements are established. Implementations of this profile define them as implementation-specific endpoints; they are not ADP-01 endpoints and are not required for compliance. Only the `POST /purchase/confirm` behavior described below is specified here.

Announcements and `/.well-known/adp` advertise version `1`, these experimental
kinds, split support, current terms root, and whether legacy new sales are
accepted. The announcement signer, terms signer, and sale processor MUST be the
same operator pubkey. Multiple URLs signed by that pubkey are one operator, one
accepted root, and at most one fee.

#### Consent and pricing

A split-enabled listing MUST contain exactly one valid `adp_terms` reference for
every distinct operator in its `server` set, including operators offering
`none`. Missing, conflicting, forged, withdrawn-at-quote-time, or unverifiable
coverage blocks publication and quote creation. Ordinary listing replacement
preserves references unless the developer explicitly changes them. Listings
without `adp_terms` remain legacy fee-free single-payment listings. An operator
MAY explicitly decline new legacy sales, but MUST NOT infer consent or deduct a
fee. `kind:30406` remains operational authority and is never commercial consent.

Split gross MUST be SAT/SATS or BTC exactly convertible to integer msat. Fiat
requires a future oracle/quote profile. For each operator independently:

`fee = clamp(floor(u128(gross_msat) * basis_points / 10000), min, max)`.

Overflow, non-positive fixed/basis-point values, invalid bounds, or aggregate
operator fees greater than or equal to gross invalidate the quote. The developer
gets the exact remainder. Zero creates no leg. Every positive amount MUST be
invoiced exactly; implementations MUST NOT drop, shift, or round a dust leg.
Leg order is ascending `(amount_msat, role, recipient_pubkey)` and is committed
by the quote.

#### Quote activation and payment

Issuance fixes `payment_expires_at = issued_at + 1800` and
`confirmation_expires_at = issued_at + 86400`. A signed quote begins `draft`,
then `preflighting`; it is not payable until the processor validates the buyer's
designated Lightning-address/LNURL-pay endpoint and obtains a processor-controlled
exact invoice for **every** positive leg. The destination's `minSendable` and
`maxSendable` MUST cover every possible per-leg principal refund. Its plaintext
stays private; the quote binds a canonical commitment. A buyer-authorized
replacement preserves history and never resets `failure_at` or extends refund
deadlines.

Each attempt records LNURL origin/callback, LSP pubkey, BOLT-11 payment hash,
expiry, and proof. NIP-57 Zap Requests are buyer-signed and merely forwarded by
the processor. The processor MUST recheck quote state, hard `kind:30406`
cancellation, and listing authorization before disclosing each invoice. It
discloses one invoice at a time in committed order. An expired invoice may be
replaced only for the same unpaid leg, recipient, and amount; all attempts remain
recorded and replacement expiry cannot exceed the confirmation deadline.

Recipient binding uses **processor-attested LNURL provenance**. The recipient's
destination MUST come from the developer-signed listing or accepted operator
terms. The processor independently fetches the exact invoice from that
destination and persists the validated endpoint, callback, invoice, payment
hash, amount, recipient identity, and LNURL/LSP metadata. Its signed quote and
aggregate receipt attest this binding. A preimage proves settlement of the
invoice, not ultimate recipient identity; third-party verification therefore
trusts the authorized processor's signed provenance attestation. Refunds use
the same rule with the buyer-authorized destination. NIP-57 evidence is optional
corroboration and, when present, its buyer-signed request, receipt, provider,
payment hash, amount, quote, and leg bindings MUST all validate.

No new invoice is disclosed after the payment deadline or after failure. A
disclosed BOLT-11 remains independently payable until its own expiry; later
settlement MUST be recorded and included in recovery obligations. If a required
leg is undisclosed at the payment deadline, `failure_at` equals that deadline.
If all were disclosed and only final proof is pending, proof may arrive until
the confirmation deadline, which becomes `failure_at` on timeout. There is no
partial rollover. A new purchase is wholly independent and cannot reuse a leg.

`POST /purchase/confirm` remains backward-compatible. For version 1 it is
idempotent by quote/purchase identity and verifies quote signature and state,
buyer, exact listing and terms snapshot, current hard authorization, deadlines,
all recipients, amounts, LSPs, invoices, globally unique proofs, aggregate sum,
and NIP-102 commitment. It issues one aggregate receipt and ownership/download
token only after complete settlement. Publication retries reuse that exact root.

#### Incomplete settlement and refunds

Failure stops disclosure but never erases late proof. Each paid recipient owes
only principal actually received; the processor coordinates and is not a
guarantor. Routing fees are visible and non-refundable by default. Partial
payment creates no ownership and refunds are not guaranteed.

Principal is due at `failure_at + 604800`. The processor has 3600 seconds after
failure to issue a signed `kind:1034` incomplete claim. Afterwards the buyer may
issue fallback evidence from the quote, proofs, and deterministic failure facts.
Evidence is deduplicated by purchase identity. A late processor claim cannot
invalidate fallback evidence or move the deadline. Each obligation independently
tracks `refund_owed`, `initiated`, `verified`, and `overdue/disputed`, retaining
proof and encrypted destination history.

Commercial terms, split quotes, partial settlement, recovery, and refund evidence do not apply to Entitlement Grants. A server MUST NOT require fee settlement or infer a payment leg from grant issuance.

## Claim-and-keep Campaigns

A claim-and-keep promotion lets an authenticated buyer claim during an active publisher-signed campaign and receive a durable Entitlement Grant. It is represented by a regular, non-replaceable event of `<adp-campaign-kind>` (provisionally `1031` in current implementations; see [Event kinds](#event-kinds)).

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

Every campaign-chain event MUST carry exactly one `d` tag and exactly one `a` tag, preserving the root values. This supports reliable indexed lookup and prevents chain events from changing campaign identity or game association.

#### Root example

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

### Campaign discovery

Clients SHOULD use listing [campaign pointers](#campaign-pointers) as a fast discovery path when available. They MUST still support coordinate-based relay discovery as the complete fallback.

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

Discovery precedence is:

1. listing campaign pointers as a preferred optimization;
2. coordinate-based `#a` queries as the authoritative fallback;
3. campaign validity always determined from the publisher-signed campaign chain.

### Campaign lifecycle

#### Before `starts`

The developer may publish an update that changes campaign terms. The update MUST:

- use the same `d` and `a` values;
- contain exactly one `e` tag referencing the immediately preceding campaign event;
- be signed by the developer key from the `a` coordinate;
- be created before the currently effective `starts` value;
- provide the complete replacement `mode`, `starts`, `ends`, and `status` fields.

#### At or after `starts`

Campaign terms are immutable. The developer may only publish `status: cancelled`.

A post-start event that changes `mode`, `starts`, or `ends` is invalid.

#### Cancellation

A valid cancellation:

- is signed by the developer key;
- preserves `d` and `a`;
- references the immediately preceding campaign event;
- uses `status: cancelled`;
- prevents grants whose root `created_at` is equal to or later than the cancellation event's `created_at`.

Cancellation is prospective. It does not revoke grants issued before cancellation. Individual entitlement revocation requires a separate developer-signed grant status update; see [Grant revocation authority](#grant-revocation-authority).

Cancellation is terminal within the campaign chain. No valid event may succeed a cancellation, whether before or after `starts`; a successor of a cancellation is an invalid transition. A developer wishing to run the promotion again publishes a new campaign root.

#### Campaign chain resolution

Campaign chains use the [valid-prefix model](#valid-prefix-chain-resolution), including timing validity of each transition.

A campaign is claimable at time `T` when:

```text
status_at(T) == active
AND starts <= T
AND T < ends
```

## Entitlement Grants (NIP-103 profile)

[NIP-103](NIP-103.md) (the Entitlement Grant Draft) defines the generic grant event, chain invariants, and Proof of Entitlement. NIP-103 is unchanged by ADP. This section defines only the ADP requirements a grant must additionally satisfy. The grant kind is `<entitlement-grant-kind>` (provisionally `1030` in current implementations; see [Event kinds](#event-kinds)).

### Required tags

A grant accepted by ADP MUST contain exactly one:

- `d` (grant identifier);
- `p`;
- `a`;
- `status`.

A delegated root grant MUST additionally contain exactly one `authorization` tag referencing the immutable `kind:30406` root and exactly one `source_event` tag referencing its issuance policy. A direct developer-signed grant MUST omit `authorization` and MAY omit `source_event`; it MUST NOT contain more than one `source_event`.

Whenever `source_event` is present, it MUST reference the root event ID of a campaign, not a replaceable listing event, and [campaign anchoring](#campaign-anchoring) applies. A campaign is the only issuance policy defined by this version.

### Issuance authority

The root grant MAY be signed directly by the developer key from the game coordinate without an authorization reference.

**Developer-signed grants without a campaign.** A developer-signed root grant without `source_event` is valid on the developer's signature and the NIP-103 chain checks alone. It covers non-campaign uses such as review copies, complimentary copies, contest prizes, and license migrations. No campaign or issuance policy is consulted.

**Delegated grants require an independently verifiable issuance policy.** A delegated root grant is valid only when:

- its signer equals the fulfillment key named by its `authorization` root;
- the root is developer-signed and bound to the same coordinate;
- the authorization was active at the grant's `created_at`;
- the authorization contains the invariant `issue_grant` capability;
- its `source_event` references a developer-signed issuance policy that a verifier can independently check — in this version, a campaign satisfying [campaign anchoring](#campaign-anchoring).

A delegated grant without `source_event`, or whose source is not a valid campaign, is invalid.

Current listing membership is required for new issuance only. Removing the authorization from a later listing does not invalidate an earlier grant. Delegated grants without an authorization anchor are invalid; no legacy fallback is defined.

Fulfillment authorization permits issuance only. It does not authorize campaign control or entitlement revocation.

### Campaign anchoring

For every grant carrying `source_event`, a verifier MUST fetch the campaign chain rooted at `source_event` and verify:

1. The root event exists, has a valid signature, and uses `<adp-campaign-kind>`.
2. The root signer equals the developer pubkey encoded in the grant's `a` coordinate.
3. The campaign `a` equals the grant `a`.
4. The campaign chain is valid and unambiguous.
5. The campaign was active at the root grant's `created_at`.
6. `starts <= grant.created_at < ends`.
7. No valid cancellation had become effective by `grant.created_at`.
8. The root grant signer was the developer or was authorized through the exact anchored `kind:30406` root with `issue_grant` at `grant.created_at`.

A later campaign cancellation does not invalidate an earlier grant.

### Grant revocation authority

Under ADP, only the developer key encoded in the grant's `a` coordinate may publish a valid `status: revoked` update.

A fulfillment key MUST NOT revoke a grant, including a grant it originally issued.

Every revocation event MUST satisfy the NIP-103 chain invariants and preserve the root's `d`, `p`, `a`, `source_event`, and `authorization` values (an absent `source_event` or `authorization` remains absent).

Revocation is terminal within the grant chain. No valid event may succeed a `revoked` status; a successor of a revocation is an invalid transition, and a revoked grant cannot return to `granted`. Restoring entitlement requires a new grant.

Campaign cancellation and grant revocation are distinct operations: cancellation is campaign-wide and prospective, blocking only grants created at or after it; revocation is per-grant and is the only way to end a previously valid grant. A grant has no automatic expiration.

### Portable grant verification

For an authenticated buyer without a local token, a server queries relays for grant events with:

```text
#p = buyer_pubkey
#a = game_coordinate
```

For each candidate grant chain, it applies:

- the NIP-103 chain and Proof-of-Entitlement checks;
- ADP [issuance authority](#issuance-authority), including [campaign anchoring](#campaign-anchoring) whenever `source_event` is present;
- ADP publisher-only [revocation authority](#grant-revocation-authority).

Any one valid chain with a `granted` tip proves entitlement.

## HTTP Endpoints

All endpoints requiring authentication use `Authorization: Nostr <base64-nip98-event>` per [NIP-98](https://github.com/nostr-protocol/nips/blob/master/98.md). The authenticated party's identity is always derived from the token's `pubkey`, never from a request body.

| Method | Path | Auth | Purpose |
|---|---|---|---|
| `GET` | `/.well-known/adp` | None | Server identity and capacity |
| `POST` | `/provision` | NIP-98 (developer) | Provision or return a fulfillment key |
| `POST` | `/provision/revoke` | NIP-98 (developer) | Operator stops using a fulfillment key |
| `POST` | `/upload` | NIP-98 (developer or fulfillment key with `upload_build`) | Upload a build |
| `POST` | `/purchase/confirm` | NIP-98 (buyer) | Exchange payment proof for a receipt and download token |
| `POST` | `/entitlement/claim` | NIP-98 (buyer) | Claim a campaign for a grant and download token |
| `GET` | `/game/:game_coordinate` | NIP-98, download token, or none under public/active timed policy | Download the archive |

The HTTP endpoints of the [split-settlement profile](#experimental-split-settlement-profile-version-1) are not part of this specification.

### HTTP errors

Every ADP endpoint uses the following status codes with the meanings given. A server MUST NOT use a code from this table for a different condition, and MUST NOT report insufficient relay evidence as a definitive failure (`400`, `403`, `404`, `451`).

| Status | Meaning |
|---|---|
| `400 Bad Request` | Malformed request, body, coordinate, event, or chain; invalid transition or anchor; a non-canonical hex value; coordinate mismatch. |
| `401 Unauthorized` | NIP-98 token missing where required, invalid, expired, or not matching the exact URL and method. At `GET /game/:game_coordinate`, also: no valid authentication was presented and the current policy grants no anonymous access. |
| `402 Payment Required` | Payment verification failed. |
| `403 Forbidden` | The authenticated party is not authorized: the uploader lacks authority, the developer does not own the named key, no access basis authorizes a download, or the campaign is not claimable (not started, ended, or cancelled). |
| `404 Not Found` | The listing, campaign root, hosted game, or hosted file is **confirmed absent** — under the server's relay-evidence policy for relay data, or by local records for hosted files. |
| `409 Conflict` | A payment proof's `payment_hash` was already accepted for a different buyer, coordinate, or purchase (reuse; a retry of the original purchase is not a conflict), or local issuance state conflicts in a way that cannot be resolved idempotently. A `409` is never evidence of ownership. |
| `413 Content Too Large` | The uploaded file exceeds the server's upload limit. |
| `451 Unavailable For Legal Reasons` | The server is not in the current listing's `server` tags for the coordinate. |
| `500 Internal Server Error` | The server holds no key currently authorized for the requested signing operation, signing or persistence failed, a stored file failed integrity re-verification, or another internal invariant failed. |
| `503 Service Unavailable` | Required relay evidence is unavailable or insufficient — including an unsatisfied freshness policy, an incomplete required chain, or an inconclusive absence — or relay publication of a required event failed. See [Indeterminate results](#indeterminate-results). |

Successful responses use `200`; `GET /game/:game_coordinate` additionally uses `304 Not Modified`. Each endpoint below lists the codes it can return. This version does not define an error response body schema; the status code is normative.

### NIP-98 `u` tag comparison

A server verifying a NIP-98 token MUST compare the token's `u` tag against a target constructed as:

- **scheme and authority** from the server's own configured external origin;
- **path and query** taken verbatim from the raw request target, exactly as transmitted.

The comparison is exact. A server MUST NOT percent-decode either side before comparing, MUST NOT normalize trailing slashes, and MUST NOT reconstruct the path from decoded route parameters.

Game coordinates contain colons (`30402:<pubkey>:<d-tag>`). Colons are legal unencoded in a path segment per RFC 3986, so percent-encoding them is optional — which means a decoded and an encoded form of the same coordinate are both valid URLs that do not compare equal as strings. Without this rule, a client and a server can each choose a defensible form and never interoperate. Taking the path verbatim from the request removes the choice: whatever the client transmitted is what the client must have signed.

Scheme and authority come from configuration rather than from the request because `Host` and forwarding headers are attacker-controlled. A server MUST NOT derive them from request headers. A deployment behind a reverse proxy MUST configure its external origin to match the origin clients address; a mismatch there is a configuration error and MUST fail closed.

The origin is part of the comparison, not omitted. A `u` tag binding only a path would let an authorization signed for one server be replayed against another serving the same coordinate, within its validity window — a live risk in this protocol precisely because servers are interchangeable and multiple servers deliberately serve the same content.

A token is valid only for the exact URL and method it signs; a token for `/purchase/confirm` MUST NOT be accepted for `/game/:game_coordinate` or any other endpoint.

### `GET /.well-known/adp`

Public. No auth required.

**Response `200`:**
```json
{
  "adp_version": "0.3.0",
  "pubkey": "<server-pubkey>",
  "name": "Arcadestr Official Distribution",
  "url": "https://dist.arcadestr.io"
}
```

The response MAY include `max_upload_bytes` (see [Upload capacity discovery](#upload-capacity-discovery)) and, for servers implementing it, the [split-settlement profile](#experimental-split-settlement-profile-version-1) fields.

### Provisioning Endpoints

These exist so any developer can onboard to any ADP-compliant hosted operator through the same flow, rather than each operator inventing its own bespoke API — the same interoperability goal that motivates every other part of this spec.

#### `POST /provision`

Developer-authenticated. Requests the operator provision (or return an existing) fulfillment key for the calling developer, optionally scoped for per-game key isolation.

**Auth:** NIP-98 token. The developer's identity comes from the token, never the request body.

**Request body (`application/json`):**
```json
{
  "scope": "my-game-slug"
}
```
`scope` is optional; omit it to request a default, unscoped key.

**Server MUST:**
1. Verify the NIP-98 token.
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

**Error responses:**

| Status | Reason |
|---|---|
| `400` | Malformed body or scope |
| `401` | NIP-98 token invalid or expired |
| `500` | Key generation, persistence, or signing failed |
| `503` | Publication of the `kind:30404` attestation failed |

#### `POST /provision/revoke`

Developer-authenticated. Requests the operator stop holding and using a previously provisioned key.

**Auth:** NIP-98 token. The server MUST confirm the calling developer owns the named key per its own records before acting.

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

**Response `200`:**
```json
{
  "fulfillment_pubkey": "<hex>",
  "attestation_event_id": "<hex>",
  "revoked_at": 1234567890
}
```

**Error responses:**

| Status | Reason |
|---|---|
| `400` | Malformed body or non-canonical `fulfillment_pubkey` |
| `401` | NIP-98 token invalid or expired |
| `403` | The calling developer does not own the named key |
| `500` | Persistence or signing failed |
| `503` | Publication of the revoked `kind:30404` failed |

### Upload capacity discovery

`GET /.well-known/adp` MAY include `max_upload_bytes`, a non-negative integer
specifying the maximum bytes in the `file` part of `POST /upload`, excluding
multipart framing and listing metadata. Absence means unknown capacity, not
unlimited capacity. Clients SHOULD check every intended distribution server's
advertised capacity before provisioning or publishing a listing with a new build.
Servers MUST enforce their limit while receiving file bytes and SHOULD return
HTTP `413` when exceeded. Metadata and total request limits may apply separately.
Discovery is advisory: limits may change before an upload completes.

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
2. Retrieve the current listing for the coordinate under its [freshness policy](#freshness-sensitive-operations), and verify direct developer authority, or that the current listing enables an authorization for the signing key whose lifecycle is active with the `upload_build` capability.
3. Verify the uploaded file's SHA-256 matches `file_hash` in `listing_event`, comparing canonical lowercase hex exactly.
4. Store the file indexed by `game_coordinate`.

The server does not validate `package` format or launch metadata; those bind clients.

**Response `200`:**
```json
{
  "game_coordinate": "30402:<developer-pubkey>:<d-tag>",
  "file_hash": "<sha256-hex>",
  "download_url": "https://dist.arcadestr.io/game/30402:<developer-pubkey>:<d-tag>"
}
```

**Error responses:**

| Status | Reason |
|---|---|
| `400` | Malformed multipart body, invalid listing event or signature, non-canonical `file_hash`, or file SHA-256 mismatch |
| `401` | NIP-98 token invalid or expired |
| `403` | Signer is neither the developer nor a fulfillment key under a current, active `upload_build` authorization |
| `404` | Listing confirmed absent from relays |
| `413` | File exceeds the server's upload limit (SHOULD) |
| `500` | Storage or persistence failed |
| `503` | Current listing or authorization evidence insufficient under the freshness policy |

### `POST /purchase/confirm`

Buyer-authenticated. Submits payment proof; receives a signed NIP-102 receipt and a download token.

**Auth:** NIP-98 token. The buyer's identity is derived from the token's `pubkey`; no buyer identity fields are accepted from the request body.

**Request body (`application/json`):**
```json
{
  "game_coordinate": "30402:<developer-pubkey>:<d-tag>",
  "listing_event": { "...": "signed kind 30402 event" },
  "zap_receipt_event": { "...": "signed kind 9735 event" }
}
```

`zap_receipt_event` MAY be replaced with `bolt11` + `preimage` fields, per NIP-102's payment-proof flexibility. The [split-settlement profile](#experimental-split-settlement-profile-version-1) defines additional version-1 verification.

**Server MUST verify:**

1. NIP-98 token: `kind = 27235`, `u` tag per [NIP-98 `u` tag comparison](#nip-98-u-tag-comparison), correct `method` tag, timestamp within 60 seconds of server clock.
2. `listing_event.sig` is valid.
3. `listing_event.pubkey` is the expected developer for the claimed `game_coordinate`.
4. The server signs directly as developer, or the current listing — retrieved under the server's [freshness policy](#freshness-sensitive-operations) — references an active authorization for its fulfillment key containing `issue_receipt`.
5. Payment proof is valid: `zap_receipt_event.sig` valid, buyer `P` tag matches the NIP-98 token pubkey, an `a`/`e` tag references the claimed `game_coordinate` with a matching amount, **and** `zap_receipt_event.pubkey` matches the LSP resolved from the listing's `lud16` tag per [LSP Verification](#lsp-verification-lud16) — or the `bolt11`/`preimage` pair validates per NIP-102.
6. The payment's canonical `payment_hash` has not been accepted before, through any proof format, enforced atomically per [Replay protection](#replay-protection). A submission that repeats the original purchase of an accepted `payment_hash` is a retry. The server answers it with the original receipt per [Replay protection](#replay-protection) instead of the steps below. Any other reuse is `409`.

**Server MUST on success:**

1. Generate an `order` id (UUID v4).
2. Construct a `kind:1020` root receipt including the NIP-102 public payment binding and, for delegated signing, `authorization` pointing to the exact `kind:30406` root.
3. Encrypt the itemized order JSON to the buyer's pubkey per [NIP-44](https://github.com/nostr-protocol/nips/blob/master/44.md) and set it as `.content`.
4. Sign with the fulfillment key (or developer key).
5. Atomically persist `(order_id, payment_hash, buyer_pubkey, game_coordinate)` with the signed receipt, **before** publication. Use a unique constraint on `order_id`, satisfying NIP-102's idempotency requirement, and a unique constraint on `payment_hash`, satisfying [Replay protection](#replay-protection). Implementations MAY additionally store the proof's event ID or other identifiers.
6. Publish the receipt to relays. If publication fails, the purchase remains accepted, and a retry returns this same receipt.
7. Issue a short-lived download token scoped to the buyer and `game_coordinate`.

**Response `200`:**
```json
{
  "receipt": { "...": "signed kind 1020 event" },
  "download_token": "<opaque-token>",
  "token_expires_at": 1234567890
}
```

A retry returns the original receipt with the same `200` shape. When a retry issues no fresh token, `download_token` and `token_expires_at` are omitted (see [Replay protection](#replay-protection)).

**Error responses:**

| Status | Reason |
|---|---|
| `400` | Malformed request body, invalid listing event, or coordinate mismatch |
| `401` | NIP-98 token invalid or expired |
| `402` | Payment verification failed (including: zap receipt not signed by the listing's resolved LSP, or listing has no `lud16` and only a zap receipt was presented) |
| `404` | Game not hosted on this server, or listing confirmed absent |
| `409` | Payment `payment_hash` already accepted for a different buyer, `game_coordinate`, or purchase (reuse). A retry of the original purchase is not a conflict. A `409` is not evidence of ownership. |
| `500` | Server's fulfillment key is not currently authorized for this listing, or signing/persistence failed |
| `503` | Current listing or authorization evidence insufficient under the freshness policy, or receipt publication failed. After a publication failure, the purchase remains accepted, and a retry returns the same receipt. |

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

**Server MUST verify:**

1. NIP-98 token kind, signature, `u` (per [NIP-98 `u` tag comparison](#nip-98-u-tag-comparison)), method, and 60-second clock bound.
2. The current listing for `game_coordinate`, retrieved under the server's [freshness policy](#freshness-sensitive-operations) and selected by NIP-01 replaceable-event ordering. If the policy cannot be satisfied, the server MUST fail with `503` rather than issue a grant.
3. The server is currently authorized in the listing's `server` tags and hosts the requested game.
4. The campaign root and complete campaign chain are retrievable and valid.
5. The campaign developer equals the listing developer and coordinate developer.
6. The campaign is active at current server time using `starts <= now < ends`.
7. No cancellation is effective at current server time.
8. The server holds the developer key, or the current listing enables an active authorization for its signing key with `issue_grant` at current server time.
9. Local idempotency for `(buyer_pubkey, game_coordinate, campaign_id)`.

A campaign does not need to be referenced by the current replaceable listing. Listing pointers are optional discovery hints only. The server MUST resolve the supplied root event and independently fetch and validate the complete publisher-signed campaign chain.

#### Idempotency

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

#### Success behavior

The server MUST:

1. Generate a unique grant ID.
2. Construct the root Entitlement Grant with:
   - recipient `p`;
   - game coordinate `a`;
   - campaign root ID in `source_event`;
   - for delegated signing, the exact authorization root in `authorization`;
   - `reason: promotional-claim`;
   - `status: granted`.
3. Sign with the developer key or an authorized fulfillment key.
4. Publish the grant to relays.
5. Persist the idempotency record.
6. Issue a short-lived download token using the same ADP token mechanism as `/purchase/confirm`.

**Response `200`:**

```jsonc
{
  "grant": { "...": "signed entitlement grant event" },
  "download_token": "<opaque-token>",
  "token_expires_at": 1234567890
}
```

#### Errors

| Status | Reason |
|---|---|
| `400` | Malformed request, invalid campaign chain, or coordinate mismatch. |
| `401` | Invalid or expired NIP-98 token. |
| `403` | Campaign not started, ended, or cancelled. |
| `404` | Campaign root or listing confirmed absent, or game not hosted. |
| `409` | Conflicting local issuance state that cannot be resolved idempotently. |
| `451` | Server is not currently authorized to distribute the game. |
| `500` | No server-held key is currently authorized to issue the grant, or signing/persistence failed. |
| `503` | Current listing, campaign chain, or authorization evidence insufficient, or grant publication failed. |

### `GET /game/:game_coordinate`

Streams the game archive. Requires a distribution-authorization check and an access check to pass independently.

**Auth:** one of:

- `?token=<download_token>` from `/purchase/confirm` or `/entitlement/claim`;
- a NIP-98 token identifying the buyer, required to evaluate durable credentials;
- none, when the current listing's public or active timed-access policy authorizes the download. A server MUST NOT require authentication for such a request.

A request carrying an invalid or expired NIP-98 token MUST be rejected with `401`; it is not downgraded to an anonymous request.

#### Current listing checks

The server first fetches the current listing for current distribution and file policy, under its [freshness policy](#freshness-sensitive-operations) for validating current distribution authorization. It MUST verify current:

- listing signature and coordinate;
- server distribution authorization (see [Distribution authorization](#distribution-authorization));
- file hash and hosted file integrity;
- file/version metadata used for the response.

A server not listed in the current listing's `server` tags MUST NOT serve the file, regardless of the access basis (`451`).

#### Access evaluation

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

Steps 2 and 3 apply only when a valid NIP-98 token identifies the buyer. An anonymous request proceeds directly from step 1 to step 4. Steps 4 and 5 authorize both anonymous and authenticated requests, and apply even when credential queries were unavailable.

A denial at step 6 returns:

- `401` when the request presented neither a valid download token nor a valid NIP-98 token;
- `503` when an authenticated request's grant or receipt evidence was unavailable or insufficient and no other basis authorized access;
- `403` otherwise.

Distribution authorization and file-hash verification remain mandatory in every branch.

- **Download token (step 1):** valid, unexpired, issued for this `game_coordinate` to this buyer by this server.
- **Portable grant (step 2):** per [Portable grant verification](#portable-grant-verification).
- **Portable receipt (step 3):** query `kind:1020` receipts by `#p` (buyer) and `#a` (coordinate), resolve the receipt chain per NIP-102, require a tip status that [retains ownership under ADP](#receipt-status-under-adp), and verify either direct developer signing or the immutable `authorization` root at the receipt root's `created_at`. Current listing membership is not required for historical credential validity.

Steps 2 and 3 require no relationship between the issuing server and the serving server — the credential and its delegation are independently verifiable by anyone.

**Response `200`:** binary game archive stream.
**Response `304`:** not modified (conditional GET SHOULD be supported).

**Error responses:**

| Status | Reason |
|---|---|
| `400` | Malformed game coordinate |
| `401` | Invalid or expired NIP-98 token; or no valid authentication presented and no public/active timed policy applies |
| `403` | Authenticated, but no valid token, durable credential, or applicable public/timed access policy |
| `404` | Listing confirmed absent, or file not hosted on this server |
| `451` | Server not authorized to distribute this file |
| `500` | Stored file failed integrity re-verification against `file_hash` |
| `503` | Current listing evidence insufficient under the freshness policy, or required credential evidence unavailable (see above) |

## Flows

### Provisioning Flow

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
8. Verify matching kind:30404 (operator, developer, key)
9. Construct + sign coordinate-scoped kind:30406 root
   with explicit capabilities
10. Publish authorization root ──────────────────────────────────────────→
11. Add fulfillment_authorization tag
    (root id, fulfillment pubkey, relay hint)
    to kind:30402 listing, sign, publish ───────────────────────────────→
```

### Purchase Flow

```
Buyer Client                    ADP Server                  Nostr Relays
─────────────────────────────────────────────────────────────────────────
1. Pay Lightning invoice
2. Poll for kind:9735 zap receipt                        ← kind 9735 appears
3. Construct NIP-98 token
4. POST /purchase/confirm ──────────────────────────→
                               5. Verify NIP-98, listing, payment proof
                               6. Verify current listing enablement and
                                  active issue_receipt authorization
                               7. Build + NIP-44 encrypt kind:1020 receipt
                                  (with authorization root if delegated)
                               8. Sign with fulfillment key
                               9. Publish ──────────────────────────────→
                               10. Return receipt + download_token ────→
11. GET /game/:coordinate ──────────────────────────→
    ?token=<download_token>
                               12. Verify current listing + server tags
                                   (distribution auth)
                               13. Verify token (access step 1)
                               14. Stream archive ──────────────────────→
15. Verify file_hash, save to disk
```

### Claim Flow

```
Buyer Client                    ADP Server                  Nostr Relays
─────────────────────────────────────────────────────────────────────────
1. Discover campaign (listing pointer, else #a query) ←────────────────
2. Construct NIP-98 token
3. POST /entitlement/claim ─────────────────────────→
   { game_coordinate, campaign_event_id }
                               4. Verify NIP-98, current listing,
                                  server tags, hosted game
                               5. Fetch + validate campaign chain ────→
                                                                     ←──
                               6. Check claimable now, no cancellation
                               7. Check idempotency record
                               8. Verify current listing enablement and
                                  active issue_grant authorization
                               9. Build + sign grant root
                                  (source_event = campaign root)
                               10. Publish ─────────────────────────────→
                               11. Persist idempotency record
                               12. Return grant + download_token ──────→
13. GET /game/:coordinate?token=<download_token> (as in Purchase Flow)
```

### Portable Download Flow (different server, no prior relationship)

```
Buyer Client                    New ADP Server              Nostr Relays
─────────────────────────────────────────────────────────────────────────
1. Construct NIP-98 token
2. GET /game/:coordinate ──────────────────────────→
   Authorization: Nostr <token>
                               3. Fetch current listing; verify server
                                  URL in server tags (distribution auth)
                               4. No download token; query grants and
                                  kind:1020 receipts by #p + #a ────────→
                                                                      ←──
                               5. Fetch campaign chains (grants) and
                                  authorization roots + complete
                                  lifecycles (delegated credentials) ──→
                                                                      ←──
                               6. Verify each credential at its root's
                                  created_at; else apply current public
                                  or timed-access policy
                               7. Stream archive ───────────────────────→
8. Verify file_hash, save to disk
```

### Anonymous Public or Timed Download Flow

```
Client                          ADP Server                  Nostr Relays
─────────────────────────────────────────────────────────────────────────
1. GET /game/:coordinate ───────────────────────────→
   (no Authorization header, no token)
                               2. Fetch current listing under freshness
                                  policy ─────────────────────────────→
                                                                      ←──
                               3. Verify server URL in server tags
                                  (distribution auth)
                               4. Policy is public, or timed-access with
                                  starts <= now < ends -> allow
                                  (otherwise 401)
                               5. Stream archive ───────────────────────→
6. Verify file_hash, save to disk
```

No durable credential is created or checked. A client holding a durable credential can instead authenticate with NIP-98, so that its access does not depend on the current policy.

## Client Installation and Execution

The requirements in this section bind clients that install and launch packages. They have no server surface.

### Path rules

These rules apply to `executable` and `working-dir`, and independently to every entry name inside a package during extraction. They are the primary security surface of package handling: a verified hash proves the archive is the one the developer signed, not that its contents are safe to write to disk.

This version deliberately defines a **portable package-path namespace** rather than target-specific filename rules. The same path is valid or invalid regardless of the platform on which the publisher creates the archive, the platform on which a client validates it, or which supported target the listing declares. This is a v1 interoperability restriction: it sacrifices some filenames that are legal on Unix filesystems in exchange for deterministic validation, simpler tooling, and the ability to move the same package metadata across targets without reinterpretation. A later revision MAY define target-specific package-path rules only if validity remains deterministic from signed artifact metadata and independent of the publisher's or consuming client's host platform. Packages published under this version remain subject to this portable namespace permanently.

A path MUST be rejected if it:

1. is absolute, or begins with `/`;
2. contains a `..` segment, before or after any normalization;
3. contains a backslash (`\`);
4. contains a colon (`:`) or otherwise encodes a drive or device prefix;
5. contains a NUL or any C0 control character;
6. contains an empty segment (`a//b`) or a `.` segment other than a `working-dir` consisting solely of `.`;
7. has any segment ending in a space or a period;
8. has any segment whose name, ignoring case and any extension, is one of `CON`, `PRN`, `AUX`, `NUL`, `COM0`–`COM9`, `LPT0`–`LPT9`;
9. exceeds 200 characters, or has any segment exceeding 100 characters.

Rules 3, 4, 7, and 8 are part of the portable namespace rather than host-specific checks. A backslash is a legal filename character on Linux and a separator on Windows, so `a\..\..\escape` has platform-dependent meaning; reserved device names and trailing dots are likewise legal in some supported filesystems and unrepresentable in others. A conforming client MUST apply the same rules regardless of its host platform or the listing's declared target.

Rule 9 is a Windows `MAX_PATH` mitigation and is not sufficient alone: clients MUST additionally keep the installation root short. Deriving an installation directory name from a game coordinate is non-conforming, since coordinates are long. Clients SHOULD use a short opaque identifier and MUST enable long-path support where the platform offers it.

Path comparison for duplicate detection MUST be case-insensitive. `Game.exe` and `game.exe` coexist on ext4 and collide on NTFS.

### Argument rules

Arguments MUST be a list of separate strings. A single string that is split by the client, or passed to a shell for word-splitting, is non-conforming.

- The array MUST contain at most 32 elements.
- Each element MUST be at most 1024 characters.
- Elements MUST NOT contain NUL.
- No element is interpreted, expanded, or substituted by the client. There is no variable syntax, no `%command%` equivalent, and no path expansion.

There is deliberately no field for a command line, a shell invocation, or publisher-supplied environment variables. Each would convert a publisher-signed metadata field into arbitrary command execution with a broader reach than the executable itself already has.

### Extraction requirements

A client MUST NOT begin extraction before the package's SHA-256 has been verified against the listing's `file_hash`.

During extraction the client MUST:

1. Apply the [Path rules](#path-rules) to every entry name and abort on the first violation. Abort is per-package, not per-entry: a package containing one hostile entry is rejected in full.
2. Reject symbolic links and hard links. A conforming implementation does not create links during extraction. (This is a deliberate v1 restriction; safely resolving link targets against an escape check is a later extension.)
3. Reject any entry that is not a regular file or a directory.
4. Enforce decompression limits and abort **mid-stream** when either is exceeded, rather than checking after extraction completes:

   | Limit | Value |
   |---|---|
   | Total entries | 200,000 |
   | Cumulative compression ratio | 200:1, evaluated only after 64 MiB has been decompressed |

   Clients MAY enforce tighter limits and MUST NOT enforce looser ones. The ratio limit is deferred until at least 64 MiB of regular-file payload has actually been emitted because small, highly compressible files are ordinary and would otherwise trip it.

   The **cumulative compression ratio** is:

   `regular-file bytes actually emitted / compressed bytes actually consumed`

   The numerator is the cumulative number of bytes emitted for regular-file contents. The denominator is measured from the compressed input actually consumed by the decompressor or decompressors up to the same point. For ZIP, the denominator is the sum of compressed entry-body bytes consumed for processed regular-file entries. For `tar.gz` and `tar.zst`, it is the compressed stream bytes consumed. Container metadata that the format makes inseparable from the compressed stream MAY contribute to the denominator; this only lowers the measured ratio. Declared uncompressed-size fields MUST NOT substitute for measured output counts. The limit MUST be checked continuously after the 64 MiB numerator threshold is reached, not only after an entry or archive completes.

   These two limits describe disproportion, which is what distinguishes a decompression attack from a large game. This specification deliberately sets **no maximum extracted size**. An absolute constant is a poor proxy for a quantity the client can measure exactly, and a protocol-level ceiling on how large a game may be would eventually make a legitimate title uninstallable. Bounding extraction against available disk space is a client responsibility and is not specified here, since no other party observes it.
5. Extract into a temporary directory adjacent to the final installation root, then move it into place as a single operation. A client MUST NOT extract directly into the final root. On platforms where directory rename over an existing target fails, the client MUST use a replace-capable operation or a delete-then-rename with a documented recovery step for interruption between the two.
6. Preserve entry paths exactly. Clients MUST NOT strip a leading directory component. An executable path is relative to the installation root, which corresponds exactly to the package's internal structure.
7. On Unix hosts, preserve execute bits supplied by the package for regular files, and apply the execute bit to the file named by each profile's `executable` when the package format did not carry mode bits, or when it carried them and the bit is absent. Clients MUST NOT synthesize execute bits for the tree at large. ZIP writers frequently discard Unix mode, so a Linux build shipped as a ZIP will otherwise not run.

After extraction and before recording the installation, the client MUST verify that every profile's `executable` resolves to an existing regular file within the installation root, and that every profile's `working-dir` resolves to an existing directory within it. A profile failing this check MUST be recorded as unavailable and MUST NOT be offered. Clients SHOULD surface unavailable profiles and the reason each profile is unavailable rather than silently omitting them. A listing whose profiles all fail MUST be surfaced as installed-but-not-launchable rather than as a successful install.

Resolution MUST be performed against the real path after symlink resolution of the installation root itself, and a target resolving outside that root MUST be rejected.

### Execution requirements

On Unix, Arcadestr accepts native ELF executables and executable interpreter scripts beginning with `#!`. ELF / `#!` detection is a launch-policy whitelist, not a security substitute for direct execution semantics. Arcadestr invokes the configured absolute launch target using direct `execve` semantics. Interpreter scripts are resolved by the operating system. Arcadestr does not perform PATH lookup for the launch target, parse shebangs, select a shell, reparse arguments, or retry `ENOEXEC` failures through a shell.

The operating system may run an otherwise foreign executable through a host-registered `binfmt_misc` handler or an equivalent kernel facility. Arcadestr does not register, select, or override those handlers. Launch validation and execution are both path-based; replacement between validation and `execve` is an accepted TOCTOU under the current same-UID trust model.

A client launching a profile MUST:

1. Execute the file directly. It MUST NOT interpose `cmd.exe`, PowerShell, `/bin/sh`, or any other client-selected interpreter, and MUST NOT construct a command string that any layer will re-parse. An interpreter selected by the Unix kernel from a launch target's shebang is not client interposition.
2. Pass `arguments` as distinct argument vector elements.
3. Set the process working directory to the resolved `working-dir`.
4. Not request or apply privilege elevation.
5. Not pass publisher-controlled environment variables. The child environment is the client's own, minus any variable holding client-internal secrets, credentials, signer state, or key material. Clients SHOULD construct the child environment from an explicit allowlist rather than by filtering an inherited one.

Launching MUST NOT require contacting a relay or an ADP server. See [Execution is DRM-free](#execution-is-drm-free).

### Installation records

At install time the client MUST persist a resolved snapshot containing at minimum:

- the game coordinate and the installation root;
- the package digest actually verified;
- the **resolved** package format, operating system, and architecture;
- the full set of launch profiles as resolved, including which were found unavailable;
- the resolved publisher-default profile ID, when the signed listing supplied a valid one;
- **a per-entry manifest** of every regular file and directory extracted, recording its path relative to the installation root and its entry type; every regular-file entry additionally records its SHA-256;
- **the complete raw signed `kind:30402` event JSON** from which the profiles were resolved.

The per-entry manifest is a client-side structure with no protocol surface — it is not published, not signed, and no server sees it. It is required because it distinguishes paths and entry types the package placed from paths the buyer created, which is the basis of [Updates](#updates), and because regular-file digests make verifying an installation possible without re-downloading. Empty package-created directories are recorded as directory entries. Regular-file digests are computed from bytes the client is already streaming during extraction.

The resolved values are recorded explicitly rather than left to be re-derived from the raw event. The raw event is evidence; the snapshot is the decision. Re-deriving would let a future change to normalization or parsing reinterpret an existing installation, which is exactly what pinning exists to prevent.

The raw event is required, not its ID. `kind:30402` is a parameterized replaceable event; relays are not obliged to retain superseded revisions, so an event ID recorded at install time is generally not resolvable afterwards. Storing the event itself is what allows the publisher's signature over the launch metadata to be re-verified offline and indefinitely.

The raw event establishes provenance of the distribution and launch metadata. It is not by itself proof of the buyer's ownership: acquisition credentials are NIP-102 receipts and NIP-103 grants, held separately, and an installation record is not evidence of durable entitlement.

Launching MUST use the pinned snapshot. A client MUST NOT re-resolve launch configuration from the current listing at launch time. A later listing revision MUST NOT alter how an existing installation starts; it takes effect only when the buyer explicitly updates or reinstalls.

### Updates

An update is available when the current listing's `file_hash` differs from the installed snapshot's package digest. That comparison is the entire detection rule.

The `version` tag MUST NOT be used for this. It is a free-form publisher string — `1.2.0`, `Build 447`, `2026-08-01` — and no ordering can be derived from it reliably. `kind:30402` is replaceable, so the current listing is by definition the current build; a client syncs to what the developer publishes now rather than climbing a version sequence. A publisher who republishes an earlier build produces an update that installs that earlier build, which is correct. Clients SHOULD name the action with the target version so this does not appear to be a defect.

Per [Ownership scope across listing versions](#ownership-scope-across-listing-versions), a durable credential for the coordinate covers the updated build.

Updating MUST be explicit. A client MUST NOT update an installation without the buyer initiating it.

#### Update state vocabulary

Two update failure states are distinct and normative:

- **`update-conflict`** — reconciliation has not begun mutating the installation because preflight or tree planning found a conflict that cannot be resolved without risking buyer-created data. The existing installation remains intact and MAY remain launchable.
- **`requires-repair`** — reconciliation began mutating the installation and was interrupted or failed before completion. The installation MUST NOT be presented as launchable until repair completes.

A UI MAY group either state under a broader presentation category such as "Needs attention", but that label is not a protocol state and MUST NOT replace the distinction above in persisted or interoperable state.

`update-conflict` persists until the buyer resolves it. A client MUST NOT resolve it automatically, and MUST NOT clear it by any action that discards untracked data without the buyer's explicit, specific confirmation. Automatic resolution is precisely the data loss the conflict check exists to prevent, and a client that resolves silently is worse than one that never checked, since the buyer is given no opportunity to intervene.

Conforming resolutions are: the buyer relocates or removes the untracked data and retries the update; the buyer explicitly confirms that the untracked data at the conflicting path may be discarded, after which reconciliation proceeds; or the publisher republishes with a layout that no longer conflicts, after which a retry finds none. A client MUST offer at least the first. The installation remains at its current version until one of these occurs, which is the correct outcome: a buyer stalled on an older build has lost nothing, while a buyer whose saves were deleted cannot be made whole.

#### Tree reconciliation

A client MUST update by reconciling the installed tree against the new package, not by deleting and recreating the installation root. Before the first mutation, the client MUST construct the reconciliation plan from the new package and the recorded per-entry manifest and perform the type-conflict checks defined below.

For regular-file paths whose entry type does not change:

| Condition | Action |
|---|---|
| In both, digest differs | Overwrite |
| In both, digest identical | Leave untouched |
| In new package only | Create |
| In manifest only | Delete |
| In neither — present on disk, absent from both | **Leave untouched** |

Package-created directories in the new manifest are created as needed. Package-created directories absent from the new manifest MAY be removed after their package-owned descendants have been reconciled, but only when empty.

Where a path's type changes between revisions:

- **file → directory:** the old package-owned file is removed before the directory is created.
- **directory → file:** the client MUST inspect the on-disk directory before mutation. If any descendant exists that was not recorded in the old per-entry manifest, replacing the directory would destroy buyer-created data. The client MUST abort the update before making any tree changes, enter `update-conflict`, and leave the existing installation intact. If no untracked descendant exists, the old package-owned descendants and directory may be removed and the new file created.

A client MUST NOT recursively delete an old package-created directory merely because the new package places a file at the same path.

Reconciliation compares the previous **package manifest** with the new package manifest; it does not use current on-disk file contents as the change detector. Consequently, a buyer modification to a package-owned regular file survives when the publisher leaves that package entry unchanged, and may be overwritten when the publisher changes that entry. Likewise, a missing or corrupted package-owned file whose manifest digest is unchanged is not repaired merely because an update occurs. Detecting and restoring such on-disk divergence would be the responsibility of a verification and repair operation, which this version does not require; it is in no case the responsibility of ordinary update reconciliation.

The last row of the regular-file table and the directory-to-file conflict rule are the point of the mechanism. Saves, configuration, and mods written by the game inside its own installation directory are absent from the manifest because the package never placed them, so reconciliation must not silently select them for deletion. Wholesale replacement of the installation root destroys this data silently and irrecoverably, and a meaningful fraction of independent builds store it in-tree.

Directories left empty after deletions SHOULD be pruned. The new per-entry manifest replaces the old one only after the complete reconciliation succeeds.

A client retaining downloaded packages MUST replace the retained package with the new one on a successful update, rather than accumulating one per revision. Only the package corresponding to the current installation is useful: earlier packages cannot reproduce the installed tree, and retaining them turns every update into permanent disk growth.

#### Interruption

Reconciliation modifies the installation root in place and therefore has no rollback point. A client MUST record the installation in the `requires-repair` state when reconciliation is interrupted, and MUST NOT present it as launchable until repair completes.

Recovery is re-running the same reconciliation, which is idempotent: it is a comparison against a manifest, not a sequence of steps, so a partially-applied update converges on re-execution. No journal or resume state is required.

Recovery requires the new package. A client MUST keep the newly downloaded package until reconciliation has either completed successfully or the installation no longer depends on it for repair. If reconciliation is interrupted, the client MUST NOT discard that package merely because it was not yet promoted to the retained-current-package slot. On success, the new package becomes the current retained package when retention is enabled, and the previous retained package may then be removed.

Clients that do not retain packages after successful installation may still treat the new package as temporary once repair is no longer required; if it is later needed again, it must be re-downloaded.

#### Preflight

Before reconciliation begins, a client MUST verify that no process from that installation is running. Replacing files belonging to a running process fails on Windows and produces undefined behavior in the running process elsewhere.

Free-space estimation and other local resource policy are implementation concerns rather than interoperability requirements. A client MAY preflight available space, but this specification does not define a required estimate. Resource exhaustion MUST NOT cause an installation to be reported as successfully updated; if mutation has begun, the installation enters `requires-repair` as defined above.

#### Profile re-pinning

After a successful update the client pins the new listing's profile set and publisher default. If the buyer's saved `profile-id` is still an available `game` profile, it remains the effective default. Otherwise the client uses the available publisher default, then the first available profile of type `game`, and informs the buyer that a fallback occurred. A temporarily unavailable buyer preference MAY be retained so it becomes effective again after repair or reinstall. This is the case the `profile-id` stability rule exists to make rare; it cannot make it impossible.

### Uninstallation

Uninstalling removes the installation root. Whether the downloaded package is also deleted MUST be the buyer's choice, presented at the point of uninstallation with both sizes shown.

The default MUST be to retain the package. The buyer acquired those bytes; execution is DRM-free; the archive is the durable artifact, and discarding it by default would contradict the ownership model the rest of this protocol exists to support. Retention additionally makes reinstallation and [update repair](#interruption) work without network access.

A client retaining packages MUST provide a view listing retained packages with their sizes and an individual delete action, so retained data is visible and manageable rather than accumulating invisibly. A client SHOULD NOT evict retained packages automatically.

### Execution is DRM-free

Once installed, a game launches. This is a protocol-level guarantee, not an implementation convenience.

- Launching MUST NOT require network access, relay reachability, or ADP server reachability.
- Launching MUST NOT be conditioned on re-verifying ownership.
- Revocation of a NIP-102 receipt or a NIP-103 entitlement grant is a commerce-layer fact. It affects future acquisition, future downloads, and displayed ownership state. It MUST NOT disable an existing installation.

The same holds for installations obtained under public or timed access: the end of the timed interval or removal of public policy affects future downloads only.

This follows directly from ADP's trust model. A credential that is portable across servers and verifiable without any operator would be worth little if execution nonetheless depended on an operator remaining reachable and willing.

Clients MUST NOT present installation as evidence of durable ownership, and MUST NOT present revocation as removing access to installed files.

### Decision record: Mark-of-the-Web

On Windows, files written by a browser or by Explorer's ZIP handler carry a `Zone.Identifier` alternate data stream. SmartScreen consults it, and an unsigned executable so marked produces a reputation warning before it runs.

A client extracting a package with its own code produces no such stream. Extracted executables therefore launch without a SmartScreen prompt.

**This specification does not require clients to apply Mark-of-the-Web to extracted files.** The reasoning is that virtually all independent game builds are unsigned, marking would produce a warning on essentially every launch, and a warning that always fires trains buyers to dismiss it — degrading the signal without adding protection.

This trade should be stated precisely. Declining to mark **removes one operating-system reputation check**, and nothing in this protocol replaces it. The developer signature over `file_hash` is a different kind of claim: it establishes that these are exactly the bytes the publisher signed, which is provenance, not safety. A correctly-signed package from a compromised or malicious publisher passes verification unchanged. SmartScreen answers a question this protocol does not ask, and the decision here is to accept the loss of that answer in exchange for warnings that mean something when they do appear.

This is recorded as a deliberate decision rather than left as an implementation accident, because the alternative is roughly one line of code and would otherwise be adopted or dropped inconsistently. Clients MAY offer marking as a user-controlled setting. A client that marks extracted files MUST do so uniformly and MUST NOT vary by publisher.

## Forward compatibility

A subsequent revision will introduce multiple platform artifacts under one coordinate and an immutable release manifest event. The vocabulary here is chosen so that migration is a remapping:

| This version (`kind:30402` tags) | Future release manifest |
|---|---|
| `file_hash` | per-artifact `digest` |
| `package` | per-artifact `format` |
| `platform` (fused `<os>-<arch>`) | per-artifact `os` / `arch`, as separate lists |
| `launch` tags | per-artifact `launch` array, same fields |
| target-scoped `launch_default` | per-artifact `default_profile_id` |
| `version` | manifest `version` |
| — | manifest `channel` |
| — | per-artifact `sources` |

Two properties do not survive migration unchanged and are called out here so implementations do not depend on them:

- **Eligibility moves from the listing to the artifact,** and profiles gain the ability to narrow it further. Implementations SHOULD keep the selection predicate in a distinct code path from the execution parameters even though this version gives it only one input.
- **The fused `platform` form is not carried forward.** The split form is what the manifest uses. Clients SHOULD parse `platform` into separate OS and architecture values internally rather than matching the fused string.

Launch profiles as defined here are protocol-neutral: nothing in [Extraction requirements](#extraction-requirements), [Execution requirements](#execution-requirements), or [Installation records](#installation-records) refers to how the profile was encoded. A client structured accordingly replaces its resolver when the manifest arrives and changes nothing else.

## Security Considerations

### Distribution, credentials, and authority

- **Replay attacks:** servers MUST store the canonical `payment_hash` of every accepted payment and atomically reject reuse by a different buyer, for a different coordinate, or for a conflicting purchase, through any proof format. A retry by the original buyer returns the original receipt and never a second one; see [Replay protection](#replay-protection).
- **Authenticity, availability, and freshness are separate:** a valid signature proves authorship, retrieval proves existence, and neither proves that no newer publisher-signed event exists elsewhere. Freshness-sensitive operations fail closed with `503` when their documented freshness policy cannot be satisfied; see [Relay availability](#relay-availability).
- **Anonymous public downloads:** public and active timed policy serve anonymous requests, so a server cannot attribute such downloads to a buyer. This exposes nothing beyond what the policy already grants; distribution authorization and file integrity still apply. An invalid NIP-98 token is rejected, never treated as anonymous.
- **Exact activation:** no clock-skew allowance may activate a `kind:30406` authorization or `kind:30404` attestation before `valid_from`.
- **Terminal states:** `kind:30406` cancellation, campaign cancellation, and grant revocation are terminal; a later event cannot reactivate them.
- **Receipt forgery:** a portable receipt is an authorized merchant attestation. Public proofs must validate, encrypted BOLT-11/preimage evidence must match the signed payment binding when selectively disclosed, and buyer assertions alone are insufficient.
- **Fulfillment key custody:** every fulfillment key MUST be distinct per developer — an operator serving multiple developers MUST NOT let two developers share a key, since compromising it would compromise every developer behind it at once. Keys must remain online to support non-interactive fulfillment; each key's authority is limited to the explicit capabilities its developer granted in coordinate-scoped `kind:30406` roots, so compromise doesn't expose that developer's listing-publishing, campaign, revocation, or social identity authority, and is recoverable prospectively through `kind:30406` cancellation.
- **Multi-tenant key storage:** an operator holding many developers' fulfillment keys MUST encrypt them at rest (e.g. AES-256-GCM under a server-wide master key, at minimum) — this store is a considerably higher-value target than any single self-hosted key, since its compromise cascades across every developer the operator serves. An operator running at meaningful scale SHOULD evaluate an HSM or KMS rather than relying on a bare master-key secret.
- **Publisher policy authority:** only the developer key may create, update, or cancel campaigns and revoke buyer grants.
- **Scoped fulfillment capabilities:** delegated keys may perform only explicitly authorized `issue_receipt`, `issue_grant`, or `upload_build` operations; they cannot alter campaign policy or revoke entitlements.
- **Prospective cancellation:** `kind:30406` cancellation is effective at the cancellation event timestamp and cannot invalidate earlier credentials. Campaign cancellation blocks new grants from its timestamp and does not invalidate prior grants.
- **Grant revocation is explicit:** prior grants remain valid until individually revoked by the developer through the grant chain.
- **Authorization completeness:** invalid chains and valid forks fail closed; insufficient relay coverage is indeterminate rather than proof of invalidity.
- **Forks fail closed:** ambiguous authorization, campaign, receipt, or grant chains are invalid.
- **Campaign immutability after start:** terms cannot be edited once the campaign begins; only cancellation is allowed.
- **Advisory listing pointers:** listing `campaign` tags improve discovery but never establish campaign validity or status.
- **Deliberate two-kind model:** campaign policy and buyer entitlement remain separate despite the additional allocation and implementation cost.
- **Server-local idempotency:** independent servers may issue redundant but valid grants.
- **No implicit public access:** zero or malformed price data never bypasses ownership checks.
- **Durable proof precedence:** current listing mode cannot erase previously acquired rights.
- **Timed access is non-durable:** servers MUST NOT infer access after `ends`.
- **Distribution remains independent:** public access and ownership never authorize an unlisted server to distribute the file.
- **NIP-98 token scope:** the `u` tag must exactly match the request URL as defined in [NIP-98 `u` tag comparison](#nip-98-u-tag-comparison); a token for `/purchase/confirm` MUST NOT be accepted for `/game/:id`.
- **NIP-98 origin binding:** a `u` tag comparison that omits scheme and authority permits cross-origin replay of an otherwise-valid authorization within its validity window. Because ADP servers are interchangeable and several may serve the same coordinate, this is a live risk rather than a theoretical one. Scheme and authority MUST come from configuration, never from request headers.
- **File integrity:** `file_hash` is a developer commitment. Servers SHOULD periodically re-verify stored files against it.
- **Encoding canonicality:** hex-valued fields have exactly one valid form (see [Canonical Hex Encoding](#canonical-hex-encoding)). Accepting variants of a value used as a key or compared for equality creates a class of defect in which two representations of the same artifact are treated as distinct, defeating deduplication and producing verification results that depend on which participant performed the comparison.
- **Evidence asymmetry:** applying a relay-completeness threshold to positive findings makes availability depend on the least available relay rather than the most available one, inverting the redundancy relays provide. The threshold belongs on negative findings only; see [Relay availability](#relay-availability).

### Packages and execution

- **Hash verification proves provenance, not safety.** `file_hash` establishes that the archive is the one the developer signed. It says nothing about whether its entry names are safe to write, and every extraction rule above assumes a hostile archive from a correctly-signed publisher.
- **Executable selection is a privilege inside an already-verified archive.** A replaceable listing can be revised to point a profile at a different file that was always present in the package. The exposure is bounded — the file was shipped by the developer and covered by the digest — but it is not nil, and it is why installations pin their snapshot rather than re-resolving. A publisher key compromise can misdirect future installs; it cannot alter existing ones.
- **Launching is arbitrary code execution, by design.** This specification defines no sandbox. It is the same trust the buyer extends to any downloaded game, and the same posture as every comparable storefront, but it should be stated rather than implied.
- **Argument injection is prevented structurally, not by escaping.** Arguments are a list and are never re-parsed. Any implementation that builds a command string reintroduces the entire class.
- **Antivirus false positives on independent game builds are common** — packed executables and certain engine runtimes are routinely flagged. Clients SHOULD surface an install failure that names this as a possible cause rather than reporting an opaque I/O error.
- **Decompression limits are mandatory, not advisory.** A package's compressed size places no bound on its decompressed size.

## Relationship to Other Specifications

| Specification | Role |
|-----|------|
| [NIP-99](https://github.com/nostr-protocol/nips/blob/master/99.md) | Classified listing (`kind:30402`): game coordinate, current distribution, package, launch, access-policy metadata, and `fulfillment_authorization` root references |
| [NIP-102](NIP-102.md) | Payment-backed marketplace receipt (`kind:1020`); unchanged by ADP. ADP supplies its delegated-authorization and LSP-resolution rules. |
| [NIP-103](NIP-103.md) | Generic non-payment Entitlement Grant; unchanged by ADP. ADP supplies campaign anchoring, issuance delegation, and revocation authority. |
| [NIP-98](https://github.com/nostr-protocol/nips/blob/master/98.md) | HTTP Auth for all authenticated endpoints |
| [NIP-44](https://github.com/nostr-protocol/nips/blob/master/44.md) | Encryption of receipt `.content`, optional grant `.content`, and split-settlement private fields |
| [NIP-57](https://github.com/nostr-protocol/nips/blob/master/57.md) | Zap Request/Receipt as payment proof; its LSP metadata is what [LSP Verification](#lsp-verification-lud16) checks a zap receipt's signer against |
| [NIP-01](https://github.com/nostr-protocol/nips/blob/master/01.md) | Event model, lowercase hex identifiers, and single-letter tag indexing used by `#a`, `#d`, `#p` queries |

Revocation of a NIP-102 receipt or NIP-103 grant is commerce-layer only and never disables an installation; see [Execution is DRM-free](#execution-is-drm-free).

## Implementation Notes

- **`kind:30403` storage:** relays SHOULD retain server announcement events; they are replaceable and low-frequency.
- **Multi-server redundancy:** multiple `server` tags on one listing declare independent, redundant distribution points; none is privileged over another.
- **Version negotiation:** `supported_adp` in `/.well-known/adp` is advisory; no negotiation protocol is defined in this draft.
- **Provisional kinds:** current implementations centralize the experimental, provisional `1030`/`1031` values in one place each and advertise them as provisional, so they can be replaced when formal allocations exist. This is informative, not a conformance requirement.
- **Arcadestr reference deployment (informative):** the following describes one deployment's relay policy and is not an ADP requirement.
  - Arcadestr's dedicated relay is configured as the default preferred relay.
  - Arcadestr publishing operations SHOULD require successful acknowledgment from this relay before reporting publication as confirmed under the reference deployment's publication policy.
  - The reference ADP server SHOULD prioritize this relay when retrieving authorization-sensitive events.
  - Other relays remain supported for interoperability, redundancy, and publisher-directed (NIP-65) discovery.
  - The Arcadestr relay provides stronger operational consistency for events published through it, but does not constitute cryptographic proof that no newer event exists on another relay.

## Reference Implementation Checklist

For a server to be considered ADP-01 compliant it SHOULD implement:

- [ ] Serving `/.well-known/adp` and publishing a `kind:30403` announcement.
- [ ] Verifying direct developer upload or current enabled authorization with `upload_build`.
- [ ] Verifying file SHA-256 against `file_hash` on upload and periodically thereafter.
- [ ] Rejecting non-lowercase hex in every field documented as hex-encoded, rather than normalizing on read.
- [ ] Verifying NIP-98, listing signature, and payment proof before issuing a receipt in `/purchase/confirm`.
- [ ] Resolving the listing's `lud16` tag (never the developer's profile) to an LSP `nostrPubkey` and confirming the zap receipt was signed by it, before accepting the zap-receipt payment path — see [LSP Verification](#lsp-verification-lud16).
- [ ] Enforcing idempotency on `order` id and atomic replay protection on the canonical `payment_hash` across all proof formats: a same-buyer retry returns the original receipt, and any other reuse is `409`.
- [ ] Signing delegated receipts only under a current enabled authorization containing `issue_receipt`, and embedding its immutable root ID.
- [ ] Implementing `POST /provision` and `POST /provision/revoke` (§[Provisioning Endpoints](#provisioning-endpoints)), publishing a `kind:30404` attestation per provisioned key, with a distinct key per `(developer, scope)` pair — never a key shared across developers.
- [ ] Encrypting stored fulfillment private keys at rest.
- [ ] Resolving authorization and campaign chains with valid-prefix semantics and distinguishing invalidity from incomplete relay coverage.
- [ ] Implementing `POST /entitlement/claim` with campaign-chain validation, `issue_grant` authorization, and local `(buyer, coordinate, campaign)` idempotency.
- [ ] Verifying Entitlement Grants with campaign anchoring, issuance delegation, and developer-only revocation.
- [ ] Evaluating distribution authorization (`server` tags) independently of access in `/game/:game_coordinate`, and evaluating access in the defined order.
- [ ] Validating durable credentials before evaluating current ungated access policy.
- [ ] Never inferring public ADP access from zero or malformed price data.
- [ ] Applying coordinate-level ownership scope consistently to receipts and grants.
- [ ] Rejecting NIP-98 tokens whose `u` tag does not match the exact requested URL.
- [ ] Comparing NIP-98 `u` tags with scheme and authority from configured origin and path/query verbatim from the raw request, without decoding or normalization.
- [ ] Selecting valid positive evidence from completed relay responses before applying any completeness threshold.
- [ ] Documenting and applying a freshness policy for receipt issuance, grant issuance, uploads, and distribution-authorization checks, returning `503` when it cannot be satisfied.
- [ ] Serving public and active timed-access downloads without requiring authentication, and rejecting invalid NIP-98 tokens with `401`.
- [ ] Returning status codes per [HTTP errors](#http-errors), never reporting insufficient relay evidence as a definitive failure.
- [ ] Treating malformed, duplicate, or unknown `acquisition` policy as granting no access, without invalidating the listing.
- [ ] Applying exact `valid_from` activation with no early-activation allowance.
- [ ] Treating `kind:30406` cancellation, campaign cancellation, and grant revocation as terminal.
- [ ] Accepting `paid`, `fulfilled`, and `disputed` receipt tips as ownership and rejecting `refunded`.

Clients implementing campaigns SHOULD additionally:

- [ ] Discover campaigns from optional listing pointers when present.
- [ ] Support coordinate-based `#a` campaign discovery when pointers are absent or stale.
- [ ] Fetch specific campaign chains by developer author and indexed `d` tag.
- [ ] Treat listing campaign pointers as advisory only.
- [ ] Preserve indexed `d` and `a` across every campaign-chain event.

Clients implementing installation and launch SHOULD additionally:

- [ ] Validate `package`, `launch`, `launch_default`, and `platform` metadata per [Package Metadata and Launch Profiles](#package-metadata-and-launch-profiles), failing the launch tier on ambiguity rather than resolving by tag order.
- [ ] Verify `file_hash` before extraction and apply [Path rules](#path-rules), link and entry-type rejection, and mid-stream decompression limits.
- [ ] Persist the resolved snapshot, per-entry manifest, and raw signed listing; launch only from the pinned snapshot.
- [ ] Launch by direct execution with a list argument vector, no shell, no elevation, and no publisher-controlled environment.
- [ ] Detect updates by `file_hash` only, reconcile trees without deleting untracked data, and honor `update-conflict` and `requires-repair`.
- [ ] Never require network access or ownership re-verification to launch.

## Open Questions

Carried forward from the consolidated documents:

- **Authorization propagation latency:** no SLA is defined for observing a new listing enablement or `30406` cancellation.
- **Operator offboarding notification:** operator revocation should notify the developer so the developer can cancel `30406` and remove listing enablement.
- **Legacy refunds:** no refund flow is defined for the legacy single-payment profile. The experimental split profile defines recovery evidence without guaranteeing recipient cooperation.
- **Formal allocation of `<entitlement-grant-kind>`.** Implementations use `1030` provisionally; it is not a standardized allocation.
- **Formal allocation of `<adp-campaign-kind>`.** Implementations use `1031` provisionally; it is not a standardized allocation.
- **Formal allocation of split-settlement kinds `1032`–`1034`,** currently experimental.
- **Discount campaigns** remain in the paid NIP-102 path and require separate pricing semantics if standardized later.
- **Package format detection for legacy listings.** Listings without a `package` tag are offered no execution. Whether a client should offer opt-in sniffing for such listings, and with what warning, is unresolved.
- **Long-path handling on Windows.** Rule 9 plus a short installation root is expected to be sufficient for the engine layouts observed so far. It has not been validated against a deeply-nested Unreal tree.
- **Constants chosen rather than derived.** Every numeric limit in the package and launch rules — the path and segment lengths in [Path rules](#path-rules) rule 9, the argument count and length caps, the `profile-id` and `display-name` lengths, the entry-count cap, and the 200:1 compression-ratio cap with its 64 MiB warm-up — was selected on judgement, not measurement. They are recorded here as a single list so they can be revisited together against real published games rather than discovered piecemeal. The ratio's calculation is normative; these threshold values remain provisional policy to validate during field use.
- **Verification as a buyer action.** The per-entry manifest makes verifying an installation and repairing it from a retained package a walk over recorded regular-file digests. This specification requires the manifest but does not require the client to expose the operation, since its value is diagnostic and no field experience yet exists to shape it.
- **Localized display names.** `display-name` is a single string. Localization is deferred to the release manifest, where a structured field is affordable.
- **Link support.** Symlinks are refused outright. Some Linux and macOS build layouts use them legitimately, so a later revision may need a resolve-and-verify rule rather than a blanket refusal.

Remaining after the consolidation decisions (see [Resolved decisions](#resolved-decisions)):

- **Delegated receipt status-update capability.** Delegated fulfillment keys require explicit status-update authority, but no capability value has been defined, nor which transitions (for example `fulfilled`) it would cover. Until one is defined, only the developer can sign effective receipt status updates.
- **Non-campaign issuance policies for delegated grants.** Delegated grants require an independently verifiable issuance policy; a campaign is the only one defined. Issuance policies for delegated review copies, prizes, or migrations are undefined; until one is defined, any `source_event` present on a grant must reference a campaign.
- **Error response body.** Status codes are standardized; no error body schema is defined.

## Resolved decisions

The following decisions were taken during consolidation and are incorporated above.

| Item | Decision | Applied in |
|---|---|---|
| OQ-1 | Public and active timed-access downloads permit anonymous access without NIP-98. | [Acquisition policy](#acquisition-policy), [`GET /game/:game_coordinate`](#get-gamegame_coordinate) |
| OQ-2 | Positive-evidence model with publisher-preferred (NIP-65) relay discovery, documented freshness policies for freshness-sensitive operations, and `503` for indeterminate evidence. No specific relay is required. | [Relay availability](#relay-availability), [Implementation Notes](#implementation-notes) |
| OQ-3 | Canonical lowercase hex is required for ADP-controlled fields. Repository copies are reconciled without silently changing normative behavior. | [Canonical Hex Encoding](#canonical-hex-encoding) |
| OQ-4 | HTTP error responses are standardized; `503` signals insufficient relay evidence. | [HTTP errors](#http-errors) and each endpoint |
| OQ-5 | `paid`, `fulfilled`, and `disputed` retain ownership; `refunded` does not. The developer controls commercial transitions; delegated keys need explicit status-update authority. | [Receipt status under ADP](#receipt-status-under-adp) |
| OQ-6 | Developer-signed grants may omit a campaign. Delegated grants require an independently verifiable issuance policy. | [Entitlement Grants](#entitlement-grants-nip-103-profile) |
| OQ-7 | Campaign cancellation and grant revocation are terminal within their chains. | [Cancellation](#cancellation-1), [Grant revocation authority](#grant-revocation-authority) |
| OQ-8 | Malformed, duplicate, and unknown acquisition policies fail closed without invalidating the listing. | [Malformed acquisition policy](#malformed-acquisition-policy) |
| OQ-9 | The canonical Lightning `payment_hash` is the atomic replay identifier across proof formats. | [Replay protection](#replay-protection) |
| OQ-12 | A retry of an accepted purchase by its buyer, for its coordinate, returns the original signed receipt and may carry a fresh token while the buyer remains entitled and distribution is authorized. Any other reuse is `409`. The receipt is stored atomically with its acceptance before publication. | [Replay protection](#replay-protection), [`POST /purchase/confirm`](#post-purchaseconfirm) |
| OQ-10 | `kind:30404` is documented using the existing attestation format; historical credential authority remains anchored to `kind:30406`. | [Fulfillment Provisioning Attestation](#fulfillment-provisioning-attestation-kind30404) |
| OQ-11 | The consolidated draft is ADP `0.3.0`; independently versioned schemas keep their own versions. | [Status of this document](#status-of-this-document) |
| — | No early-activation allowance for fulfillment authorizations or attestations. | [No early activation](#no-early-activation) |
| — | Split-settlement endpoints stay outside the normative specification. | [Experimental split-settlement profile](#experimental-split-settlement-profile-version-1) |
| — | NIP-102 and NIP-103 remain independent specifications, unchanged. | [Relationship to Other Specifications](#relationship-to-other-specifications) |
| — | Provisional event kinds remain explicitly experimental. | [Event kinds](#event-kinds) |

## History

Superseded drafts and amendments are preserved, unmodified apart from a superseded notice, in [docs/protocol-history/](docs/protocol-history/README.md). They are not normative.
