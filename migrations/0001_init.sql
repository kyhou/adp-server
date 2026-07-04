-- Purchases: one row per confirmed purchase. UNIQUE constraints on order_id and
-- payment_proof_event_id are what enforce NIP-102 idempotency and ADP-01 replay
-- protection at the storage layer, per ADP-01.md's Reference Implementation Checklist.
CREATE TABLE IF NOT EXISTS purchases (
    id                      INTEGER PRIMARY KEY AUTOINCREMENT,
    order_id                TEXT NOT NULL UNIQUE,
    payment_proof_event_id  TEXT NOT NULL UNIQUE,
    buyer_pubkey            TEXT NOT NULL,
    game_coordinate         TEXT NOT NULL,
    receipt_event_id        TEXT NOT NULL,
    created_at              INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_purchases_buyer ON purchases(buyer_pubkey);
CREATE INDEX IF NOT EXISTS idx_purchases_coordinate ON purchases(game_coordinate);

-- Short-lived tokens issued by /purchase/confirm for the fast-path ownership check
-- ("Path A") in GET /game/:game_coordinate. Portable cross-server downloads use
-- "Path B" (relay receipt query) instead and never touch this table.
CREATE TABLE IF NOT EXISTS download_tokens (
    token            TEXT PRIMARY KEY,
    game_coordinate  TEXT NOT NULL,
    buyer_pubkey     TEXT NOT NULL,
    expires_at       INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_download_tokens_coordinate ON download_tokens(game_coordinate);

-- Uploaded game archives, indexed by file_hash (matches the listing's file_hash tag).
CREATE TABLE IF NOT EXISTS files (
    file_hash        TEXT PRIMARY KEY,
    game_coordinate  TEXT NOT NULL,
    file_path        TEXT NOT NULL,
    uploaded_at      INTEGER NOT NULL
);
