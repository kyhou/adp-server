CREATE TABLE IF NOT EXISTS fulfillment_keys (
    id                    INTEGER PRIMARY KEY AUTOINCREMENT,
    developer_pubkey      TEXT NOT NULL,
    scope                 TEXT NOT NULL DEFAULT '',
    fulfillment_pubkey    TEXT NOT NULL UNIQUE,
    encrypted_privkey     BLOB NOT NULL,
    encryption_nonce      BLOB NOT NULL,
    attestation_event_id  TEXT NOT NULL,
    valid_from            INTEGER NOT NULL,
    revoked_at            INTEGER,
    created_at            INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_fulfillment_keys_developer ON fulfillment_keys(developer_pubkey);
CREATE UNIQUE INDEX IF NOT EXISTS idx_fulfillment_keys_active_scope
    ON fulfillment_keys(developer_pubkey, scope)
    WHERE revoked_at IS NULL;
