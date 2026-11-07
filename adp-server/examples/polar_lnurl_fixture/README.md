# Polar LNURL-pay fixture (regtest only)

Standalone Cargo example, not a production ADP route. One loopback-only HTTP
listener serves Alice (refunds), Bob (developer), Carol (operator A), and Dave
(operator B). It **creates real unpaid LND invoices**, never pays them, never
publishes Nostr events, and never changes wallets, channels, ADP configuration,
or database state. Invoices remain in the corresponding LND invoice database.

## Configuration

Run commands from the repository root. The checked-in `config.example.json`
contains no identities or credentials. REST ports match Polar's current
`polar-n1-{alice,bob,carol,dave}` containers; adjust a private copy if needed.
No `.env` file is loaded by this example.

Each recipient config names four environment variables:

| Config field | Environment variable value |
|---|---|
| `tls_cert_path_env` | Path to that node's PEM `tls.cert` |
| `macaroon_path_env` | Path to a **binary invoice-only macaroon**, not hex text |
| `nostr_pubkey_env` | Existing 64-hex Nostr public key; never an `nsec` |
| `node_pubkey_env` | That LND node's compressed public `identity_pubkey` |

Nostr pubkeys are the public identities you intentionally associate with the
fixture recipients. They are not LND's 33-byte node pubkeys. The fixture advertises
`allowsNostr:false`: it does not sign zap receipts and needs no Nostr private keys.
ADP still requires `nostrPubkey` metadata even for its BOLT11/preimage flow.

### Find connection information

In Polar, select each node → **Connect** to find its REST address, TLS certificate
location, and macaroon directory. `docker ps` shows published REST ports. Obtain
the public node ID without displaying credentials:

```bash
for node in alice bob carol dave; do
  docker exec "polar-n1-$node" \
    lncli --lnddir=/home/lnd/.lnd --network=regtest getinfo |
    jq '{alias, identity_pubkey, chains}'
done
```

TLS certificate verification remains enabled, including hostname verification.
Use a REST hostname/IP covered by the node's certificate. No insecure TLS bypass
is available. LND upstream connections must be HTTPS, redirects and proxy use
are disabled, and requests have a 10-second timeout.

This example explicitly uses native TLS (OpenSSL on Linux): Polar's self-signed
LND certificates have `CA:TRUE`, which Rustls/webpki rejects with
`CaUsedAsEndEntity` even when the certificate is trusted. Native TLS still checks
certificate trust, validity, and hostname; no verification bypass is used. Its
dependencies are dev-only and do not enable native TLS in production builds.
On Linux, building the example requires OpenSSL development libraries and
`pkg-config`.

### Least-privilege macaroons

Use a dedicated macaroon with only `invoices:write`; the fixture sends **only**
`POST /v1/invoices`. Do not use `admin.macaroon`. Polar's standard invoice
macaroon can include read permissions that this fixture does not need.

For example, an operator may bake a dedicated file **inside each container**:

```bash
for node in alice bob carol dave; do
  docker exec --user lnd "polar-n1-$node" \
    lncli --lnddir=/home/lnd/.lnd --network=regtest bakemacaroon \
    --save_to=/home/lnd/.lnd/lnurl-fixture.macaroon invoices:write >/dev/null
  docker exec --user lnd "polar-n1-$node" \
    chmod 600 /home/lnd/.lnd/lnurl-fixture.macaroon
done
```

This command creates credentials; it is not needed for automated tests and is
not run by the fixture. The mounted Polar volume makes those files accessible
from the host. Keep them outside the checkout, owner-readable only. Do not paste
macaroons into config files, command arguments, HTTP URLs, or logs. The bridge
does not inspect/enforce macaroon permissions; provision the stated scope.

### Set external environment

