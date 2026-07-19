use adp_core::{parse_entitlement_event, parse_receipt_event};
use nostr_sdk::Event;
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::{Row, SqlitePool};
use thiserror::Error;

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_secs() as i64
}

#[derive(Clone)]
pub struct Storage {
    pool: SqlitePool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredFile {
    pub file_hash: String,
    pub file_path: String,
}

#[derive(Debug, Error)]
pub enum PersistenceError {
    #[error(transparent)]
    Sql(#[from] sqlx::Error),
    #[error("invalid stored event JSON: {0}")]
    EventJson(#[from] serde_json::Error),
    #[error("persistence corruption: {0}")]
    Corrupt(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimState {
    Pending,
    Published,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkClaimPublishedResult {
    Updated,
    AlreadyPublishedIdentical,
    Conflict,
    Missing,
}

#[derive(Debug, Clone)]
pub struct StoredEntitlementClaim {
    pub buyer_pubkey: String,
    pub game_coordinate: String,
    pub campaign_id: String,
    pub campaign_root_event_id: String,
    pub grant_id: String,
    pub grant_event_id: String,
    pub issuer_pubkey: String,
    pub authorization_event_id: Option<String>,
    pub grant_event_json: String,
    pub grant: Event,
    pub state: ClaimState,
    pub created_at: i64,
    pub last_attempt_at: Option<i64>,
    pub published_at: Option<i64>,
}

impl Storage {
    pub async fn connect(db_path: &str) -> Result<Self, sqlx::Error> {
        let url = format!("sqlite://{db_path}?mode=rwc");
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(&url)
            .await?;
        sqlx::migrate!("../migrations")
            .run(&pool)
            .await
            .map_err(|e| sqlx::Error::Migrate(Box::new(e)))?;
        Ok(Self { pool })
    }

    pub fn pool(&self) -> SqlitePool {
        self.pool.clone()
    }

    /// Records a confirmed purchase. `order_id` and `payment_proof_key` both carry
    /// UNIQUE constraints (see migrations/0001_init.sql) — this is what enforces
    /// NIP-102's idempotency requirement and ADP-01's replay protection.
    pub async fn record_purchase(
        &self,
        order_id: &str,
        payment_proof_key: &str,
        buyer_pubkey: &str,
        game_coordinate: &str,
        receipt_event_id: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO purchases
                (order_id, payment_proof_event_id, buyer_pubkey, game_coordinate, receipt_event_id, created_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(order_id)
        .bind(payment_proof_key)
        .bind(buyer_pubkey)
        .bind(game_coordinate)
        .bind(receipt_event_id)
        .bind(now_secs())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn record_purchase_v3(
        &self,
        order_id: &str,
        payment_proof_key: &str,
        buyer_pubkey: &str,
        game_coordinate: &str,
        receipt: &Event,
    ) -> Result<(), PersistenceError> {
        let parsed = parse_receipt_event(receipt)
            .map_err(|e| PersistenceError::Corrupt(format!("invalid receipt: {e}")))?;
        if parsed.order_id != order_id
            || parsed.buyer_pubkey.to_hex() != buyer_pubkey
            || parsed.coordinate != game_coordinate
        {
            return Err(PersistenceError::Corrupt(
                "receipt columns disagree with signed event".into(),
            ));
        }
        let event_json = serde_json::to_string(receipt)?;
        sqlx::query(
            "INSERT INTO purchases (
                order_id, payment_proof_event_id, buyer_pubkey, game_coordinate,
                receipt_event_id, created_at, adp_profile_version,
                authorization_event_id, receipt_issuer_pubkey, receipt_event_json
             ) VALUES (?, ?, ?, ?, ?, ?, 3, ?, ?, ?)",
        )
        .bind(order_id)
        .bind(payment_proof_key)
        .bind(buyer_pubkey)
        .bind(game_coordinate)
        .bind(receipt.id.to_hex())
        .bind(now_secs())
        .bind(parsed.authorization_event_id.map(|id| id.to_hex()))
        .bind(receipt.pubkey.to_hex())
        .bind(event_json)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn payment_proof_already_used(
        &self,
        payment_proof_key: &str,
    ) -> Result<bool, sqlx::Error> {
        let row: Option<(i64,)> =
            sqlx::query_as("SELECT 1 FROM purchases WHERE payment_proof_event_id = ? LIMIT 1")
                .bind(payment_proof_key)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.is_some())
    }

    pub async fn record_file(
        &self,
        file_hash: &str,
        game_coordinate: &str,
        file_path: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO files (file_hash, game_coordinate, file_path, uploaded_at)
             VALUES (?, ?, ?, ?)
             ON CONFLICT(file_hash) DO UPDATE SET
                game_coordinate = excluded.game_coordinate,
                file_path = excluded.file_path,
                uploaded_at = excluded.uploaded_at",
        )
        .bind(file_hash)
        .bind(game_coordinate)
        .bind(file_path)
        .bind(now_secs())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn issue_download_token(
        &self,
        token: &str,
        game_coordinate: &str,
        buyer_pubkey: &str,
        expires_at: i64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO download_tokens (token, game_coordinate, buyer_pubkey, expires_at)
             VALUES (?, ?, ?, ?)",
        )
        .bind(token)
        .bind(game_coordinate)
        .bind(buyer_pubkey)
        .bind(expires_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn issue_attributed_download_token(
        &self,
        token: &str,
        game_coordinate: &str,
        buyer_pubkey: &str,
        expires_at: i64,
        source_type: &str,
        source_event_id: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO download_tokens
                (token, game_coordinate, buyer_pubkey, expires_at, source_type, source_event_id)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(token)
        .bind(game_coordinate)
        .bind(buyer_pubkey)
        .bind(expires_at)
        .bind(source_type)
        .bind(source_event_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn find_entitlement_claim(
        &self,
        buyer_pubkey: &str,
        game_coordinate: &str,
        campaign_id: &str,
    ) -> Result<Option<StoredEntitlementClaim>, PersistenceError> {
        let row = sqlx::query(
            "SELECT buyer_pubkey, game_coordinate, campaign_id, campaign_root_event_id,
                    grant_id, grant_event_id, issuer_pubkey, authorization_event_id,
                    grant_event_json, state, created_at, last_attempt_at, published_at
             FROM entitlement_claims
             WHERE buyer_pubkey = ? AND game_coordinate = ? AND campaign_id = ?
             LIMIT 1",
        )
        .bind(buyer_pubkey)
        .bind(game_coordinate)
        .bind(campaign_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(Self::claim_from_row).transpose()
    }

    pub async fn insert_pending_entitlement_claim(
        &self,
        buyer_pubkey: &str,
        game_coordinate: &str,
        campaign_id: &str,
        campaign_root_event_id: &str,
        grant: &Event,
    ) -> Result<(StoredEntitlementClaim, bool), PersistenceError> {
        let parsed = parse_entitlement_event(grant)
            .map_err(|e| PersistenceError::Corrupt(format!("invalid grant: {e}")))?;
        if parsed.recipient.to_hex() != buyer_pubkey
            || parsed.coordinate != game_coordinate
            || parsed.source_event.map(|id| id.to_hex()).as_deref() != Some(campaign_root_event_id)
        {
            return Err(PersistenceError::Corrupt(
                "claim columns disagree with signed grant".into(),
            ));
        }
        let grant_json = serde_json::to_string(grant)?;
        let result = sqlx::query(
            "INSERT OR IGNORE INTO entitlement_claims (
                buyer_pubkey, game_coordinate, campaign_id, campaign_root_event_id,
                grant_id, grant_event_id, issuer_pubkey, authorization_event_id,
                grant_event_json, state, created_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'pending', ?)",
        )
        .bind(buyer_pubkey)
        .bind(game_coordinate)
        .bind(campaign_id)
        .bind(campaign_root_event_id)
        .bind(&parsed.grant_id)
        .bind(grant.id.to_hex())
        .bind(grant.pubkey.to_hex())
        .bind(parsed.authorization_event.map(|id| id.to_hex()))
        .bind(grant_json)
        .bind(grant.created_at.as_secs() as i64)
        .execute(&self.pool)
        .await?;
        let inserted = result.rows_affected() == 1;
        let stored = self
            .find_entitlement_claim(buyer_pubkey, game_coordinate, campaign_id)
            .await?
            .ok_or_else(|| {
                PersistenceError::Corrupt(
                    "claim uniqueness conflict did not identify an existing claim".into(),
                )
            })?;
        Ok((stored, inserted))
    }

    pub async fn record_claim_publish_attempt(
        &self,
        buyer_pubkey: &str,
        game_coordinate: &str,
        campaign_id: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE entitlement_claims SET last_attempt_at = ?
             WHERE buyer_pubkey = ? AND game_coordinate = ? AND campaign_id = ?",
        )
        .bind(now_secs())
        .bind(buyer_pubkey)
        .bind(game_coordinate)
        .bind(campaign_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn mark_claim_published_if_identical(
        &self,
        buyer_pubkey: &str,
        game_coordinate: &str,
        campaign_id: &str,
        expected_grant: &Event,
    ) -> Result<MarkClaimPublishedResult, PersistenceError> {
        let expected_json = serde_json::to_string(expected_grant)?;
        let result = sqlx::query(
            "UPDATE entitlement_claims
             SET state = 'published', published_at = ?
             WHERE buyer_pubkey = ? AND game_coordinate = ? AND campaign_id = ?
               AND state = 'pending' AND grant_event_id = ? AND grant_event_json = ?",
        )
        .bind(now_secs())
        .bind(buyer_pubkey)
        .bind(game_coordinate)
        .bind(campaign_id)
        .bind(expected_grant.id.to_hex())
        .bind(expected_json)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 1 {
            return Ok(MarkClaimPublishedResult::Updated);
        }
        let Some(stored) = self
            .find_entitlement_claim(buyer_pubkey, game_coordinate, campaign_id)
            .await?
        else {
            return Ok(MarkClaimPublishedResult::Missing);
        };
        if stored.state == ClaimState::Published && stored.grant == *expected_grant {
            Ok(MarkClaimPublishedResult::AlreadyPublishedIdentical)
        } else {
            Ok(MarkClaimPublishedResult::Conflict)
        }
    }

    fn claim_from_row(
        row: sqlx::sqlite::SqliteRow,
    ) -> Result<StoredEntitlementClaim, PersistenceError> {
        let grant_event_json: String = row.try_get("grant_event_json")?;
        let grant: Event = serde_json::from_str(&grant_event_json)?;
        let parsed = parse_entitlement_event(&grant)
            .map_err(|e| PersistenceError::Corrupt(format!("invalid stored grant: {e}")))?;
        let buyer_pubkey: String = row.try_get("buyer_pubkey")?;
        let game_coordinate: String = row.try_get("game_coordinate")?;
        let campaign_root_event_id: String = row.try_get("campaign_root_event_id")?;
        let grant_id: String = row.try_get("grant_id")?;
        let grant_event_id: String = row.try_get("grant_event_id")?;
        let issuer_pubkey: String = row.try_get("issuer_pubkey")?;
        let authorization_event_id: Option<String> = row.try_get("authorization_event_id")?;
        let created_at: i64 = row.try_get("created_at")?;
        let state_text: String = row.try_get("state")?;
        let published_at: Option<i64> = row.try_get("published_at")?;
        let state = match state_text.as_str() {
            "pending" if published_at.is_none() => ClaimState::Pending,
            "published" if published_at.is_some() => ClaimState::Published,
            _ => {
                return Err(PersistenceError::Corrupt(
                    "claim state and published_at disagree".into(),
                ))
            }
        };
        if parsed.grant_id != grant_id
            || grant.id.to_hex() != grant_event_id
            || parsed.recipient.to_hex() != buyer_pubkey
            || parsed.coordinate != game_coordinate
            || parsed.source_event.map(|id| id.to_hex()).as_deref()
                != Some(campaign_root_event_id.as_str())
            || parsed.authorization_event.map(|id| id.to_hex()) != authorization_event_id
            || grant.pubkey.to_hex() != issuer_pubkey
            || grant.created_at.as_secs() as i64 != created_at
        {
            return Err(PersistenceError::Corrupt(
                "claim columns disagree with signed event".into(),
            ));
        }
        Ok(StoredEntitlementClaim {
            buyer_pubkey,
            game_coordinate,
            campaign_id: row.try_get("campaign_id")?,
            campaign_root_event_id,
            grant_id,
            grant_event_id,
            issuer_pubkey,
            authorization_event_id,
            grant_event_json,
            grant,
            state,
            created_at,
            last_attempt_at: row.try_get("last_attempt_at")?,
            published_at,
        })
    }

    /// Resolves a download token to its buyer pubkey, scoped to `game_coordinate`,
    /// if the token exists and has not expired. This is the fast "Path A" ownership
    /// check in `GET /game/:game_coordinate` — no relay round trip required.
    pub async fn resolve_download_token(
        &self,
        token: &str,
        game_coordinate: &str,
    ) -> Result<Option<String>, sqlx::Error> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT buyer_pubkey FROM download_tokens
             WHERE token = ? AND game_coordinate = ? AND expires_at > ?
             LIMIT 1",
        )
        .bind(token)
        .bind(game_coordinate)
        .bind(now_secs())
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(pk,)| pk))
    }

    pub async fn file_path_for_hash(&self, file_hash: &str) -> Result<Option<String>, sqlx::Error> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT file_path FROM files WHERE file_hash = ? LIMIT 1")
                .bind(file_hash)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(|(p,)| p))
    }

    pub async fn stored_files(&self) -> Result<Vec<StoredFile>, sqlx::Error> {
        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT file_hash, file_path FROM files ORDER BY file_hash")
                .fetch_all(&self.pool)
                .await?;
        Ok(rows
            .into_iter()
            .map(|(file_hash, file_path)| StoredFile {
                file_hash,
                file_path,
            })
            .collect())
    }
}
