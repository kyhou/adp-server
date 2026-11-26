#!/usr/bin/env bash
# Verifies that <dir> holds exactly the expected release archives for
# <version>, writes and checks <dir>/SHA256SUMS, and writes release notes for
# <tag> at <sha> to <notes-file>. Run from a full-history checkout.
#
# Usage: assemble.sh <version> <tag> <sha> <dir> <notes-file>
set -euo pipefail

version="$1"
tag="$2"
sha="$3"
dir="$4"
notes="$5"

expected=(
  "adp-server-$version-linux-arm64.tar.gz"
  "adp-server-$version-linux-x86_64.tar.gz"
)

actual="$(cd "$dir" && find . -mindepth 1 -maxdepth 1 -printf '%P\n' | LC_ALL=C sort)"
wanted="$(printf '%s\n' "${expected[@]}" | LC_ALL=C sort)"
if [[ "$actual" != "$wanted" ]]; then
  echo "::error::release assets differ from the expected set" >&2
  diff <(echo "$wanted") <(echo "$actual") >&2 || true
  exit 1
fi
for asset in "${expected[@]}"; do
  gzip -t "$dir/$asset" || { echo "::error::$asset is not a valid gzip archive" >&2; exit 1; }
done

(cd "$dir" && LC_ALL=C sha256sum -- "${expected[@]}" >SHA256SUMS && sha256sum --check --strict SHA256SUMS)

previous="$(git describe --tags --abbrev=0 --match 'v[0-9]*.[0-9]*.[0-9]*' "$sha^" 2>/dev/null || true)"
{
  echo "adp-server $tag, built from \`$sha\`."
  echo
  echo "This is a pre-release. See"
  echo "[running a release](https://github.com/kyhou/adp-server/blob/$sha/docs/releasing.md#running-a-release)"
  echo "for supported Linux systems, runtime requirements and how to verify \`SHA256SUMS\`."
  echo
  if [[ -n "$previous" ]]; then
    echo "## Changes since $previous"
    echo
    git log --no-merges --format='- %s (%h)' "$previous..$sha"
  else
    echo "## Changes"
    echo
    echo "First release; earlier history is not itemized."
  fi
} >"$notes"
echo "Assembled ${#expected[@]} archives for $tag; notes in $notes"
