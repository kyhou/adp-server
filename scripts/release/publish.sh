#!/usr/bin/env bash
# Publishes the assembled assets in <dir> as the GitHub pre-release for <tag>.
# Safe to re-run; it never creates a second release or overwrites a published
# one:
#   - no release yet:     create a draft with every asset, then publish it;
#   - draft (an earlier run stopped part-way): replace its assets and notes,
#                          remove stray assets, then publish it;
#   - already published:   verify every expected asset is present and change
#                          nothing. Rebuilt binaries are not bit-identical, so
#                          a published release is never replaced.
#
# Usage: publish.sh <tag> <title> <dir> <notes-file>
# Requires GH_TOKEN with contents: write and GH_REPO (owner/name).
set -euo pipefail
# sort and comm must agree on ordering.
export LC_ALL=C

tag="$1"
title="$2"
dir="$3"
notes="$4"

mapfile -t assets < <(find "$dir" -mindepth 1 -maxdepth 1 -type f | sort)
((${#assets[@]} > 0)) || { echo "::error::no assets in $dir" >&2; exit 1; }
names="$(for asset in "${assets[@]}"; do basename "$asset"; done | sort)"

if ! state="$(gh release view "$tag" --json isDraft,assets 2>/dev/null)"; then
  gh release create "$tag" "${assets[@]}" --verify-tag --draft --prerelease \
    --title "$title" --notes-file "$notes"
  gh release edit "$tag" --draft=false
  echo "Published $tag with ${#assets[@]} assets"
  exit 0
fi

existing="$(jq -r '.assets[].name' <<<"$state" | sort)"
if [[ "$(jq -r '.isDraft' <<<"$state")" == false ]]; then
  missing="$(comm -23 <(echo "$names") <(echo "$existing"))"
  if [[ -n "$missing" ]]; then
    echo "::error::$tag is already published but lacks assets; fix it manually:" >&2
    echo "$missing" >&2
    exit 1
  fi
  echo "$tag is already published with every asset; left unchanged"
  exit 0
fi

echo "Completing draft release $tag from an earlier run"
gh release upload "$tag" "${assets[@]}" --clobber
comm -13 <(echo "$names") <(echo "$existing") | while read -r stray; do
  [[ -n "$stray" ]] && gh release delete-asset "$tag" "$stray" --yes
done
gh release edit "$tag" --title "$title" --notes-file "$notes" --prerelease --draft=false
echo "Published $tag with ${#assets[@]} assets"