Set `POLAR_LND_ROOT` to the externally located directory containing the four
node directories (the host directory mounted as each node's `/home/lnd/.lnd`).
Set the four Nostr public key variables to your **existing actual** identities.
No identities are generated here.

```bash
# Supply these values securely in your shell or private environment file:
# POLAR_LND_ROOT, POLAR_ALICE_NOSTR_PUBKEY, POLAR_BOB_NOSTR_PUBKEY,
# POLAR_CAROL_NOSTR_PUBKEY, POLAR_DAVE_NOSTR_PUBKEY
: "${POLAR_LND_ROOT:?set the external Polar LND volume directory}"
: "${POLAR_ALICE_NOSTR_PUBKEY:?set existing public key}"
: "${POLAR_BOB_NOSTR_PUBKEY:?set existing public key}"
: "${POLAR_CAROL_NOSTR_PUBKEY:?set existing public key}"
: "${POLAR_DAVE_NOSTR_PUBKEY:?set existing public key}"
export POLAR_ALICE_NOSTR_PUBKEY POLAR_BOB_NOSTR_PUBKEY
export POLAR_CAROL_NOSTR_PUBKEY POLAR_DAVE_NOSTR_PUBKEY

for node in alice bob carol dave; do
  upper="${node^^}"
  export "POLAR_${upper}_TLS_CERT=$POLAR_LND_ROOT/$node/tls.cert"
  export "POLAR_${upper}_INVOICE_MACAROON=$POLAR_LND_ROOT/$node/lnurl-fixture.macaroon"
  pubkey="$(docker exec "polar-n1-$node" \
    lncli --lnddir=/home/lnd/.lnd --network=regtest getinfo |
    jq -er .identity_pubkey)"
  export "POLAR_${upper}_NODE_PUBKEY=$pubkey"
done
```

Defaults: bind `127.0.0.1:18080`, minimum `1000` msat, maximum `100000000`
msat, expiry `600` seconds per recipient. Bounds and expiry are independently
configurable. Minimum must be positive; maximum must fit SQLite/LND signed
integer limits; expiry must be `60..=3600` seconds, comfortably below ADP's
24-hour confirmation deadline. The fixture validates the returned invoice's
regtest network, signer, exact msat amount, metadata hash, and expiry.

Only literal loopback bind addresses are permitted. There is no public HTTP
listener option, host-header callback construction, arbitrary upstream routing,
payment API, or administrative proxy. Untrusted local processes can still create
unpaid invoices: keep this fixture stopped when unused. It is not hardened for
production or multi-user hosts.

## Start

```bash
cargo run --locked -p adp-server --example polar_lnurl_fixture -- \
  adp-server/examples/polar_lnurl_fixture/config.example.json
```

Stop with Ctrl-C. To change ports/limits, pass a private JSON config stored
outside the checkout instead. Never commit credentials or a private environment
file. Existing `.env` values are untouched.

## Verify metadata (no invoices created)

```bash
for node in alice bob carol dave; do
  curl --fail --silent --show-error \
    "http://127.0.0.1:18080/.well-known/lnurlp/$node" |
    jq -e '.tag == "payRequest" and .minSendable == 1000 and
           .maxSendable == 100000000 and .allowsNostr == false and
           (.nostrPubkey | length == 64)'
done
```

Endpoints can be supplied to ADP as either:

```text
bob@127.0.0.1:18080
http://127.0.0.1:18080/.well-known/lnurlp/bob
```

Use the equivalent Alice/Carol/Dave addresses as appropriate.

## Manual Polar smoke test (creates four unpaid invoices; no payments)

With the fixture running, run in Bash:

```bash
set -euo pipefail
for node in alice bob carol dave; do
  response="$(curl --fail --silent --show-error \
    "http://127.0.0.1:18080/lnurlp/$node/callback?amount=12001")"
  invoice="$(jq -er .pr <<<"$response")"
  expected="$(docker exec "polar-n1-$node" \
    lncli --lnddir=/home/lnd/.lnd --network=regtest getinfo |
    jq -er .identity_pubkey)"
  docker exec "polar-n1-$node" \
    lncli --lnddir=/home/lnd/.lnd --network=regtest decodepayreq \
    --pay_req="$invoice" |
    jq -e --arg expected "$expected" \
      '.destination == $expected and .num_msat == "12001" and
       .expiry == "600" and
       ((.timestamp | tonumber) + (.expiry | tonumber) > now)'
done
```

This intentionally checks fractional-satoshi precision (12.001 sats).
The fixture already verifies invoice signature and metadata commitment before
returning it. Never pay these smoke-test invoices.

Malformed/out-of-range amounts return HTTP 400 plus LNURL `status:ERROR`:

```bash
curl -i 'http://127.0.0.1:18080/lnurlp/bob/callback?amount=0'
curl -i 'http://127.0.0.1:18080/lnurlp/bob/callback?amount=1.5'
curl -i 'http://127.0.0.1:18080/lnurlp/bob/callback?amount=1000&amount=1000'
```

Unknown recipients return 404. Unavailable/rejecting LND nodes and invalid
upstream invoices return 502 with sanitized errors, never upstream bodies.
Callbacks reject unsupported parameters, including `nostr`, rather than
pretending to support zaps.

## Automated tests

```bash
cargo test --locked -p adp-server --example polar_lnurl_fixture
cargo check --locked -p adp-server --example polar_lnurl_fixture
```

Tests use mock REST nodes and synthetic signed regtest invoices only. They do
not read credentials, contact Polar, create real invoices, or pay anything.
The TLS regression test requires the `openssl` executable and generates an
ephemeral synthetic certificate/key in an automatically cleaned temporary directory.
It checks acceptance of a Polar-style `CA:TRUE` server certificate and rejection
of untrusted certificates and hostname mismatches.
One test serves the fixture locally and exercises ADP's existing
`request_exact_invoice` against it, still backed exclusively by a mock LND.

## Remaining compatibility blockers

- ADP Server must have `ADP_ALLOW_INSECURE_LNURL=true` for these HTTP destinations.
  No configuration is changed automatically.
- Arcadestr's refund `resolve_lud16` still hardcodes HTTPS. This HTTP-only fixture
  works with ADP's direct API but **does not fix Arcadestr desktop refund-address
  resolution**. TLS termination plus suitable non-loopback host/trust setup, or
  a separate Arcadestr change, is still required for that flow.
- Fee policies can produce amounts below the configured minimum. Choose bounds
  and fees deliberately; ADP rejects out-of-range legs and refund destinations
  that cannot accept every leg's principal.
- The fixture does not create/publish commercial terms, update listings, enable
  split mode, generate Nostr identities, or drive settlement/refund payments.
