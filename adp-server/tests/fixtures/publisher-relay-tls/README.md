# Publisher relay TLS test fixtures

Test-only certificates for `adp-server/tests/multi_relay_freshness_tests.rs`
(gap S8). Not used by the server at runtime; the private keys protect nothing.

- `ca.cert.der`: test CA that the test client trusts.
- `outbox.cert.der` / `outbox.key.der`: server certificate for
  `outbox.example.com`, signed by the test CA.
- `other.cert.der` / `other.key.der`: server certificate for
  `other.example.com`, used to show hostname verification still applies when
  the connection is pinned to a validated IP.

Generated with OpenSSL (P-256, 100-year validity, DER encoded).
