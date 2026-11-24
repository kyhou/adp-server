# Documentation and Repository Contents

## Current References

- [CODEBASE.md](../CODEBASE.md): implementation and architecture map.
- [ADP-01.md](../ADP-01.md): the single authoritative ADP specification
  (consolidated draft `0.3.0`). Its former amendments are superseded records in
  [protocol-history/](protocol-history/README.md).
- [NIP-102.md](../NIP-102.md) and [NIP-103.md](../NIP-103.md): independent
  receipt and Entitlement Grant specifications referenced by ADP-01.
- [Protocol implementation gaps](protocol-implementation-gaps.md): where this
  server does not yet conform to ADP-01. These are open until code changes.
- [Settlement recovery backup](settlement-recovery-backup.md): recovery guidance.
- [Implementation roadmap](../adp-01-roadmap-v3.md): historical planning context.
  It cites superseded ADP-01 wording (for example "≥2 relays, highest
  `created_at`"); verify every item against current ADP-01, code, and tests.

## Protocol Synchronization

The canonical source of `ADP-01.md` is the sibling Arcadestr repository
(`../Arcadestr/ADP-01.md`). This repository's copy is a byte-for-byte mirror:

1. Make every protocol change in `../Arcadestr/ADP-01.md`, never here, and
   commit it there.
2. Copy it here unchanged: `cp ../Arcadestr/ADP-01.md ADP-01.md`.
3. Record the source in [ADP-01.sync](ADP-01.sync):
   `canonical_commit=` the Arcadestr commit that last changed `ADP-01.md`
   (`git -C ../Arcadestr log -1 --format=%H -- ADP-01.md`), and `sha256=` the
   hash of the copy (`sha256sum ADP-01.md`).
4. Run `scripts/check-adp-sync.sh`; it must pass.
5. Record any new conformance gaps in
   [protocol-implementation-gaps.md](protocol-implementation-gaps.md).

CI runs `scripts/check-adp-sync.sh` (job `adp-spec-sync`) against the public
Arcadestr repository on GitHub. It **fails** if `ADP-01.md` was edited in place
or is not a byte-for-byte copy of the recorded Arcadestr commit. It only
**warns** when Arcadestr `main` has a newer spec, so the two repositories may
be committed in either order, and when GitHub is unreachable or the recorded
commit has not been pushed yet. Locally, the script reads `../Arcadestr`.

`NIP-102.md` and `NIP-103.md` are separate specifications and are not part of
this process.

## Examples and Historical Material

- `examples/listing_event.json` preserves the original signed local-test event.
  It uses a localhost server, a test address, and a placeholder authorization
  root; it is not a production configuration or a generally valid upload recipe.
  Changing its signed fields would invalidate its original signature.
- [Protocol history](protocol-history/README.md) contains explicitly superseded,
  non-normative drafts and the consolidated ADP-01 amendments.
- [Proposals](proposals/README.md) keep their original text with a resolution
  notice. Both current proposals have been resolved into ADP-01.
- `superpowers/plans/` and `superpowers/specs/` contain dated implementation
  snapshots, not automatically current requirements or completion records.

## What Belongs in Git

Keep source, manifests, `Cargo.lock`, SQL migrations, tests, examples, CI,
maintained documentation, and the secret-free `.env.example` template.
Keep `testgame.bin` at the repository root: Arcadestr's sibling regression
scripts read that exact path as an inert upload/download payload.

Keep `.env`, operator keys, runtime databases, uploaded games, build output,
caches, personal editor/agent state, backups, and absolute documentation links
out of Git. The optional local `arcadestr_CODEBASE.md` link is ignored; use
`../Arcadestr/CODEBASE.md` as the portable cross-repository reference.
Use the ignored `.local-artifacts/` directory for one-off captures and exports.
