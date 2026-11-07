-- One immutable event per evidence set, not one event for the quote's lifetime.
-- Preserve every existing signed event and its ID, including legacy NULL evidence.
CREATE TABLE recovery_claims_versioned (
    claim_event_id TEXT PRIMARY KEY,
    quote_id TEXT NOT NULL REFERENCES purchase_quotes(quote_id),
    claimant_pubkey TEXT NOT NULL,
    claimant_role TEXT NOT NULL CHECK (claimant_role IN ('processor','buyer')),
    event_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    evidence_commitment TEXT
);
INSERT INTO recovery_claims_versioned
SELECT claim_event_id, quote_id, claimant_pubkey, claimant_role, event_json,
       created_at, evidence_commitment FROM recovery_claims;
DROP TABLE recovery_claims;
ALTER TABLE recovery_claims_versioned RENAME TO recovery_claims;
CREATE UNIQUE INDEX recovery_claim_evidence_identity
ON recovery_claims(quote_id, claimant_role, evidence_commitment)
WHERE evidence_commitment IS NOT NULL;
