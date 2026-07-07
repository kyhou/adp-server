use std::time::Duration;

use crate::error::ApiResult;
use crate::routes::game::verify_file_hash;
use crate::storage::Storage;

pub async fn verify_stored_files_once(storage: &Storage) -> ApiResult<()> {
    for file in storage.stored_files().await? {
        verify_file_hash(&file.file_path, &file.file_hash).await?;
    }
    Ok(())
}

pub fn spawn_file_hash_reverification(storage: Storage, interval: Duration) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        loop {
            ticker.tick().await;
            if let Err(error) = verify_stored_files_once(&storage).await {
                tracing::error!(%error, "stored file hash re-verification failed");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Storage;
    use sha2::{Digest, Sha256};

    #[tokio::test]
    async fn verify_stored_files_once_accepts_matching_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("adp.db");
        let file_path = dir.path().join("game.bin");
        tokio::fs::write(&file_path, b"game bytes")
            .await
            .expect("write file");
        let file_hash = hex::encode(Sha256::digest(b"game bytes"));
        let storage = Storage::connect(db_path.to_str().expect("utf8 db path"))
            .await
            .expect("connect storage");
        storage
            .record_file(
                &file_hash,
                "30402:developer:game",
                file_path.to_str().expect("utf8 path"),
            )
            .await
            .expect("record file");

        verify_stored_files_once(&storage)
            .await
            .expect("verify stored files");
    }

    #[tokio::test]
    async fn verify_stored_files_once_rejects_mismatched_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("adp.db");
        let file_path = dir.path().join("game.bin");
        tokio::fs::write(&file_path, b"tampered bytes")
            .await
            .expect("write file");
        let storage = Storage::connect(db_path.to_str().expect("utf8 db path"))
            .await
            .expect("connect storage");
        storage
            .record_file(
                "abc123",
                "30402:developer:game",
                file_path.to_str().expect("utf8 path"),
            )
            .await
            .expect("record file");

        assert!(verify_stored_files_once(&storage).await.is_err());
    }
}
