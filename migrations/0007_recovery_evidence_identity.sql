-- Retain legacy event IDs and allow new immutable evidence sets after late proof.
ALTER TABLE recovery_claims ADD COLUMN evidence_commitment TEXT;
CREATE UNIQUE INDEX recovery_claim_evidence_identity
ON recovery_claims(quote_id, claimant_role, evidence_commitment)
WHERE evidence_commitment IS NOT NULL;
