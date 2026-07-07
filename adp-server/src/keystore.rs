use nostr_sdk::{Keys, SecretKey};
use sqlx::{Row, SqlitePool};
use thiserror::Error;

use crate::crypto::{EncryptedSecret, MasterCipher};

#[derive(Debug, Error)]
pub enum KeyStoreError {
    #[error("storage error: {0}")]
    Storage(#[from] sqlx::Error),
    #[error("encryption error: {0}")]
    Encryption(String),
    #[error("key not found or not owned by this developer")]
    NotFound,
}

#[derive(Debug, Clone)]
pub struct ProvisionedKey {
    pub keys: Keys,
    pub developer_pubkey: String,
    pub scope: String,
    pub fulfillment_pubkey: String,
    pub attestation_event_id: String,
    pub valid_from: u64,
    pub revoked_at: Option<u64>,
}

#[derive(Clone)]
pub struct KeyStore {
    pool: SqlitePool,
    master_key: [u8; 32],
}

impl KeyStore {
    pub fn new(pool: SqlitePool, master_key: [u8; 32]) -> Self {
        Self { pool, master_key }
    }

    pub async fn active_key_for_scope(
        &self,
        developer_pubkey: &str,
        scope: &str,
    ) -> Result<Option<ProvisionedKey>, KeyStoreError> {
        let row = sqlx::query(
            "SELECT developer_pubkey, scope, fulfillment_pubkey, encrypted_privkey,
                    encryption_nonce, attestation_event_id, valid_from, revoked_at
             FROM fulfillment_keys
             WHERE developer_pubkey = ? AND scope = ? AND revoked_at IS NULL
             LIMIT 1",
        )
        .bind(developer_pubkey)
        .bind(scope)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| self.provisioned_key_from_row(row))
            .transpose()
    }

    pub async fn insert_provisioned_key(
        &self,
        developer_pubkey: &str,
        scope: &str,
        keys: &Keys,
        attestation_event_id: &str,
        valid_from: u64,
    ) -> Result<(), KeyStoreError> {
        let cipher = MasterCipher::new(&self.master_key)
            .map_err(|e| KeyStoreError::Encryption(e.to_string()))?;
        let encrypted = cipher.encrypt(keys.secret_key().as_secret_bytes());
        sqlx::query(
            "INSERT INTO fulfillment_keys (
                developer_pubkey, scope, fulfillment_pubkey, encrypted_privkey,
                encryption_nonce, attestation_event_id, valid_from, revoked_at, created_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, NULL, ?)",
        )
        .bind(developer_pubkey)
        .bind(scope)
        .bind(keys.public_key().to_hex())
        .bind(encrypted.ciphertext)
        .bind(encrypted.nonce)
        .bind(attestation_event_id)
        .bind(valid_from as i64)
        .bind(valid_from as i64)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn attestation_for_fulfillment_pubkey(
        &self,
        developer_pubkey: &str,
        fulfillment_pubkey: &str,
    ) -> Result<Option<ProvisionedKey>, KeyStoreError> {
        let row = sqlx::query(
            "SELECT developer_pubkey, scope, fulfillment_pubkey, encrypted_privkey,
                    encryption_nonce, attestation_event_id, valid_from, revoked_at
             FROM fulfillment_keys
             WHERE developer_pubkey = ? AND fulfillment_pubkey = ? AND revoked_at IS NULL
             LIMIT 1",
        )
        .bind(developer_pubkey)
        .bind(fulfillment_pubkey)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| self.provisioned_key_from_row(row))
            .transpose()
    }

    pub async fn key_for_fulfillment_pubkey(
        &self,
        fulfillment_pubkey: &str,
    ) -> Result<Option<Keys>, KeyStoreError> {
        let row = sqlx::query(
            "SELECT developer_pubkey, scope, fulfillment_pubkey, encrypted_privkey,
                    encryption_nonce, attestation_event_id, valid_from, revoked_at
             FROM fulfillment_keys
             WHERE fulfillment_pubkey = ? AND revoked_at IS NULL
             LIMIT 1",
        )
        .bind(fulfillment_pubkey)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| self.provisioned_key_from_row(row).map(|key| key.keys))
            .transpose()
    }

    pub async fn revoke(
        &self,
        developer_pubkey: &str,
        fulfillment_pubkey: &str,
        revoked_at: u64,
    ) -> Result<(), KeyStoreError> {
        let result = sqlx::query(
            "UPDATE fulfillment_keys
             SET revoked_at = ?
             WHERE developer_pubkey = ? AND fulfillment_pubkey = ? AND revoked_at IS NULL",
        )
        .bind(revoked_at as i64)
        .bind(developer_pubkey)
        .bind(fulfillment_pubkey)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(KeyStoreError::NotFound);
        }
        Ok(())
    }

    fn provisioned_key_from_row(
        &self,
        row: sqlx::sqlite::SqliteRow,
    ) -> Result<ProvisionedKey, KeyStoreError> {
        let developer_pubkey: String = row.try_get("developer_pubkey")?;
        let scope: String = row.try_get("scope")?;
        let fulfillment_pubkey: String = row.try_get("fulfillment_pubkey")?;
        let ciphertext: Vec<u8> = row.try_get("encrypted_privkey")?;
        let nonce: Vec<u8> = row.try_get("encryption_nonce")?;
        let attestation_event_id: String = row.try_get("attestation_event_id")?;
        let valid_from: i64 = row.try_get("valid_from")?;
        let revoked_at: Option<i64> = row.try_get("revoked_at")?;
        let cipher = MasterCipher::new(&self.master_key)
            .map_err(|e| KeyStoreError::Encryption(e.to_string()))?;
        let plaintext = cipher
            .decrypt(&EncryptedSecret { nonce, ciphertext })
            .map_err(|e| KeyStoreError::Encryption(e.to_string()))?;
        let secret_key = SecretKey::from_slice(&plaintext)
            .map_err(|e| KeyStoreError::Encryption(e.to_string()))?;
        let keys = Keys::new(secret_key);
        Ok(ProvisionedKey {
            keys,
            developer_pubkey,
            scope,
            fulfillment_pubkey,
            attestation_event_id,
            valid_from: valid_from as u64,
            revoked_at: revoked_at.map(|r| r as u64),
        })
    }
}
