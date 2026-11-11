# Documentation and Repository Contents

## Current References

- [CODEBASE.md](../CODEBASE.md): implementation and architecture map.
- [ADP-01.md](../ADP-01.md), root amendments, [NIP-102.md](../NIP-102.md), and
  [NIP-103.md](../NIP-103.md): protocol references.
- [Settlement recovery backup](settlement-recovery-backup.md): recovery guidance.
- [Implementation roadmap](../adp-01-roadmap-v3.md): planning context; verify
  outstanding items against current code and tests.

The sibling Arcadestr repository's `ADP-01.md` and launch-profile amendment
currently differ from this repository's copies. Reconcile their requirements
before synchronizing; do not deduplicate them by overwriting either copy.

## Examples and Historical Material

- `examples/listing_event.json` preserves the original signed local-test event.
  It uses a localhost server, a test address, and a placeholder authorization
  root; it is not a production configuration or a generally valid upload recipe.
  Changing its signed fields would invalidate its original signature.
- [Protocol history](protocol-history/README.md) contains explicitly superseded,
  non-normative drafts.
- `proposals/` contains non-normative proposals pending explicit incorporation.
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
