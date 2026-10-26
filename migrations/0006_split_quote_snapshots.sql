ALTER TABLE purchase_quotes ADD COLUMN listing_event_json TEXT;
ALTER TABLE purchase_quotes ADD COLUMN quote_plaintext_encrypted BLOB;

CREATE UNIQUE INDEX invoice_attempts_proof_identity
    ON invoice_attempts(proof_type, proof_reference)
    WHERE proof_reference IS NOT NULL;

ALTER TABLE refund_attempts ADD COLUMN bolt11_encrypted BLOB;
ALTER TABLE refund_attempts ADD COLUMN lnurl_origin TEXT;
ALTER TABLE refund_attempts ADD COLUMN callback TEXT;
ALTER TABLE refund_attempts ADD COLUMN lsp_pubkey TEXT;
ALTER TABLE refund_attempts ADD COLUMN payment_hash TEXT;
ALTER TABLE refund_attempts ADD COLUMN expires_at INTEGER;

CREATE UNIQUE INDEX refund_attempts_payment_hash
    ON refund_attempts(payment_hash)
    WHERE payment_hash IS NOT NULL;
