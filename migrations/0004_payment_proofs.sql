CREATE TABLE payment_proofs (
    proof_key TEXT PRIMARY KEY,
    order_id TEXT NOT NULL,
    FOREIGN KEY (order_id) REFERENCES purchases(order_id)
);

INSERT OR IGNORE INTO payment_proofs (proof_key, order_id)
SELECT payment_proof_event_id, order_id FROM purchases;

CREATE INDEX idx_payment_proofs_order ON payment_proofs(order_id);
