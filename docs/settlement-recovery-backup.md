# Settlement recovery backup baseline

This source checkpoint preserves ongoing development; it is not a production
readiness or live-settlement certification. Full acceptance testing is deferred.

## Arcadestr dependencies

Arcadestr's combined Gate 3/4 work consumes authenticated recipient refund history
at `POST /settlement/refunds` and buyer-scoped history at
`POST /settlement/:quote_id/refunds`. These operations cannot prepare invoices or
dispatch payments, but can materialize elapsed deadlines in server storage.

Restart recovery reuses the persisted signed receipt and issues a download token
for the original buyer. Refund retries reuse the original persisted attempt and
accept repeated valid confirmation. Buyer recovery ciphertext must match the
submitted evidence rather than merely carrying a valid unrelated signature.

Migrations 0007 and 0008 belong together: recovery identity is stable per quote,
claimant role and evidence commitment, while changed evidence can append another
immutable event. Existing event bytes and IDs are preserved. This is not a
claim that upgrade and rollback compatibility have been exhaustively tested.

The regtest examples additionally depend on the injectable settlement clock,
upload-capacity configuration field and separate Polar LNURL fixture. These are
development-tool dependencies, not permission to run live fixture operations.

## Deferred issues and compatibility boundaries

- Refund preparation currently reuses a persisted invoice even after expiry.
  Expiry handling and interaction with buyer-authorized destination replacement
  require a follow-up policy review. Do not infer that replacement or repayment
  is safe merely because an invoice expired.
- Concurrency tests establish one persisted refund attempt. They do not establish
  that concurrent HTTP requests create only one upstream LNURL invoice.
- Refund history is capped at 500 rows without pagination. Review large-account
  completeness and deadline-materialization behavior before production use.
- Protocol wording about deduplication by purchase identity should be reconciled
  with immutable per-evidence recovery versions.
- The separate download fix enforces exact raw-target authentication on downloads;
  the shared NIP-98 verifier and other handlers have not been comprehensively
  migrated to that contract. Broad specification edits remain outside this backup.
- Canonical-hex requirements and relay positive-evidence/freshness policy changes
  are independent work and are not prerequisites certified by this checkpoint.

Use existing synthetic route/storage tests for further validation. Do not use
historical paid regtest commands, credentials, databases or reservations to
validate this source backup without separate explicit operational authorization.
