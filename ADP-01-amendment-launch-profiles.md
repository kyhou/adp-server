# ADP-01 Amendment: Package Format, Launch Profiles, and Execution

`draft` `proposed amendment to ADP-01` `v1` `client-only — no server changes`

## Motivation

ADP-01 defines how a client obtains and verifies a game archive. It defines nothing about what happens next. A verified archive is an opaque blob: no part of the protocol states which file inside it is the game, what working directory it expects, or what arguments it takes. A client that has completed a purchase, a download, and a hash verification still cannot start the game.

This amendment supplies the missing statement and defines who is authorized to make it.

Steam's answer is that the storefront operator holds launch configuration in mutable server-side metadata and the client trusts the operator. That model is unavailable here by design: no ADP server is privileged, servers are interchangeable, and an installed game must remain launchable when every server that ever carried it is gone. Launch configuration is therefore developer-signed Nostr metadata, cached and served by nothing that has authority over it.

## Scope

This amendment covers the current one-listing-to-one-archive model. It adds tags to `kind:30402` and defines client obligations for extraction and execution.

It does **not** define multiple artifacts per listing, immutable release manifests, or release channels. Those are the subject of a subsequent amendment and are deliberately deferred until real games have been published and launched under this one. The tag vocabulary defined here is chosen so that relocation into a future release manifest is a mechanical remapping rather than a redesign; see [Forward compatibility](#forward-compatibility).

**No ADP server change is required by this amendment.** Servers continue to store and serve one file per coordinate, verified against `file_hash`. Every requirement below binds clients.

## Terminology

| Term | Definition |
|------|------------|
| **Package** | The archive identified by a listing's `file_hash`, in the format declared by its `package` tag |
| **Installation root** | The directory into which a package is extracted on the buyer's device |
| **Launch profile** | A publisher-signed, named statement of one way to start the game: an executable, a working directory, arguments, and a type |
| **Relative path** | A path expressed relative to the installation root, subject to [Path rules](#path-rules) |
| **Resolved snapshot** | The exact set of values a client used to perform one installation, persisted locally at install time |

## Listing extensions (`kind:30402`)

Two new tags. Both use multi-character names. This is intentional and consistent with NIP-01: only single-letter tag names are guaranteed relay-filterable, and neither of these is ever a filter subject. They are payload, not index.

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

A listing with no `package` tag and no `launch` tags follows existing behavior: the file may be downloaded and verified, but the client offers no execution.

### The `launch` tag

Repeatable. Each occurrence defines one launch profile.

```
["launch", "<profile-id>", "<executable>", "<working-dir>", "<arguments>", "<type>", "<display-name>"]
```

All seven elements are REQUIRED and positional. Elements beyond the seventh MUST be ignored by current implementations and MUST NOT be treated as an error; this reserves room for additive extension.

| Position | Element | Rules |
|---:|---|---|
| 1 | `profile-id` | Stable publisher-assigned identifier. `[a-z0-9][a-z0-9_-]{0,63}`. MUST be unique within the listing. MUST NOT be reassigned to a different profile in a later listing revision. |
| 2 | `executable` | Relative path to the file to execute. Subject to [Path rules](#path-rules). MUST NOT be empty. |
| 3 | `working-dir` | Relative path to the process working directory. Subject to [Path rules](#path-rules). `.` denotes the installation root. |
| 4 | `arguments` | A JSON array of strings. `[]` when there are no arguments. Subject to [Argument rules](#argument-rules). |
| 5 | `type` | One of `game`, `config`, `server`, `editor`, `tool`. Unrecognized values MUST be treated as `tool`. |
| 6 | `display-name` | Human-readable label, 1–64 characters, no control characters. |

Example:

```jsonc
{
  "kind": 30402,
  "pubkey": "<developer-pubkey>",
  "tags": [
    ["d", "my-game"],
    ["server", "https://dist.arcadestr.io"],
    ["file_hash", "a3f5..."],
    ["version", "1.2.0"],
    ["platform", "windows-x86_64"],
    ["package", "zip"],
    ["launch", "default", "bin/Game.exe", "bin", "[]", "game", "Play"],
    ["launch", "vulkan", "bin/Game.exe", "bin", "[\"--renderer\",\"vulkan\"]", "game", "Play with Vulkan"],
    ["launch", "config", "ConfigTool.exe", ".", "[]", "config", "Configuration tool"],
    ["lud16", "studio@getalby.com"]
  ],
  "content": "...",
  "sig": "<developer-signature>"
}
```

#### Malformed tag sets

Malformed metadata is handled at two levels.

A listing MUST be treated as having no launch tier — downloadable but not launchable — when it carries more than one `package` tag, or when two `launch` tags contain the same syntactically valid `profile-id`. The duplicate rule applies even when one of those tags is invalid for some other reason: once two tags claim the same valid identifier, their intended relationship is ambiguous and the client MUST NOT resolve that ambiguity by tag order.

First-occurrence-wins is explicitly rejected for both cases. It would make which executable runs depend on tag ordering within a signed event, and it would let a partially-correct edit silently repoint an identifier that installed clients already hold. Failing the launch tier is visible; resolving by position is not.

An individual `launch` tag is invalid when a required positional element is missing, `profile-id` does not match its grammar, `executable` or `working-dir` violates [Path rules](#path-rules), `arguments` is not a JSON array of strings or violates [Argument rules](#argument-rules), or `display-name` violates its length or control-character rule. An invalid individual profile MUST be ignored for execution and MUST NOT make an otherwise unambiguous launch tier fail. Unrecognized `type` values are not malformed; they are treated as `tool` as specified above.

Clients SHOULD surface discarded profiles and the reason each profile was discarded rather than silently omitting them. Publishing clients SHOULD present this before signing; consuming clients SHOULD make it available when inspecting a listing or diagnosing why an expected launch option is absent.

After invalid individual profiles are discarded, a listing with no valid profile of type `game` MUST NOT be offered for execution. It remains downloadable.

This case is reachable in practice. `kind:30402` is replaceable, so an editing client rebuilds the entire tag set on every revision; a client that carries unmanaged tags forward without filtering the `launch` family will emit the previous revision's profiles alongside the new ones. Publishing clients MUST assert, before signing, that exactly one `package` tag, no duplicate syntactically-valid `profile-id`, and no invalid `launch` tag are present.

`profile-id` identifies a **semantic launch option**, not an executable path. It answers "which way of starting this game is this," and its meaning must remain constant across listing revisions.

Changing which file a profile points at is therefore ordinary. A build reorganization moving `Game.exe` to `bin/Game.exe` keeps `profile-id` `default`: the option is still "play the game," and only its implementation moved. Repurposing `default` from the main game to a dedicated server is reassignment and is prohibited — the correct action is a new `profile-id`.

This matters because a local installation pins the `profile-id` the buyer selected, and re-pins it after an update. Stable semantics mean the buyer's choice survives; unstable semantics mean it silently becomes a different program.

### Eligibility

In this amendment, a listing's `platform` tags govern whether the client offers execution at all. Launch profiles carry no eligibility of their own: every profile in a listing is eligible on every platform that listing declares.

This follows from the one-archive-per-listing model. Per-profile eligibility becomes meaningful only when one listing carries artifacts for several platforms, which this amendment does not permit.

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

A `platform` value that does not normalize to a defined pair MUST be treated as not matching the host, and MUST NOT be treated as a parse error or as matching everything. A listing declaring only unrecognized platforms is surfaced as unsupported on this device — the same state as a genuine platform mismatch. This is what allows a platform this version does not define to be published today and become installable later without any listing change.

A listing that declares no `platform` tag makes no claim about where it runs. Such a listing MUST NOT be offered for execution: a client MUST treat it as having no launch tier, regardless of whether `package` and `launch` tags are present.

This differs from how a `platform`-less listing behaves for download, which is unrestricted. The asymmetry is deliberate. Launch metadata is new in this amendment, so no existing listing depends on the permissive reading, and the permissive reading has a concrete failure mode: a Windows executable offered on a Linux host. A publisher whose archive genuinely runs on several targets declares each of them.

A listing declaring only unrecognized platform values has a complete launch tier and is launchable nowhere. This is the intended consequence of the vocabulary rule above, and it is what allows a target this version does not define to be published now and become launchable later without any change to the listing.

## Path rules

These rules apply to `executable` and `working-dir`, and independently to every entry name inside a package during extraction. They are the primary security surface of this amendment: a verified hash proves the archive is the one the developer signed, not that its contents are safe to write to disk.

This version deliberately defines a **portable package-path namespace** rather than target-specific filename rules. The same path is valid or invalid regardless of the platform on which the publisher creates the archive, the platform on which a client validates it, or which supported target the listing declares. This is a v1 interoperability restriction: it sacrifices some filenames that are legal on Unix filesystems in exchange for deterministic validation, simpler tooling, and the ability to move the same package metadata across targets without reinterpretation. A later amendment MAY define target-specific package-path rules only if validity remains deterministic from signed artifact metadata and independent of the publisher's or consuming client's host platform. Packages published under this version remain subject to this portable namespace permanently.

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

## Argument rules

Arguments MUST be a list of separate strings. A single string that is split by the client, or passed to a shell for word-splitting, is non-conforming.

- The array MUST contain at most 32 elements.
- Each element MUST be at most 1024 characters.
- Elements MUST NOT contain NUL.
- No element is interpreted, expanded, or substituted by the client. There is no variable syntax, no `%command%` equivalent, and no path expansion.

There is deliberately no field for a command line, a shell invocation, or publisher-supplied environment variables. Each would convert a publisher-signed metadata field into arbitrary command execution with a broader reach than the executable itself already has.

## Extraction requirements

A client MUST NOT begin extraction before the package's SHA-256 has been verified against the listing's `file_hash`.

During extraction the client MUST:

1. Apply the [Path rules](#path-rules) to every entry name and abort on the first violation. Abort is per-package, not per-entry: a package containing one hostile entry is rejected in full.
2. Reject symbolic links and hard links. A conforming implementation of this amendment does not create links during extraction. (This is a deliberate v1 restriction; safely resolving link targets against an escape check is a later extension.)
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

   These two limits describe disproportion, which is what distinguishes a decompression attack from a large game. This amendment deliberately sets **no maximum extracted size**. An absolute constant is a poor proxy for a quantity the client can measure exactly, and a protocol-level ceiling on how large a game may be would eventually make a legitimate title uninstallable. Bounding extraction against available disk space is a client responsibility and is not specified here, since no other party observes it.
5. Extract into a temporary directory adjacent to the final installation root, then move it into place as a single operation. A client MUST NOT extract directly into the final root. On platforms where directory rename over an existing target fails, the client MUST use a replace-capable operation or a delete-then-rename with a documented recovery step for interruption between the two.
6. Preserve entry paths exactly. Clients MUST NOT strip a leading directory component. An executable path is relative to the installation root, which corresponds exactly to the package's internal structure.
7. On Unix hosts, apply the execute bit to the file named by each profile's `executable` when the package format did not carry mode bits, or when it carried them and the bit is absent. Clients MUST NOT apply the execute bit to the tree at large. ZIP writers frequently discard Unix mode, so a Linux build shipped as a ZIP will otherwise not run.

After extraction and before recording the installation, the client MUST verify that every profile's `executable` resolves to an existing regular file within the installation root, and that every profile's `working-dir` resolves to an existing directory within it. A profile failing this check MUST be recorded as unavailable and MUST NOT be offered. Clients SHOULD surface unavailable profiles and the reason each profile is unavailable rather than silently omitting them. A listing whose profiles all fail MUST be surfaced as installed-but-not-launchable rather than as a successful install.

Resolution MUST be performed against the real path after symlink resolution of the installation root itself, and a target resolving outside that root MUST be rejected.

## Execution requirements

A client launching a profile MUST:

1. Execute the file directly. It MUST NOT interpose `cmd.exe`, PowerShell, `/bin/sh`, or any other interpreter, and MUST NOT construct a command string that any layer will re-parse.
2. Pass `arguments` as distinct argument vector elements.
3. Set the process working directory to the resolved `working-dir`.
4. Not request or apply privilege elevation.
5. Not pass publisher-controlled environment variables. The child environment is the client's own, minus any variable holding client-internal secrets, credentials, signer state, or key material. Clients SHOULD construct the child environment from an explicit allowlist rather than by filtering an inherited one.

Launching MUST NOT require contacting a relay or an ADP server. See [Execution is DRM-free](#execution-is-drm-free).

## Installation records

At install time the client MUST persist a resolved snapshot containing at minimum:

- the game coordinate and the installation root;
- the package digest actually verified;
- the **resolved** package format, operating system, and architecture;
- the full set of launch profiles as resolved, including which were found unavailable;
- **a per-entry manifest** of every regular file and directory extracted, recording its path relative to the installation root and its entry type; every regular-file entry additionally records its SHA-256;
- **the complete raw signed `kind:30402` event JSON** from which the profiles were resolved.

The per-entry manifest is a client-side structure with no protocol surface — it is not published, not signed, and no server sees it. It is required because it distinguishes paths and entry types the package placed from paths the buyer created, which is the basis of [Updates](#updates), and because regular-file digests make verifying an installation possible without re-downloading. Empty package-created directories are recorded as directory entries. Regular-file digests are computed from bytes the client is already streaming during extraction.

The resolved values are recorded explicitly rather than left to be re-derived from the raw event. The raw event is evidence; the snapshot is the decision. Re-deriving would let a future change to normalization or parsing reinterpret an existing installation, which is exactly what pinning exists to prevent.

The raw event is required, not its ID. `kind:30402` is a parameterized replaceable event; relays are not obliged to retain superseded revisions, so an event ID recorded at install time is generally not resolvable afterwards. Storing the event itself is what allows the publisher's signature over the launch metadata to be re-verified offline and indefinitely.

The raw event establishes provenance of the distribution and launch metadata. It is not by itself proof of the buyer's ownership: acquisition credentials are NIP-102 receipts and NIP-103 grants, held separately, and an installation record is not evidence of durable entitlement.

Launching MUST use the pinned snapshot. A client MUST NOT re-resolve launch configuration from the current listing at launch time. A later listing revision MUST NOT alter how an existing installation starts; it takes effect only when the buyer explicitly updates or reinstalls.

## Updates

An update is available when the current listing's `file_hash` differs from the installed snapshot's package digest. That comparison is the entire detection rule.

The `version` tag MUST NOT be used for this. It is a free-form publisher string — `1.2.0`, `Build 447`, `2026-08-01` — and no ordering can be derived from it reliably. `kind:30402` is replaceable, so the current listing is by definition the current build; a client syncs to what the developer publishes now rather than climbing a version sequence. A publisher who republishes an earlier build produces an update that installs that earlier build, which is correct. Clients SHOULD name the action with the target version so this does not appear to be a defect.

Updating MUST be explicit. A client MUST NOT update an installation without the buyer initiating it.

### Update state vocabulary

Two update failure states are distinct and normative:

- **`update-conflict`** — reconciliation has not begun mutating the installation because preflight or tree planning found a conflict that cannot be resolved without risking buyer-created data. The existing installation remains intact and MAY remain launchable.
- **`requires-repair`** — reconciliation began mutating the installation and was interrupted or failed before completion. The installation MUST NOT be presented as launchable until repair completes.

A UI MAY group either state under a broader presentation category such as "Needs attention", but that label is not a protocol state and MUST NOT replace the distinction above in persisted or interoperable state.

`update-conflict` persists until the buyer resolves it. A client MUST NOT resolve it automatically, and MUST NOT clear it by any action that discards untracked data without the buyer's explicit, specific confirmation. Automatic resolution is precisely the data loss the conflict check exists to prevent, and a client that resolves silently is worse than one that never checked, since the buyer is given no opportunity to intervene.

Conforming resolutions are: the buyer relocates or removes the untracked data and retries the update; the buyer explicitly confirms that the untracked data at the conflicting path may be discarded, after which reconciliation proceeds; or the publisher republishes with a layout that no longer conflicts, after which a retry finds none. A client MUST offer at least the first. The installation remains at its current version until one of these occurs, which is the correct outcome: a buyer stalled on an older build has lost nothing, while a buyer whose saves were deleted cannot be made whole.

### Tree reconciliation

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

### Interruption

Reconciliation modifies the installation root in place and therefore has no rollback point. A client MUST record the installation in the `requires-repair` state when reconciliation is interrupted, and MUST NOT present it as launchable until repair completes.

Recovery is re-running the same reconciliation, which is idempotent: it is a comparison against a manifest, not a sequence of steps, so a partially-applied update converges on re-execution. No journal or resume state is required.

Recovery requires the new package. A client MUST keep the newly downloaded package until reconciliation has either completed successfully or the installation no longer depends on it for repair. If reconciliation is interrupted, the client MUST NOT discard that package merely because it was not yet promoted to the retained-current-package slot. On success, the new package becomes the current retained package when retention is enabled, and the previous retained package may then be removed.

Clients that do not retain packages after successful installation may still treat the new package as temporary once repair is no longer required; if it is later needed again, it must be re-downloaded.

### Preflight

Before reconciliation begins, a client MUST verify that no process from that installation is running. Replacing files belonging to a running process fails on Windows and produces undefined behavior in the running process elsewhere.

Free-space estimation and other local resource policy are implementation concerns rather than interoperability requirements. A client MAY preflight available space, but this amendment does not define a required estimate. Resource exhaustion MUST NOT cause an installation to be reported as successfully updated; if mutation has begun, the installation enters `requires-repair` as defined above.

### Profile re-pinning

After a successful update the client pins the new listing's profile set. If the buyer's previously selected `profile-id` is absent from it, the client MUST fall back to the first profile of type `game` and MUST inform the buyer that the selection changed. This is the case the `profile-id` stability rule exists to make rare; it cannot make it impossible.

## Uninstallation

Uninstalling removes the installation root. Whether the downloaded package is also deleted MUST be the buyer's choice, presented at the point of uninstallation with both sizes shown.

The default MUST be to retain the package. The buyer paid for those bytes; execution is DRM-free; the archive is the durable artifact, and discarding it by default would contradict the ownership model the rest of this protocol exists to support. Retention additionally makes reinstallation and [update repair](#interruption) work without network access.

A client retaining packages MUST provide a view listing retained packages with their sizes and an individual delete action, so retained data is visible and manageable rather than accumulating invisibly. A client SHOULD NOT evict retained packages automatically.

## Execution is DRM-free

Once installed, a game launches. This is a protocol-level guarantee, not an implementation convenience.

- Launching MUST NOT require network access, relay reachability, or ADP server reachability.
- Launching MUST NOT be conditioned on re-verifying ownership.
- Revocation of a NIP-102 receipt or a NIP-103 entitlement grant is a commerce-layer fact. It affects future acquisition, future downloads, and displayed ownership state. It MUST NOT disable an existing installation.

This follows directly from ADP-01's trust model. A credential that is portable across servers and verifiable without any operator would be worth little if execution nonetheless depended on an operator remaining reachable and willing.

Clients MUST NOT present installation as evidence of durable ownership, and MUST NOT present revocation as removing access to installed files.

## Decision record: Mark-of-the-Web

On Windows, files written by a browser or by Explorer's ZIP handler carry a `Zone.Identifier` alternate data stream. SmartScreen consults it, and an unsigned executable so marked produces a reputation warning before it runs.

A client extracting a package with its own code produces no such stream. Extracted executables therefore launch without a SmartScreen prompt.

**This amendment does not require clients to apply Mark-of-the-Web to extracted files.** The reasoning is that virtually all independent game builds are unsigned, marking would produce a warning on essentially every launch, and a warning that always fires trains buyers to dismiss it — degrading the signal without adding protection.

This trade should be stated precisely. Declining to mark **removes one operating-system reputation check**, and nothing in this protocol replaces it. The developer signature over `file_hash` is a different kind of claim: it establishes that these are exactly the bytes the publisher signed, which is provenance, not safety. A correctly-signed package from a compromised or malicious publisher passes verification unchanged. SmartScreen answers a question this protocol does not ask, and the decision here is to accept the loss of that answer in exchange for warnings that mean something when they do appear.

This is recorded as a deliberate decision rather than left as an implementation accident, because the alternative is roughly one line of code and would otherwise be adopted or dropped inconsistently. Clients MAY offer marking as a user-controlled setting. A client that marks extracted files MUST do so uniformly and MUST NOT vary by publisher.

## Publishing client guidance

This extends ADP-01's [Mandatory fields for client-published listings](ADP-01.md#mandatory-fields-for-client-published-listings) with a third tier:

- **Launch tier — required only for listings that want the buyer to be able to play:** `package`, at least one `platform`, and at least one `launch` profile of type `game`. A listing carrying `file_hash` and `server` but no launch tier remains valid and purchasable; the buyer can acquire and download it, and the client SHOULD present it as downloadable-but-not-launchable rather than silently offering a Play action that cannot work.

A publishing client enforcing the launch tier SHOULD, before allowing submit:

1. Read the archive index and present the file listing. Executable candidates MAY be ranked — `.exe` on Windows targets, executable ELF files on Linux targets, known engine output layouts — but the client MUST NOT select one silently. The publisher selects; the client suggests.
2. Validate every profile path against [Path rules](#path-rules) and against the archive index, so a nonexistent path is caught before publication rather than after a buyer's download.
3. Report archive contents that will fail on a platform the listing declares: reserved device names, case-collisions, trailing dots or spaces, backslashes in entry names, symlinks. These originate in the publisher's build and are cheap to fix there and expensive to fix after distribution.
4. Warn when an edit removes a `profile-id` that a previous revision published, since installed clients may have it pinned. A client MUST NOT warn merely because a profile's executable path changed between revisions; per [the `launch` tag](#the-launch-tag), that is ordinary build reorganization.

An embedded manifest inside the archive MAY be read as a source of publisher-facing defaults for automated or headless publishing. It MUST NOT be authoritative, and its values MUST NOT reach a published listing without the publisher reviewing and signing them.

## Forward compatibility

A subsequent amendment will introduce multiple platform artifacts under one coordinate and an immutable release manifest event. The vocabulary here is chosen so that migration is a remapping:

| This amendment (`kind:30402` tags) | Future release manifest |
|---|---|
| `file_hash` | per-artifact `digest` |
| `package` | per-artifact `format` |
| `platform` (fused `<os>-<arch>`) | per-artifact `os` / `arch`, as separate lists |
| `launch` tags | per-artifact `launch` array, same fields |
| `version` | manifest `version` |
| — | manifest `channel` |
| — | per-artifact `sources` |

Two properties do not survive migration unchanged and are called out here so implementations do not depend on them:

- **Eligibility moves from the listing to the artifact,** and profiles gain the ability to narrow it further. Implementations SHOULD keep the selection predicate in a distinct code path from the execution parameters even though this amendment gives it only one input.
- **The fused `platform` form is not carried forward.** The split form is what the manifest uses. Clients SHOULD parse `platform` into separate OS and architecture values internally rather than matching the fused string.

Launch profiles as defined here are protocol-neutral: nothing in [Extraction requirements](#extraction-requirements), [Execution requirements](#execution-requirements), or [Installation records](#installation-records) refers to how the profile was encoded. A client structured accordingly replaces its resolver when the manifest arrives and changes nothing else.

## Security considerations

- **Hash verification proves provenance, not safety.** `file_hash` establishes that the archive is the one the developer signed. It says nothing about whether its entry names are safe to write, and every extraction rule above assumes a hostile archive from a correctly-signed publisher.
- **Executable selection is a privilege inside an already-verified archive.** A replaceable listing can be revised to point a profile at a different file that was always present in the package. The exposure is bounded — the file was shipped by the developer and covered by the digest — but it is not nil, and it is why installations pin their snapshot rather than re-resolving. A publisher key compromise can misdirect future installs; it cannot alter existing ones.
- **Launching is arbitrary code execution, by design.** This amendment defines no sandbox. It is the same trust the buyer extends to any downloaded game, and the same posture as every comparable storefront, but it should be stated rather than implied.
- **Argument injection is prevented structurally, not by escaping.** Arguments are a list and are never re-parsed. Any implementation that builds a command string reintroduces the entire class.
- **Antivirus false positives on independent game builds are common** — packed executables and certain engine runtimes are routinely flagged. Clients SHOULD surface an install failure that names this as a possible cause rather than reporting an opaque I/O error.
- **Decompression limits are mandatory, not advisory.** A package's compressed size places no bound on its decompressed size.

## Open questions

- **Package format detection for legacy listings.** Listings published before this amendment carry no `package` tag. This amendment offers no execution for them. Whether a client should offer opt-in sniffing for such listings, and with what warning, is unresolved.
- **Long-path handling on Windows.** Rule 9 plus a short installation root is expected to be sufficient for the engine layouts observed so far. It has not been validated against a deeply-nested Unreal tree.
- **Constants chosen rather than derived.** Every numeric limit in this document — the path and segment lengths in [Path rules](#path-rules) rule 9, the argument count and length caps, the `profile-id` and `display-name` lengths, the entry-count cap, and the 200:1 compression-ratio cap with its 64 MiB warm-up — was selected on judgement, not measurement. They are recorded here as a single list so they can be revisited together against real published games rather than discovered piecemeal. The ratio's calculation is normative; these threshold values remain provisional policy to validate during field use.
- **Verification as a buyer action.** The per-entry manifest makes verifying an installation and repairing it from a retained package a walk over recorded regular-file digests. This amendment requires the manifest but does not require the client to expose the operation, since its value is diagnostic and no field experience yet exists to shape it.
- **Localized display names.** `display-name` is a single string. Localization is deferred to the release manifest, where a structured field is affordable.
- **Link support.** Symlinks are refused outright. Some Linux and macOS build layouts use them legitimately, so a later revision may need a resolve-and-verify rule rather than a blanket refusal.

## Relationship to other specifications

| Specification | Role |
|---|---|
| [ADP-01](ADP-01.md) | Defines acquisition, distribution authorization, and the `file_hash` this amendment's package is verified against |
| [ADP-01 Free Acquisitions Amendment](ADP-01-amendment-free-acquisitions.md) | Defines the access policies under which a package may be obtained; unaffected by this amendment |
| [NIP-102](NIP-102.md) | Ownership credential; its revocation semantics are explicitly commerce-layer only, per [Execution is DRM-free](#execution-is-drm-free) |
| [NIP-103](NIP-103.md) | Entitlement grants; same |
| [NIP-99](99.md) | The classified listing carrying these tags |
