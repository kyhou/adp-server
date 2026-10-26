use adp_core::{
    parse_entitlement_event, parse_receipt_event, CommercialTerms, QuoteState, SignedQuote,
    REFUND_DUE_SECS,
};
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

#[derive(Debug, Clone)]
pub struct NewSettlementTerm {
    pub event: Event,
    pub terms: CommercialTerms,
    pub encrypted_destination: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct NewSettlementAttempt {
    pub attempt_id: String,
    pub leg_id: String,
    pub encrypted_bolt11: Vec<u8>,
    pub lnurl_origin: String,
    pub callback: String,
    pub lsp_pubkey: String,
    pub payment_hash: String,
    pub amount_msat: u64,
    pub expires_at: u64,
}

#[derive(Debug, Clone)]
pub struct StoredSettlementQuote {
    pub quote_event: Event,
    pub listing_event: Event,
    pub encrypted_quote: Vec<u8>,
    pub buyer_pubkey: String,
    pub state: QuoteState,
    pub payment_expires_at: u64,
    pub confirmation_expires_at: u64,
    pub failure_at: Option<u64>,
    pub receipt: Option<Event>,
}

#[derive(Debug, Clone)]
pub struct StoredSettlementAttempt {
    pub attempt_id: String,
    pub leg_id: String,
    pub encrypted_bolt11: Vec<u8>,
    pub payment_hash: String,
    pub amount_msat: u64,
    pub expires_at: u64,
    pub disclosed_at: Option<u64>,
    pub settled_at: Option<u64>,
    pub proof_type: Option<String>,
    pub proof_reference: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementStatus {
    pub state: QuoteState,
    pub total_legs: u64,
    pub disclosed_legs: u64,
    pub settled_legs: u64,
    pub failure_at: Option<u64>,
    pub refund_due_at: Option<u64>,
    pub receipt_event_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RefundContext {
    pub leg_id: String,
    pub recipient_pubkey: String,
    pub buyer_pubkey: String,
    pub principal_msat: u64,
    pub destination_id: String,
    pub encrypted_destination: Vec<u8>,
    pub failure_at: u64,
}

#[derive(Debug, Clone)]
pub struct StoredRefundAttempt {
    pub attempt_id: String,
    pub leg_id: String,
    pub amount_msat: u64,
    pub encrypted_bolt11: Vec<u8>,
    pub payment_hash: String,
    pub expires_at: u64,
    pub verified_at: Option<u64>,
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
        payment_proof_keys: &[String],
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
        let primary_proof = payment_proof_keys
            .first()
            .ok_or_else(|| PersistenceError::Corrupt("purchase has no payment proof key".into()))?;
        let mut transaction = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO purchases (
                order_id, payment_proof_event_id, buyer_pubkey, game_coordinate,
                receipt_event_id, created_at, adp_profile_version,
                authorization_event_id, receipt_issuer_pubkey, receipt_event_json
             ) VALUES (?, ?, ?, ?, ?, ?, 3, ?, ?, ?)",
        )
        .bind(order_id)
        .bind(primary_proof)
        .bind(buyer_pubkey)
        .bind(game_coordinate)
        .bind(receipt.id.to_hex())
        .bind(now_secs())
        .bind(parsed.authorization.map(|id| id.to_hex()))
        .bind(receipt.pubkey.to_hex())
        .bind(event_json)
        .execute(&mut *transaction)
        .await?;
        for proof_key in payment_proof_keys {
            sqlx::query("INSERT INTO payment_proofs (proof_key, order_id) VALUES (?, ?)")
                .bind(proof_key)
                .bind(order_id)
                .execute(&mut *transaction)
                .await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    pub async fn payment_proofs_already_used(
        &self,
        proof_keys: &[String],
    ) -> Result<bool, sqlx::Error> {
        for proof_key in proof_keys {
            let used: Option<(i64,)> =
                sqlx::query_as("SELECT 1 FROM payment_proofs WHERE proof_key = ? LIMIT 1")
                    .bind(proof_key)
                    .fetch_optional(&self.pool)
                    .await?;
            if used.is_some() {
                return Ok(true);
            }
        }
        Ok(false)
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
        .bind(parsed.authorization.map(|id| id.to_hex()))
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
            || parsed.authorization.map(|id| id.to_hex()) != authorization_event_id
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

fn settlement_i64(value: u64) -> Result<i64, PersistenceError> {
    i64::try_from(value)
        .map_err(|_| PersistenceError::Corrupt("settlement integer exceeds sqlite range".into()))
}

fn settlement_u64(value: i64) -> Result<u64, PersistenceError> {
    u64::try_from(value)
        .map_err(|_| PersistenceError::Corrupt("negative settlement integer".into()))
}

fn parse_quote_state(value: &str) -> Result<QuoteState, PersistenceError> {
    match value {
        "draft" => Ok(QuoteState::Draft),
        "preflighting" => Ok(QuoteState::Preflighting),
        "active" => Ok(QuoteState::Active),
        "failed" => Ok(QuoteState::Failed),
        "settled" => Ok(QuoteState::Settled),
        _ => Err(PersistenceError::Corrupt("unknown settlement state".into())),
    }
}

impl Storage {
    #[allow(clippy::too_many_arguments)]
    pub async fn save_active_settlement(
        &self,
        quote: &SignedQuote,
        quote_event: &Event,
        listing_event: &Event,
        encrypted_quote: &[u8],
        terms: &[NewSettlementTerm],
        encrypted_leg_destinations: &[(String, Vec<u8>)],
        attempts: &[NewSettlementAttempt],
        refund_destination_id: &str,
        refund_commitment: &str,
        encrypted_refund_destination: &[u8],
        refund_min_msat: u64,
        refund_max_msat: u64,
    ) -> Result<(), PersistenceError> {
        if quote.state != QuoteState::Active || attempts.len() != quote.legs.len() {
            return Err(PersistenceError::Corrupt(
                "only fully preflighted quotes may be stored".into(),
            ));
        }
        let quote_json = serde_json::to_string(quote_event)?;
        let listing_json = serde_json::to_string(listing_event)?;
        let mut tx = self.pool.begin().await?;
        for record in terms {
            sqlx::query("INSERT OR IGNORE INTO commercial_terms (root_event_id, operator_pubkey, event_json, fee_json, payment_destination_encrypted, issued_at) VALUES (?, ?, ?, ?, ?, ?)")
                .bind(record.event.id.to_hex()).bind(&record.terms.operator_pubkey)
                .bind(serde_json::to_string(&record.event)?).bind(serde_json::to_string(&record.terms.fee)?)
                .bind(&record.encrypted_destination).bind(settlement_i64(record.event.created_at.as_secs())?)
                .execute(&mut *tx).await?;
        }
        sqlx::query("INSERT INTO purchase_quotes (quote_id, quote_event_id, quote_event_json, buyer_pubkey, listing_coordinate, listing_event_id, processor_pubkey, gross_msat, state, issued_at, payment_expires_at, confirmation_expires_at, listing_event_json, quote_plaintext_encrypted) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'active', ?, ?, ?, ?, ?)")
            .bind(&quote.quote_id).bind(quote_event.id.to_hex()).bind(quote_json).bind(&quote.buyer_pubkey)
            .bind(&quote.listing_coordinate).bind(&quote.listing_event_id).bind(&quote.processor_pubkey)
            .bind(settlement_i64(quote.gross_msat)?).bind(settlement_i64(quote.issued_at)?)
            .bind(settlement_i64(quote.payment_expires_at)?).bind(settlement_i64(quote.confirmation_expires_at)?)
            .bind(listing_json).bind(encrypted_quote).execute(&mut *tx).await?;
        for record in terms {
            sqlx::query("INSERT INTO quote_terms (quote_id, operator_pubkey, terms_event_id) VALUES (?, ?, ?)")
                .bind(&quote.quote_id).bind(&record.terms.operator_pubkey).bind(record.event.id.to_hex())
                .execute(&mut *tx).await?;
        }
        for (order, leg) in quote.legs.iter().enumerate() {
            let destination = encrypted_leg_destinations
                .iter()
                .find(|(id, _)| id == &leg.leg_id)
                .ok_or_else(|| {
                    PersistenceError::Corrupt("missing encrypted leg destination".into())
                })?;
            let role = match leg.role {
                adp_core::PaymentRole::Developer => "developer",
                adp_core::PaymentRole::Operator => "operator",
            };
            sqlx::query("INSERT INTO payment_legs (quote_id, leg_id, leg_order, role, recipient_pubkey, destination_encrypted, amount_msat, accepted_terms_id) VALUES (?, ?, ?, ?, ?, ?, ?, ?)")
                .bind(&quote.quote_id).bind(&leg.leg_id).bind(i64::try_from(order).map_err(|_| PersistenceError::Corrupt("too many legs".into()))?)
                .bind(role).bind(&leg.recipient_pubkey).bind(&destination.1).bind(settlement_i64(leg.amount_msat)?)
                .bind(&leg.accepted_terms_id).execute(&mut *tx).await?;
        }
        for attempt in attempts {
            sqlx::query("INSERT INTO invoice_attempts (attempt_id, quote_id, leg_id, bolt11_encrypted, lnurl_origin, callback, lsp_pubkey, payment_hash, amount_msat, expires_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
                .bind(&attempt.attempt_id).bind(&quote.quote_id).bind(&attempt.leg_id).bind(&attempt.encrypted_bolt11)
                .bind(&attempt.lnurl_origin).bind(&attempt.callback).bind(&attempt.lsp_pubkey).bind(&attempt.payment_hash)
                .bind(settlement_i64(attempt.amount_msat)?).bind(settlement_i64(attempt.expires_at)?)
                .execute(&mut *tx).await?;
        }
        sqlx::query("INSERT INTO refund_destinations (destination_id, quote_id, commitment, destination_encrypted, min_sendable_msat, max_sendable_msat, authorized_at) VALUES (?, ?, ?, ?, ?, ?, ?)")
            .bind(refund_destination_id).bind(&quote.quote_id).bind(refund_commitment).bind(encrypted_refund_destination)
            .bind(settlement_i64(refund_min_msat)?).bind(settlement_i64(refund_max_msat)?).bind(settlement_i64(quote.issued_at)?)
            .execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn settlement_quote(
        &self,
        quote_id: &str,
    ) -> Result<Option<StoredSettlementQuote>, PersistenceError> {
        let row = sqlx::query("SELECT quote_event_json, listing_event_json, quote_plaintext_encrypted, buyer_pubkey, state, payment_expires_at, confirmation_expires_at, failure_at, receipt_event_json FROM purchase_quotes WHERE quote_id=?")
            .bind(quote_id).fetch_optional(&self.pool).await?;
        row.map(|row| {
            let receipt_json: Option<String> = row.try_get("receipt_event_json")?;
            Ok(StoredSettlementQuote {
                quote_event: serde_json::from_str(row.try_get("quote_event_json")?)?,
                listing_event: serde_json::from_str(
                    row.try_get::<Option<&str>, _>("listing_event_json")?
                        .ok_or_else(|| {
                            PersistenceError::Corrupt("missing listing snapshot".into())
                        })?,
                )?,
                encrypted_quote: row
                    .try_get::<Option<Vec<u8>>, _>("quote_plaintext_encrypted")?
                    .ok_or_else(|| PersistenceError::Corrupt("missing quote snapshot".into()))?,
                buyer_pubkey: row.try_get("buyer_pubkey")?,
                state: parse_quote_state(row.try_get("state")?)?,
                payment_expires_at: settlement_u64(row.try_get("payment_expires_at")?)?,
                confirmation_expires_at: settlement_u64(row.try_get("confirmation_expires_at")?)?,
                failure_at: row
                    .try_get::<Option<i64>, _>("failure_at")?
                    .map(settlement_u64)
                    .transpose()?,
                receipt: receipt_json
                    .map(|json| serde_json::from_str(&json))
                    .transpose()?,
            })
        })
        .transpose()
    }

    pub async fn settlement_attempts(
        &self,
        quote_id: &str,
    ) -> Result<Vec<StoredSettlementAttempt>, PersistenceError> {
        let rows = sqlx::query("SELECT i.attempt_id, i.leg_id, i.bolt11_encrypted, i.payment_hash, i.amount_msat, i.expires_at, i.disclosed_at, i.settled_at, i.proof_type, i.proof_reference FROM invoice_attempts i JOIN payment_legs l ON l.quote_id=i.quote_id AND l.leg_id=i.leg_id WHERE i.quote_id=? ORDER BY l.leg_order, i.rowid")
            .bind(quote_id).fetch_all(&self.pool).await?;
        rows.into_iter()
            .map(|row| {
                Ok(StoredSettlementAttempt {
                    attempt_id: row.try_get("attempt_id")?,
                    leg_id: row.try_get("leg_id")?,
                    encrypted_bolt11: row.try_get("bolt11_encrypted")?,
                    payment_hash: row.try_get("payment_hash")?,
                    amount_msat: settlement_u64(row.try_get("amount_msat")?)?,
                    expires_at: settlement_u64(row.try_get("expires_at")?)?,
                    disclosed_at: row
                        .try_get::<Option<i64>, _>("disclosed_at")?
                        .map(settlement_u64)
                        .transpose()?,
                    settled_at: row
                        .try_get::<Option<i64>, _>("settled_at")?
                        .map(settlement_u64)
                        .transpose()?,
                    proof_type: row.try_get("proof_type")?,
                    proof_reference: row.try_get("proof_reference")?,
                })
            })
            .collect()
    }

    pub async fn add_replacement_attempt(
        &self,
        quote_id: &str,
        replaced_attempt_id: &str,
        attempt: &NewSettlementAttempt,
        now: u64,
    ) -> Result<(), PersistenceError> {
        let result = sqlx::query("INSERT INTO invoice_attempts (attempt_id, quote_id, leg_id, bolt11_encrypted, lnurl_origin, callback, lsp_pubkey, payment_hash, amount_msat, expires_at) SELECT ?, i.quote_id, i.leg_id, ?, ?, ?, ?, ?, i.amount_msat, ? FROM invoice_attempts i JOIN purchase_quotes q ON q.quote_id=i.quote_id WHERE i.quote_id=? AND i.attempt_id=? AND i.leg_id=? AND i.settled_at IS NULL AND i.expires_at<=? AND q.state='active' AND q.payment_expires_at>=? AND ?<=q.confirmation_expires_at")
            .bind(&attempt.attempt_id).bind(&attempt.encrypted_bolt11).bind(&attempt.lnurl_origin)
            .bind(&attempt.callback).bind(&attempt.lsp_pubkey).bind(&attempt.payment_hash)
            .bind(settlement_i64(attempt.expires_at)?).bind(quote_id).bind(replaced_attempt_id)
            .bind(&attempt.leg_id).bind(settlement_i64(now)?).bind(settlement_i64(now)?)
            .bind(settlement_i64(attempt.expires_at)?).execute(&self.pool).await?;
        if result.rows_affected() != 1 {
            return Err(PersistenceError::Corrupt(
                "invoice attempt is not replaceable".into(),
            ));
        }
        Ok(())
    }

    pub async fn disclose_attempt(
        &self,
        quote_id: &str,
        attempt_id: &str,
        now: u64,
    ) -> Result<bool, PersistenceError> {
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query("UPDATE invoice_attempts SET disclosed_at=COALESCE(disclosed_at, ?) WHERE quote_id=? AND attempt_id=? AND settled_at IS NULL AND EXISTS (SELECT 1 FROM purchase_quotes q WHERE q.quote_id=? AND q.state='active' AND q.payment_expires_at>=?) AND NOT EXISTS (SELECT 1 FROM payment_legs prior WHERE prior.quote_id=? AND prior.leg_order < (SELECT leg_order FROM payment_legs WHERE quote_id=? AND leg_id=invoice_attempts.leg_id) AND prior.settled_at IS NULL)")
            .bind(settlement_i64(now)?).bind(quote_id).bind(attempt_id).bind(quote_id)
            .bind(settlement_i64(now)?).bind(quote_id).bind(quote_id)
            .execute(&mut *tx).await?;
        if result.rows_affected() == 1 {
            sqlx::query("UPDATE payment_legs SET disclosed_at=COALESCE(disclosed_at, ?) WHERE quote_id=? AND leg_id=(SELECT leg_id FROM invoice_attempts WHERE attempt_id=?)")
                .bind(settlement_i64(now)?).bind(quote_id).bind(attempt_id).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(result.rows_affected() == 1)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn settle_attempt(
        &self,
        quote_id: &str,
        attempt_id: &str,
        proof_type: &str,
        proof_reference: &str,
        encrypted_proof: &[u8],
        settled_at: u64,
    ) -> Result<bool, PersistenceError> {
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query("UPDATE invoice_attempts SET proof_type=?, proof_reference=?, proof_encrypted=?, settled_at=? WHERE quote_id=? AND attempt_id=? AND disclosed_at IS NOT NULL AND settled_at IS NULL")
            .bind(proof_type).bind(proof_reference).bind(encrypted_proof).bind(settlement_i64(settled_at)?)
            .bind(quote_id).bind(attempt_id).execute(&mut *tx).await?;
        if result.rows_affected() == 1 {
            sqlx::query("UPDATE payment_legs SET settled_at=? WHERE quote_id=? AND leg_id=(SELECT leg_id FROM invoice_attempts WHERE attempt_id=?)")
                .bind(settlement_i64(settled_at)?).bind(quote_id).bind(attempt_id).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn process_settlement_deadlines(
        &self,
        quote_id: &str,
        now: u64,
    ) -> Result<(), PersistenceError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("UPDATE purchase_quotes SET state='failed', failure_at=payment_expires_at WHERE quote_id=? AND state='active' AND payment_expires_at<=? AND EXISTS (SELECT 1 FROM payment_legs l WHERE l.quote_id=purchase_quotes.quote_id AND l.disclosed_at IS NULL)")
            .bind(quote_id).bind(settlement_i64(now)?).execute(&mut *tx).await?;
        sqlx::query("UPDATE purchase_quotes SET state='failed', failure_at=confirmation_expires_at WHERE quote_id=? AND state='active' AND confirmation_expires_at<=?")
            .bind(quote_id).bind(settlement_i64(now)?).execute(&mut *tx).await?;
        let due_delta = settlement_i64(REFUND_DUE_SECS)?;
        sqlx::query("INSERT OR IGNORE INTO refund_obligations (quote_id, leg_id, principal_msat, state, due_at, updated_at) SELECT q.quote_id, l.leg_id, l.amount_msat, 'refund_owed', q.failure_at + ?, ? FROM purchase_quotes q JOIN payment_legs l ON l.quote_id=q.quote_id WHERE q.quote_id=? AND q.state='failed' AND l.settled_at IS NOT NULL")
            .bind(due_delta).bind(settlement_i64(now)?).bind(quote_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE refund_obligations SET state='overdue', updated_at=? WHERE quote_id=? AND state IN ('refund_owed','initiated') AND due_at<?")
            .bind(settlement_i64(now)?).bind(quote_id).bind(settlement_i64(now)?).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Advances every expired quote/refund deadline without requiring endpoint traffic.
    pub async fn process_all_settlement_deadlines(
        &self,
        now: u64,
    ) -> Result<u64, PersistenceError> {
        let now_i64 = settlement_i64(now)?;
        let rows = sqlx::query(
            "SELECT quote_id FROM purchase_quotes \
             WHERE state='active' AND (payment_expires_at<=? OR confirmation_expires_at<=?) \
             UNION \
             SELECT quote_id FROM refund_obligations \
             WHERE state IN ('refund_owed','initiated') AND due_at<?",
        )
        .bind(now_i64)
        .bind(now_i64)
        .bind(now_i64)
        .fetch_all(&self.pool)
        .await?;

        let mut processed = 0_u64;
        for row in rows {
            let quote_id: String = row.try_get("quote_id")?;
            self.process_settlement_deadlines(&quote_id, now).await?;
            processed = processed.saturating_add(1);
        }
        Ok(processed)
    }

    pub async fn settlement_status(
        &self,
        quote_id: &str,
    ) -> Result<Option<SettlementStatus>, PersistenceError> {
        let row = sqlx::query("SELECT q.state, q.failure_at, q.receipt_event_id, COUNT(l.leg_id) total_legs, COALESCE(SUM(l.disclosed_at IS NOT NULL),0) disclosed_legs, COALESCE(SUM(l.settled_at IS NOT NULL),0) settled_legs FROM purchase_quotes q LEFT JOIN payment_legs l ON l.quote_id=q.quote_id WHERE q.quote_id=? GROUP BY q.quote_id")
            .bind(quote_id).fetch_optional(&self.pool).await?;
        row.map(|row| {
            let failure_at = row
                .try_get::<Option<i64>, _>("failure_at")?
                .map(settlement_u64)
                .transpose()?;
            Ok(SettlementStatus {
                state: parse_quote_state(row.try_get("state")?)?,
                total_legs: settlement_u64(row.try_get("total_legs")?)?,
                disclosed_legs: settlement_u64(row.try_get("disclosed_legs")?)?,
                settled_legs: settlement_u64(row.try_get("settled_legs")?)?,
                failure_at,
                refund_due_at: failure_at.and_then(|value| value.checked_add(REFUND_DUE_SECS)),
                receipt_event_id: row.try_get("receipt_event_id")?,
            })
        })
        .transpose()
    }

    pub async fn store_settlement_receipt(
        &self,
        quote_id: &str,
        order_id: &str,
        proof_keys: &[String],
        buyer_pubkey: &str,
        game_coordinate: &str,
        receipt: &Event,
    ) -> Result<bool, PersistenceError> {
        let parsed = parse_receipt_event(receipt).map_err(|error| {
            PersistenceError::Corrupt(format!("invalid aggregate receipt: {error}"))
        })?;
        if parsed.order_id != order_id
            || parsed.buyer_pubkey.to_hex() != buyer_pubkey
            || parsed.coordinate != game_coordinate
            || parsed.aggregate.is_none()
            || proof_keys.is_empty()
        {
            return Err(PersistenceError::Corrupt(
                "aggregate receipt columns disagree with event".into(),
            ));
        }
        let event_json = serde_json::to_string(receipt)?;
        let mut tx = self.pool.begin().await?;
        let existing = sqlx::query("SELECT receipt_event_id FROM purchase_quotes WHERE quote_id=?")
            .bind(quote_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| PersistenceError::Corrupt("unknown settlement quote".into()))?
            .try_get::<Option<String>, _>("receipt_event_id")?;
        if let Some(existing) = existing {
            tx.commit().await?;
            return Ok(existing == receipt.id.to_hex());
        }
        sqlx::query("INSERT INTO purchases (order_id, payment_proof_event_id, buyer_pubkey, game_coordinate, receipt_event_id, created_at, adp_profile_version, authorization_event_id, receipt_issuer_pubkey, receipt_event_json) VALUES (?, ?, ?, ?, ?, ?, 3, ?, ?, ?)")
            .bind(order_id).bind(&proof_keys[0]).bind(buyer_pubkey).bind(game_coordinate)
            .bind(receipt.id.to_hex()).bind(now_secs()).bind(parsed.authorization.map(|id| id.to_hex()))
            .bind(receipt.pubkey.to_hex()).bind(&event_json).execute(&mut *tx).await?;
        for proof_key in proof_keys {
            sqlx::query("INSERT INTO payment_proofs (proof_key, order_id) VALUES (?, ?)")
                .bind(proof_key)
                .bind(order_id)
                .execute(&mut *tx)
                .await?;
        }
        sqlx::query("UPDATE purchase_quotes SET state='settled', receipt_event_id=?, receipt_event_json=? WHERE quote_id=? AND state='active'")
            .bind(receipt.id.to_hex()).bind(event_json).bind(quote_id).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(true)
    }

    pub async fn record_receipt_publish_attempt(
        &self,
        quote_id: &str,
    ) -> Result<(), PersistenceError> {
        sqlx::query("UPDATE purchase_quotes SET receipt_publish_attempts=receipt_publish_attempts+1 WHERE quote_id=?")
            .bind(quote_id).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn replace_refund_destination(
        &self,
        quote_id: &str,
        destination_id: &str,
        commitment: &str,
        encrypted_destination: &[u8],
        min_msat: u64,
        max_msat: u64,
        authorized_at: u64,
    ) -> Result<(), PersistenceError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "UPDATE refund_destinations SET replaced_at=? WHERE quote_id=? AND replaced_at IS NULL",
        )
        .bind(settlement_i64(authorized_at)?)
        .bind(quote_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query("INSERT INTO refund_destinations (destination_id, quote_id, commitment, destination_encrypted, min_sendable_msat, max_sendable_msat, authorized_at) VALUES (?, ?, ?, ?, ?, ?, ?)")
            .bind(destination_id).bind(quote_id).bind(commitment).bind(encrypted_destination)
            .bind(settlement_i64(min_msat)?).bind(settlement_i64(max_msat)?)
            .bind(settlement_i64(authorized_at)?).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn refund_context(
        &self,
        quote_id: &str,
        leg_id: &str,
    ) -> Result<Option<RefundContext>, PersistenceError> {
        let row = sqlx::query("SELECT l.leg_id, l.recipient_pubkey, q.buyer_pubkey, o.principal_msat, d.destination_id, d.destination_encrypted, q.failure_at FROM refund_obligations o JOIN payment_legs l ON l.quote_id=o.quote_id AND l.leg_id=o.leg_id JOIN purchase_quotes q ON q.quote_id=o.quote_id JOIN refund_destinations d ON d.quote_id=q.quote_id AND d.replaced_at IS NULL WHERE o.quote_id=? AND o.leg_id=? AND o.state IN ('refund_owed','initiated','overdue')")
            .bind(quote_id).bind(leg_id).fetch_optional(&self.pool).await?;
        row.map(|row| {
            Ok(RefundContext {
                leg_id: row.try_get("leg_id")?,
                recipient_pubkey: row.try_get("recipient_pubkey")?,
                buyer_pubkey: row.try_get("buyer_pubkey")?,
                principal_msat: settlement_u64(row.try_get("principal_msat")?)?,
                destination_id: row.try_get("destination_id")?,
                encrypted_destination: row.try_get("destination_encrypted")?,
                failure_at: settlement_u64(
                    row.try_get::<Option<i64>, _>("failure_at")?
                        .ok_or_else(|| {
                            PersistenceError::Corrupt("refund without failure time".into())
                        })?,
                )?,
            })
        })
        .transpose()
    }

    pub async fn save_refund_attempt(
        &self,
        quote_id: &str,
        context: &RefundContext,
        attempt: &NewSettlementAttempt,
        created_at: u64,
    ) -> Result<(), PersistenceError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("INSERT INTO refund_attempts (attempt_id, quote_id, leg_id, destination_id, amount_msat, created_at, bolt11_encrypted, lnurl_origin, callback, lsp_pubkey, payment_hash, expires_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(&attempt.attempt_id).bind(quote_id).bind(&context.leg_id).bind(&context.destination_id)
            .bind(settlement_i64(context.principal_msat)?).bind(settlement_i64(created_at)?)
            .bind(&attempt.encrypted_bolt11).bind(&attempt.lnurl_origin).bind(&attempt.callback)
            .bind(&attempt.lsp_pubkey).bind(&attempt.payment_hash).bind(settlement_i64(attempt.expires_at)?)
            .execute(&mut *tx).await?;
        sqlx::query("UPDATE refund_obligations SET state='initiated', updated_at=? WHERE quote_id=? AND leg_id=? AND state IN ('refund_owed','overdue')")
            .bind(settlement_i64(created_at)?).bind(quote_id).bind(&context.leg_id).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn refund_attempt(
        &self,
        quote_id: &str,
        attempt_id: &str,
    ) -> Result<Option<StoredRefundAttempt>, PersistenceError> {
        let row = sqlx::query("SELECT attempt_id, leg_id, amount_msat, bolt11_encrypted, payment_hash, expires_at, verified_at FROM refund_attempts WHERE quote_id=? AND attempt_id=?")
            .bind(quote_id).bind(attempt_id).fetch_optional(&self.pool).await?;
        row.map(|row| {
            Ok(StoredRefundAttempt {
                attempt_id: row.try_get("attempt_id")?,
                leg_id: row.try_get("leg_id")?,
                amount_msat: settlement_u64(row.try_get("amount_msat")?)?,
                encrypted_bolt11: row
                    .try_get::<Option<Vec<u8>>, _>("bolt11_encrypted")?
                    .ok_or_else(|| PersistenceError::Corrupt("refund invoice missing".into()))?,
                payment_hash: row
                    .try_get::<Option<String>, _>("payment_hash")?
                    .ok_or_else(|| PersistenceError::Corrupt("refund hash missing".into()))?,
                expires_at: settlement_u64(
                    row.try_get::<Option<i64>, _>("expires_at")?
                        .ok_or_else(|| PersistenceError::Corrupt("refund expiry missing".into()))?,
                )?,
                verified_at: row
                    .try_get::<Option<i64>, _>("verified_at")?
                    .map(settlement_u64)
                    .transpose()?,
            })
        })
        .transpose()
    }

    pub async fn verify_refund_attempt(
        &self,
        quote_id: &str,
        attempt_id: &str,
        proof_reference: &str,
        encrypted_proof: &[u8],
        verified_at: u64,
    ) -> Result<bool, PersistenceError> {
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query("UPDATE refund_attempts SET proof_reference=?, proof_encrypted=?, verified_at=? WHERE quote_id=? AND attempt_id=? AND verified_at IS NULL")
            .bind(proof_reference).bind(encrypted_proof).bind(settlement_i64(verified_at)?)
            .bind(quote_id).bind(attempt_id).execute(&mut *tx).await?;
        if result.rows_affected() == 1 {
            sqlx::query("UPDATE refund_obligations SET state='verified', updated_at=? WHERE quote_id=? AND leg_id=(SELECT leg_id FROM refund_attempts WHERE attempt_id=?)")
                .bind(settlement_i64(verified_at)?).bind(quote_id).bind(attempt_id).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn save_recovery_claim(
        &self,
        quote_id: &str,
        claimant_pubkey: &str,
        claimant_role: &str,
        event: &Event,
    ) -> Result<(), PersistenceError> {
        sqlx::query("INSERT OR IGNORE INTO recovery_claims (claim_event_id, quote_id, claimant_pubkey, claimant_role, event_json, created_at) VALUES (?, ?, ?, ?, ?, ?)")
            .bind(event.id.to_hex()).bind(quote_id).bind(claimant_pubkey).bind(claimant_role)
            .bind(serde_json::to_string(event)?).bind(settlement_i64(event.created_at.as_secs())?)
            .execute(&self.pool).await?;
        Ok(())
    }
}

#[cfg(test)]
mod settlement_deadline_tests {
    use super::Storage;
    use sqlx::Row;

    async fn insert_quote(storage: &Storage, quote_id: &str, second_leg_undisclosed: bool) {
        sqlx::query(
            "INSERT INTO purchase_quotes \
             (quote_id, quote_event_id, quote_event_json, buyer_pubkey, listing_coordinate, \
              listing_event_id, processor_pubkey, gross_msat, state, issued_at, \
              payment_expires_at, confirmation_expires_at) \
             VALUES (?, ?, '{}', 'buyer', '30402:dev:game', 'listing', 'processor', \
                     2000, 'active', 100, 1900, 86500)",
        )
        .bind(quote_id)
        .bind(format!("event-{quote_id}"))
        .execute(&storage.pool())
        .await
        .expect("insert quote");
        sqlx::query(
            "INSERT INTO payment_legs \
             (quote_id, leg_id, leg_order, role, recipient_pubkey, destination_encrypted, \
              amount_msat, disclosed_at, settled_at) \
             VALUES (?, 'developer', 0, 'developer', 'dev', X'01', 1000, 150, 200)",
        )
        .bind(quote_id)
        .execute(&storage.pool())
        .await
        .expect("insert settled leg");
        sqlx::query(
            "INSERT INTO payment_legs \
             (quote_id, leg_id, leg_order, role, recipient_pubkey, destination_encrypted, \
              amount_msat, disclosed_at) \
             VALUES (?, 'operator', 1, 'operator', 'operator', X'02', 1000, ?)",
        )
        .bind(quote_id)
        .bind((!second_leg_undisclosed).then_some(160_i64))
        .execute(&storage.pool())
        .await
        .expect("insert pending leg");
    }

    #[tokio::test]
    async fn global_deadline_processing_is_deterministic_and_idempotent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let storage = Storage::connect(dir.path().join("adp.db").to_str().expect("path"))
            .await
            .expect("storage");
        insert_quote(&storage, "partial", true).await;
        insert_quote(&storage, "all-disclosed", false).await;

        assert_eq!(
            storage
                .process_all_settlement_deadlines(1900)
                .await
                .expect("process"),
            2
        );
        let partial =
            sqlx::query("SELECT state, failure_at FROM purchase_quotes WHERE quote_id='partial'")
                .fetch_one(&storage.pool())
                .await
                .expect("partial row");
        assert_eq!(partial.get::<String, _>("state"), "failed");
        assert_eq!(partial.get::<i64, _>("failure_at"), 1900);
        let disclosed = sqlx::query(
            "SELECT state, failure_at FROM purchase_quotes WHERE quote_id='all-disclosed'",
        )
        .fetch_one(&storage.pool())
        .await
        .expect("disclosed row");
        assert_eq!(disclosed.get::<String, _>("state"), "active");
        assert_eq!(disclosed.get::<Option<i64>, _>("failure_at"), None);

        let obligation = sqlx::query(
            "SELECT state, principal_msat, due_at FROM refund_obligations \
             WHERE quote_id='partial' AND leg_id='developer'",
        )
        .fetch_one(&storage.pool())
        .await
        .expect("obligation");
        assert_eq!(obligation.get::<String, _>("state"), "refund_owed");
        assert_eq!(obligation.get::<i64, _>("principal_msat"), 1000);
        assert_eq!(obligation.get::<i64, _>("due_at"), 606700);

        sqlx::query(
            "UPDATE payment_legs SET disclosed_at=1800, settled_at=2000 \
             WHERE quote_id='partial' AND leg_id='operator'",
        )
        .execute(&storage.pool())
        .await
        .expect("record late settlement");
        storage
            .process_settlement_deadlines("partial", 2000)
            .await
            .expect("retain late settlement");
        let late_obligation = sqlx::query(
            "SELECT principal_msat, due_at FROM refund_obligations \
             WHERE quote_id='partial' AND leg_id='operator'",
        )
        .fetch_one(&storage.pool())
        .await
        .expect("late obligation");
        assert_eq!(late_obligation.get::<i64, _>("principal_msat"), 1000);
        assert_eq!(late_obligation.get::<i64, _>("due_at"), 606700);

        sqlx::query(
            "INSERT INTO refund_destinations \
             (destination_id, quote_id, commitment, destination_encrypted, \
              min_sendable_msat, max_sendable_msat, authorized_at) \
             VALUES ('old-destination', 'partial', 'old', X'01', 1, 2000, 2100)",
        )
        .execute(&storage.pool())
        .await
        .expect("old destination");
        storage
            .replace_refund_destination("partial", "new-destination", "new", &[2], 1, 2000, 2500)
            .await
            .expect("replace destination");
        let destination_count = sqlx::query(
            "SELECT COUNT(*) active FROM refund_destinations \
             WHERE quote_id='partial' AND replaced_at IS NULL",
        )
        .fetch_one(&storage.pool())
        .await
        .expect("active destination count");
        assert_eq!(destination_count.get::<i64, _>("active"), 1);
        let immutable_failure =
            sqlx::query("SELECT failure_at FROM purchase_quotes WHERE quote_id='partial'")
                .fetch_one(&storage.pool())
                .await
                .expect("failure after replacement");
        assert_eq!(immutable_failure.get::<i64, _>("failure_at"), 1900);

        storage
            .process_all_settlement_deadlines(606701)
            .await
            .expect("overdue");
        storage
            .process_all_settlement_deadlines(606702)
            .await
            .expect("idempotent rerun");
        let overdue = sqlx::query(
            "SELECT state, due_at FROM refund_obligations \
             WHERE quote_id='partial' AND leg_id='developer'",
        )
        .fetch_one(&storage.pool())
        .await
        .expect("overdue row");
        assert_eq!(overdue.get::<String, _>("state"), "overdue");
        assert_eq!(overdue.get::<i64, _>("due_at"), 606700);
        let unchanged =
            sqlx::query("SELECT failure_at FROM purchase_quotes WHERE quote_id='partial'")
                .fetch_one(&storage.pool())
                .await
                .expect("unchanged failure");
        assert_eq!(unchanged.get::<i64, _>("failure_at"), 1900);

        storage
            .process_all_settlement_deadlines(86500)
            .await
            .expect("confirmation");
        let confirmed = sqlx::query(
            "SELECT state, failure_at FROM purchase_quotes WHERE quote_id='all-disclosed'",
        )
        .fetch_one(&storage.pool())
        .await
        .expect("confirmed row");
        assert_eq!(confirmed.get::<String, _>("state"), "failed");
        assert_eq!(confirmed.get::<i64, _>("failure_at"), 86500);
    }
}
