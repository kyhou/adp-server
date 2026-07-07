use sqlx::sqlite::SqlitePoolOptions;
use sqlx::SqlitePool;

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
