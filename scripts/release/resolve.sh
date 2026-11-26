#!/usr/bin/env bash
# Resolves what a release run builds, from the full-history checkout in the
# current directory, and writes tag, sha, version and publish to
# $GITHUB_OUTPUT (or stdout).
#
# Environment:
#   EVENT     push | workflow_dispatch
#   PUSH_TAG  the pushed tag name (push events)
#   INPUT_REF the dispatch input: a version tag, or with DRY_RUN any ref
#   DRY_RUN   true | false (dispatch only)
#
# A publishing run requires an existing vX.Y.Z tag whose commit is on
# origin/main, and adp-server/Cargo.toml at that commit must declare version
# X.Y.Z. A dry run builds any ref and never publishes.
set -euo pipefail

fail() {
  echo "::error::$1" >&2
  exit 1
}

semver='^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$'
publish=true
case "${EVENT:?}" in
  push) ref="${PUSH_TAG:?}" ;;
  workflow_dispatch)
    ref="${INPUT_REF:?}"
    [[ "${DRY_RUN:-false}" == true ]] && publish=false
    ;;
  *) fail "unsupported event: $EVENT" ;;
esac

if [[ "$publish" == true ]]; then
  [[ "$ref" =~ $semver ]] || fail "release tags must be vX.Y.Z, got '$ref'"
  git rev-parse -q --verify "refs/tags/$ref" >/dev/null || fail "tag $ref does not exist"
  sha="$(git rev-parse "refs/tags/$ref^{commit}")"
  git merge-base --is-ancestor "$sha" origin/main ||
    fail "tag $ref points to $sha, which is not on origin/main"
else
  sha=""
  for candidate in "refs/tags/$ref" "origin/$ref" "$ref"; do
    if sha="$(git rev-parse -q --verify "$candidate^{commit}")"; then break; fi
    sha=""
  done
  [[ -n "$sha" ]] || fail "cannot resolve '$ref' to a commit"
fi

version="$(
  git show "$sha:adp-server/Cargo.toml" | python3 -c '
import sys, tomllib
print(tomllib.loads(sys.stdin.read())["package"]["version"])'
)"
if [[ "$publish" == true && "v$version" != "$ref" ]]; then
  fail "tag $ref does not match adp-server version $version"
fi

echo "Resolved $ref -> $sha (version $version, publish=$publish)" >&2
{
  echo "tag=$ref"
  echo "sha=$sha"
  echo "version=$version"
  echo "publish=$publish"
} >>"${GITHUB_OUTPUT:-/dev/stdout}"
