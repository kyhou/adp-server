#!/usr/bin/env bash
# Verifies and packages the release binaries for one Linux architecture.
#
# Usage: package.sh <x86_64|arm64> <version> <out-dir>
# Run from the repository root after
#   cargo build --release --locked -p adp-server --bins
#
# Checks: ELF architecture, dynamic libraries limited to the C runtime (no
# OpenSSL or SQLite: TLS is rustls and SQLite is bundled), and no glibc symbol
# newer than $GLIBC_BASELINE (default 2.35, Ubuntu 22.04). Writes
# <out-dir>/adp-server-<version>-linux-<arch>.tar.gz with a fixed allowlist of
# files, so no keys, configuration, databases or fixtures can be included.
set -euo pipefail

arch="$1"
version="$2"
out="$3"
baseline="${GLIBC_BASELINE:-2.35}"
name="adp-server-$version-linux-$arch"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

fail() {
  echo "::error::$1" >&2
  exit 1
}

allowed_libraries='^(libc|libm|libgcc_s|libdl|libpthread|librt)\.so\.[0-9]+$|^ld-linux(-x86-64|-aarch64)\.so\.[0-9]+$'
for binary in adp-server adp-refunds; do
  path="target/release/$binary"
  [[ -x "$path" ]] || fail "$path is missing; build it first"
  python3 "$here/check-arch.py" "$path" "$arch"
  while read -r library; do
    [[ "$library" =~ $allowed_libraries ]] || fail "$binary links unexpected library $library"
  done < <(readelf -d "$path" | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p')
  newest="$(objdump -T "$path" | grep -o 'GLIBC_[0-9][0-9.]*' | sed 's/GLIBC_//' | sort -uV | tail -1)"
  [[ "$(printf '%s\n%s\n' "$newest" "$baseline" | sort -V | tail -1)" == "$baseline" ]] ||
    fail "$binary requires glibc $newest, newer than the $baseline baseline"
  echo "$binary: needs glibc >= $newest; libraries: $(readelf -d "$path" | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p' | tr '\n' ' ')"
done

staging="$(mktemp -d)"
trap 'rm -rf "$staging"' EXIT
root="$staging/$name"
install -d "$root/docs"
install -m 0755 target/release/adp-server target/release/adp-refunds "$root/"
install -m 0644 README.md "$root/README.md"
install -m 0644 .env.example "$root/adp-server.env.example"
install -m 0644 docs/releasing.md "$root/docs/releasing.md"
install -m 0644 docs/outbound-relay-policy.md "$root/docs/outbound-relay-policy.md"

expected="$(printf '%s\n' \
  "$name/" "$name/README.md" "$name/adp-refunds" "$name/adp-server" \
  "$name/adp-server.env.example" "$name/docs/" "$name/docs/outbound-relay-policy.md" \
  "$name/docs/releasing.md")"
# The example environment must not carry credentials.
if grep -Eq '^(ADP_OPERATOR_NSEC|ADP_MASTER_KEY)=.+' "$root/adp-server.env.example"; then
  fail ".env.example contains a credential value"
fi

mkdir -p "$out"
archive="$out/$name.tar.gz"
epoch="${SOURCE_DATE_EPOCH:-$(git log -1 --format=%ct)}"
tar --sort=name --owner=0 --group=0 --numeric-owner --mtime="@$epoch" \
  -C "$staging" -cf - "$name" | gzip -n -9 >"$archive"

listing="$(tar -tzf "$archive" | LC_ALL=C sort)"
[[ "$listing" == "$(LC_ALL=C sort <<<"$expected")" ]] || {
  diff <(LC_ALL=C sort <<<"$expected") <(echo "$listing") >&2 || true
  fail "archive contents differ from the allowlist"
}
echo "Packaged $archive"
