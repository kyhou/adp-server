# Releases

adp-server releases are GitHub pre-releases with prebuilt Linux binaries,
built by `.github/workflows/release.yml` from a `vX.Y.Z` tag. They contain
only this repository; building never needs the Arcadestr repository. The
release version is the `adp-server` crate version, which is independent of the
ADP protocol version the server implements (`adp_version` in
`/.well-known/adp`).

## Running a release

### Supported systems

| Asset | Architecture | Minimum system |
|---|---|---|
| `adp-server-X.Y.Z-linux-x86_64.tar.gz` | x86-64 | glibc 2.35 (Ubuntu 22.04, Debian 12, RHEL 10, or newer) |
| `adp-server-X.Y.Z-linux-arm64.tar.gz` | ARM64 (aarch64) | glibc 2.35 |

The binaries link only the C runtime (`libc`, `libm`, `libgcc_s`). TLS uses
rustls and SQLite is built in, so no OpenSSL or SQLite package is needed. musl
systems such as Alpine are not supported.

### Verify the download

Download the archive and `SHA256SUMS` from the release, then:

```bash
sha256sum --check --ignore-missing SHA256SUMS
```

`--ignore-missing` checks only the files you downloaded. A mismatch means the
download is corrupt or not the published file; do not run it.

### Install and run

Each archive contains:

| File | Purpose |
|---|---|
| `adp-server` | The ADP-01 HTTP server |
| `adp-refunds` | Operator CLI for partial-settlement refunds (`adp-refunds list`) |
| `adp-server.env.example` | Configuration template with empty credentials |
| `README.md` | Project overview and configuration notes |
| `docs/outbound-relay-policy.md`, `docs/releasing.md` | Relay policy and this document |

```bash
tar -xzf adp-server-X.Y.Z-linux-x86_64.tar.gz
sudo install -m 0755 adp-server-X.Y.Z-linux-x86_64/adp-server \
  adp-server-X.Y.Z-linux-x86_64/adp-refunds /usr/local/bin/
```

Create the environment file from `adp-server.env.example` outside any synced
or shared directory, owner-only (`0600`). Set `ADP_OPERATOR_NSEC` and
`ADP_MASTER_KEY`, at least two `ADP_RELAYS`, and absolute `ADP_DB_PATH` and
`ADP_FILES_DIR`. The server reads its process environment, not the file, so
load it with your service manager. For example, with systemd:

```ini
[Service]
EnvironmentFile=/etc/adp-server/server.env
ExecStart=/usr/local/bin/adp-server
User=adp-server
WorkingDirectory=/var/lib/adp-server
```

Releases contain no keys, configuration, databases or fixtures. Never reuse
another instance's keys or database paths.

## Publishing a release

1. Set `version` in `adp-server/Cargo.toml` to `X.Y.Z`, run
   `cargo build --locked` so `Cargo.lock` agrees, and commit to `main`.
2. Tag that commit and push the tag to Forgejo (`origin`):

   ```bash
   git tag -a vX.Y.Z -m "adp-server vX.Y.Z"
   git push origin vX.Y.Z
   ```

3. The Forgejo push mirror forwards the tag to GitHub, where the tag push
   starts the release workflow. Confirm the tag arrived:

   ```bash
   git ls-remote https://github.com/kyhou/adp-server.git refs/tags/vX.Y.Z
   ```

   If it is missing, run "Synchronize now" for the push mirror in the Forgejo
   repository settings. Until the mirror delivers the tag, nothing is built.

The workflow refuses tags that are not `vX.Y.Z`, that point outside `main`,
or whose version differs from `adp-server/Cargo.toml`. Tags are never moved or
reused; fix a bad release with a new version.

### What the workflow does

1. **resolve** finds the tag's exact commit and checks the version.
2. **build** runs natively on `ubuntu-22.04` (x86-64) and `ubuntu-22.04-arm`
   (ARM64). It runs `cargo build --release --locked`, then checks each
   binary's architecture, linked libraries and glibc baseline. It packages a
   fixed allowlist of files and starts the packaged server against two local
   test relays with throwaway credentials (`scripts/release/smoke.py`).
3. **assemble** requires both archives, writes `SHA256SUMS` and generates
   notes from the commits since the previous tag.
4. **publish** is the only job with write permission. It creates one draft
   release with every asset, then publishes it as a pre-release.

A failed build stops the run before anything is published.

### Retrying

Re-run the failed jobs from the Actions page, or run the workflow manually
with the same tag. Publication is idempotent:

- no release yet: it is created;
- a draft left by an interrupted run: its assets and notes are replaced and it
  is published;
- already published: it is left unchanged (rebuilt binaries are not
  bit-identical, so published assets are never replaced).

### Dry run

Run the workflow manually with **dry run** enabled and any branch, tag or
commit. It builds, checks and assembles everything and keeps the result as a
workflow artifact named `release`, but never publishes.
