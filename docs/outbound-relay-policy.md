# Outbound relay security policy

**Status:** normative for this server. Implements ADP-01 §Relay selection
(gap S8) in `adp-server/src/publisher_relays.rs`.

ADP-01 says implementations SHOULD prefer a publisher's NIP-65 write relays when
reading that publisher's events. On a server this means connecting to URLs any
publisher can choose, which is a server-side request forgery (SSRF) risk: a
signed relay list could point the server at its own network. A correctly
signed `kind:10002` event proves who wrote the list, not that its URLs are safe,
so every publisher relay URL is treated as untrusted input.

## Operator control

The feature is **disabled by default**. With `ADP_PUBLISHER_RELAYS` unset or
`disabled`, the server uses only the operator-configured relays in `ADP_RELAYS`
and its behavior is unchanged. Set `ADP_PUBLISHER_RELAYS=enabled` to turn it on.
Any other value stops the server at startup.

## Rules

1. **URL validation.** Only `wss://` URLs with a host are accepted. URLs with
   embedded credentials, fragments, or that do not parse are rejected. HTTP
   redirects during the WebSocket handshake are never followed; a 3xx response
   fails the connection.
2. **Public destinations only.** A relay is rejected unless every address it
   uses is public. Rejected ranges include loopback, private (RFC 1918),
   carrier-grade NAT, link-local (including `169.254.169.254` cloud metadata),
   multicast, broadcast, reserved, benchmarking, documentation, and IETF
   special-use ranges; IPv6 unspecified, loopback, unique-local, link-local,
   site-local, multicast, documentation, Teredo, and 6to4. IPv4 embedded in
   IPv6 (`::ffff:0:0/96`, `64:ff9b::/96`) is judged by its IPv4 address. Only
   IPv6 global unicast (`2000::/3`) outside those exclusions is public.
   Unrecognized addresses are treated as non-public. IP literals in every
   spelling the URL parser accepts (decimal, hex, octal, bracketed IPv6) are
   checked the same way.
3. **DNS rebinding protection.** The hostname is resolved **once**. If any
   returned address is non-public, the whole relay is rejected. TCP then
   connects directly to one of those validated addresses, and TLS and the
   WebSocket handshake run over that socket with the original hostname, so the
   certificate is still verified against the hostname (webpki roots) and no
   second DNS answer can change the destination.
4. **Resource limits.** At most 5 relays per publisher and 25 publisher relays
   in total (oldest evicted first); at most 8 simultaneous connection attempts;
   5 seconds per attempt, covering DNS, TCP, TLS and handshake. Publisher relays
   never reconnect automatically: each is attempted once when added. Relay lists
   are cached for 10 minutes, for at most 1024 publishers. Proxied connection
   modes are refused.
5. **NIP-65 verification.** Only `kind:10002` events signed by the publisher
   and passing signature verification are used, newest first. Relays marked
   `write` or unmarked are used; `read` relays are not. Relay lists are read
   from operator-configured relays only.
6. **Operator control.** See above.
7. **Freshness isolation.** Publisher relays can add candidate events, such as
   a newer signed listing, which is then used. They never count toward
   completeness or the freshness policy: `FRESHNESS_MIN_COMPLETED_RELAYS`
   (see `docs/protocol-implementation-gaps.md`, S9) counts configured relays
   only.
8. **Failure handling.** An unavailable, slow, or rejected publisher relay is
   ignored. Operations succeed or fail exactly as the configured relays
   determine.

## Verification

Tests cover each rule:

- Unit tests in `adp-server/src/publisher_relays.rs`: URL validation, IP-literal
  spellings, address classification (including IPv4-mapped IPv6, NAT64, 6to4,
  Teredo), mixed and private DNS answers (never dialed), DNS rebinding (one
  resolution, validated address dialed), the concurrent-attempt limit, the
  attempt timeout, NIP-65 parsing (forged, foreign, read-only, invalid
  entries), the per-publisher cap, and the global pool cap.
- Integration tests in `adp-server/tests/multi_relay_freshness_tests.rs`, over
  real TLS with test certificates (`tests/fixtures/publisher-relay-tls`): a
  listing found only on a publisher relay, a newer listing revealed there,
  freshness isolation, fallback when the publisher relay is unreachable (one
  attempt only), unsafe list entries never dialed, a certificate for another
  hostname refused, redirects not followed, and no use when disabled.
