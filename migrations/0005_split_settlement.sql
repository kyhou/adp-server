-- Experimental ADP split settlement v1. Legacy purchase tables remain intact.
CREATE TABLE commercial_terms (
    root_event_id TEXT PRIMARY KEY,
    operator_pubkey TEXT NOT NULL,
    event_json TEXT NOT NULL,
    fee_json TEXT NOT NULL,
    payment_destination_encrypted BLOB NOT NULL,
    issued_at INTEGER NOT NULL,
    withdrawn_at INTEGER,
    withdrawal_event_id TEXT UNIQUE
);
CREATE INDEX commercial_terms_operator ON commercial_terms(operator_pubkey, issued_at);

CREATE TABLE purchase_quotes (
    quote_id TEXT PRIMARY KEY,
    quote_event_id TEXT NOT NULL UNIQUE,
    quote_event_json TEXT NOT NULL,
    buyer_pubkey TEXT NOT NULL,
    listing_coordinate TEXT NOT NULL,
    listing_event_id TEXT NOT NULL,
    processor_pubkey TEXT NOT NULL,
    gross_msat INTEGER NOT NULL CHECK (gross_msat > 0),
    state TEXT NOT NULL CHECK (state IN ('draft','preflighting','active','failed','settled')),
    issued_at INTEGER NOT NULL,
    payment_expires_at INTEGER NOT NULL,
    confirmation_expires_at INTEGER NOT NULL,
    failure_at INTEGER,
    receipt_event_id TEXT UNIQUE,
    receipt_event_json TEXT,
    receipt_publish_attempts INTEGER NOT NULL DEFAULT 0,
    CHECK (payment_expires_at = issued_at + 1800),
    CHECK (confirmation_expires_at = issued_at + 86400)
);

CREATE TABLE quote_terms (
    quote_id TEXT NOT NULL REFERENCES purchase_quotes(quote_id),
    operator_pubkey TEXT NOT NULL,
    terms_event_id TEXT NOT NULL REFERENCES commercial_terms(root_event_id),
    PRIMARY KEY (quote_id, operator_pubkey)
);

CREATE TABLE payment_legs (
    quote_id TEXT NOT NULL REFERENCES purchase_quotes(quote_id),
    leg_id TEXT NOT NULL,
    leg_order INTEGER NOT NULL CHECK (leg_order >= 0),
    role TEXT NOT NULL CHECK (role IN ('developer','operator')),
    recipient_pubkey TEXT NOT NULL,
    destination_encrypted BLOB NOT NULL,
    amount_msat INTEGER NOT NULL CHECK (amount_msat > 0),
    accepted_terms_id TEXT,
    disclosed_at INTEGER,
    settled_at INTEGER,
    PRIMARY KEY (quote_id, leg_id),
    UNIQUE (quote_id, leg_order)
);

CREATE TABLE invoice_attempts (
    attempt_id TEXT PRIMARY KEY,
    quote_id TEXT NOT NULL,
    leg_id TEXT NOT NULL,
    bolt11_encrypted BLOB NOT NULL,
    lnurl_origin TEXT NOT NULL,
    callback TEXT NOT NULL,
    lsp_pubkey TEXT NOT NULL,
    payment_hash TEXT NOT NULL UNIQUE,
    amount_msat INTEGER NOT NULL CHECK (amount_msat > 0),
    expires_at INTEGER NOT NULL,
    disclosed_at INTEGER,
    proof_type TEXT,
    proof_reference TEXT UNIQUE,
    proof_encrypted BLOB,
    settled_at INTEGER,
    FOREIGN KEY (quote_id, leg_id) REFERENCES payment_legs(quote_id, leg_id)
);

CREATE TABLE refund_destinations (
    destination_id TEXT PRIMARY KEY,
    quote_id TEXT NOT NULL REFERENCES purchase_quotes(quote_id),
    commitment TEXT NOT NULL,
    destination_encrypted BLOB NOT NULL,
    min_sendable_msat INTEGER NOT NULL,
    max_sendable_msat INTEGER NOT NULL,
    authorized_at INTEGER NOT NULL,
    replaced_at INTEGER
);
CREATE INDEX refund_destinations_quote ON refund_destinations(quote_id, authorized_at);

CREATE TABLE recovery_claims (
    claim_event_id TEXT PRIMARY KEY,
    quote_id TEXT NOT NULL REFERENCES purchase_quotes(quote_id),
    claimant_pubkey TEXT NOT NULL,
    claimant_role TEXT NOT NULL CHECK (claimant_role IN ('processor','buyer')),
    event_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE (quote_id, claimant_role)
);

CREATE TABLE refund_obligations (
    quote_id TEXT NOT NULL,
    leg_id TEXT NOT NULL,
    principal_msat INTEGER NOT NULL CHECK (principal_msat > 0),
    state TEXT NOT NULL CHECK (state IN ('refund_owed','initiated','verified','overdue','disputed')),
    due_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (quote_id, leg_id),
    FOREIGN KEY (quote_id, leg_id) REFERENCES payment_legs(quote_id, leg_id)
);

CREATE TABLE refund_attempts (
    attempt_id TEXT PRIMARY KEY,
    quote_id TEXT NOT NULL,
    leg_id TEXT NOT NULL,
    destination_id TEXT NOT NULL REFERENCES refund_destinations(destination_id),
    amount_msat INTEGER NOT NULL CHECK (amount_msat > 0),
    proof_reference TEXT UNIQUE,
    proof_encrypted BLOB,
    created_at INTEGER NOT NULL,
    verified_at INTEGER,
    FOREIGN KEY (quote_id, leg_id) REFERENCES refund_obligations(quote_id, leg_id)
);
