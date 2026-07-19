CREATE TABLE entitlement_claims (
    buyer_pubkey               TEXT NOT NULL,
    game_coordinate            TEXT NOT NULL,
    campaign_id                TEXT NOT NULL,
    campaign_root_event_id     TEXT NOT NULL,
    grant_id                   TEXT NOT NULL,
    grant_event_id             TEXT NOT NULL,
    issuer_pubkey              TEXT NOT NULL,
    authorization_event_id     TEXT,
    grant_event_json           TEXT NOT NULL,
    state                      TEXT NOT NULL CHECK (state IN ('pending', 'published')),
    created_at                 INTEGER NOT NULL,
    last_attempt_at            INTEGER,
    published_at               INTEGER,
    UNIQUE (buyer_pubkey, game_coordinate, campaign_id),
    UNIQUE (grant_id),
    UNIQUE (grant_event_id),
    CHECK (
        (state = 'pending' AND published_at IS NULL)
        OR (state = 'published' AND published_at IS NOT NULL)
    )
);

CREATE INDEX idx_entitlement_claims_buyer_coordinate
    ON entitlement_claims(buyer_pubkey, game_coordinate);
CREATE INDEX idx_entitlement_claims_campaign_root
    ON entitlement_claims(campaign_root_event_id);
CREATE INDEX idx_entitlement_claims_authorization
    ON entitlement_claims(authorization_event_id);

ALTER TABLE purchases ADD COLUMN adp_profile_version INTEGER;
ALTER TABLE purchases ADD COLUMN authorization_event_id TEXT;
ALTER TABLE purchases ADD COLUMN receipt_issuer_pubkey TEXT;
ALTER TABLE purchases ADD COLUMN receipt_event_json TEXT;

ALTER TABLE download_tokens ADD COLUMN source_type TEXT;
ALTER TABLE download_tokens ADD COLUMN source_event_id TEXT;
