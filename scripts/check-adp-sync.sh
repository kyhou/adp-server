#!/usr/bin/env bash
# Verifies that ADP-01.md is a faithful mirror of the canonical specification
# in the Arcadestr repository. See docs/README.md §Protocol Synchronization.
#
# docs/ADP-01.sync records the Arcadestr commit the mirror was copied from and
# the SHA-256 of the copy. Checks:
#   1. ADP-01.md still matches the recorded SHA-256.               (error)
#   2. The recorded Arcadestr commit's ADP-01.md is byte-identical. (error)
#   3. Arcadestr's current branch has the same spec.     (warning: sync lag)
# Checks 2 and 3 need the canonical source; if it cannot be reached they are
# reported as warnings, never as passes.
#
# Canonical source, in order of preference:
#   ADP_CANONICAL_REPO  path to a local Arcadestr checkout (default ../Arcadestr)
#   ADP_CANONICAL_RAW   raw-file URL prefix, e.g.
#                       https://raw.githubusercontent.com/kyhou/Arcadestr
#   ADP_CANONICAL_BRANCH branch for check 3 (default main)
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
mirror="$root/ADP-01.md"
sync_file="$root/docs/ADP-01.sync"
repo="${ADP_CANONICAL_REPO:-$root/../Arcadestr}"
raw="${ADP_CANONICAL_RAW:-https://raw.githubusercontent.com/kyhou/Arcadestr}"
branch="${ADP_CANONICAL_BRANCH:-main}"

warn() {
	if [ -n "${GITHUB_ACTIONS:-}${GITEA_ACTIONS:-}" ]; then
		echo "::warning::$1"
	else
		echo "warning: $1" >&2
	fi
}
fail() {
	if [ -n "${GITHUB_ACTIONS:-}${GITEA_ACTIONS:-}" ]; then
		echo "::error::$1"
	else
		echo "error: $1" >&2
	fi
	exit 1
}
sha256() { sha256sum | cut -d' ' -f1; }

[ -f "$sync_file" ] || fail "missing $sync_file"
commit="$(sed -n 's/^canonical_commit=//p' "$sync_file")"
recorded="$(sed -n 's/^sha256=//p' "$sync_file")"
[[ "$commit" =~ ^[0-9a-f]{40}$ ]] || fail "canonical_commit in docs/ADP-01.sync must be a full lowercase commit SHA"
[[ "$recorded" =~ ^[0-9a-f]{64}$ ]] || fail "sha256 in docs/ADP-01.sync must be 64 lowercase hex characters"

# 1. Local integrity: the mirror was not edited in place.
actual="$(sha256 <"$mirror")"
[ "$actual" = "$recorded" ] ||
	fail "ADP-01.md ($actual) does not match docs/ADP-01.sync ($recorded). Never edit the mirror; re-copy it from Arcadestr and update docs/ADP-01.sync."
echo "ok: ADP-01.md matches its recorded SHA-256"

# Fetches the canonical ADP-01.md at a revision, from the local checkout or
# the raw URL. Prints nothing and returns non-zero when unavailable.
canonical_at() {
	local rev="$1"
	if [ -d "$repo/.git" ] && git -C "$repo" cat-file -e "$rev:ADP-01.md" 2>/dev/null; then
		git -C "$repo" show "$rev:ADP-01.md"
		return 0
	fi
	curl --fail --silent --show-error --location --max-time 20 "$raw/$rev/ADP-01.md" 2>/dev/null
}

# 2. Faithful copy of a real canonical revision.
# Hash the bytes straight from the pipe: command substitution would strip
# trailing newlines. pipefail makes a fetch failure fail the assignment.
if pinned_sha="$(canonical_at "$commit" | sha256)"; then
	[ "$pinned_sha" = "$actual" ] ||
		fail "ADP-01.md differs from Arcadestr $commit ($pinned_sha). The mirror must be a byte-for-byte copy."
	echo "ok: ADP-01.md is identical to Arcadestr $commit"
else
	warn "could not fetch Arcadestr $commit:ADP-01.md (not pushed yet, or offline); faithful-copy check skipped"
fi

# 3. Sync lag: a newer canonical spec is allowed temporarily, but reported.
if current_sha="$(canonical_at "$branch" | sha256)"; then
	if [ "$current_sha" = "$actual" ]; then
		echo "ok: ADP-01.md is current with Arcadestr $branch"
	else
		warn "Arcadestr $branch has a different ADP-01.md ($current_sha); re-sync the mirror (see docs/README.md §Protocol Synchronization)"
	fi
else
	warn "could not fetch Arcadestr $branch:ADP-01.md; sync-lag check skipped"
fi
