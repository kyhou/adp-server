use sqlx::{Executor, SqlitePool};

#[tokio::test]
async fn payment_proof_migration_backfills_existing_replay_keys() {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    pool.execute(
        "CREATE TABLE purchases (
            order_id TEXT NOT NULL UNIQUE,
            payment_proof_event_id TEXT NOT NULL UNIQUE
        )",
    )
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO purchases (order_id, payment_proof_event_id)
         VALUES ('zap-order', 'zap-event-id'), ('bolt-order', 'preimage-hex')",
    )
    .execute(&pool)
    .await
    .unwrap();

    pool.execute(include_str!("../../migrations/0004_payment_proofs.sql"))
        .await
        .unwrap();

    let keys: Vec<String> =
        sqlx::query_scalar("SELECT proof_key FROM payment_proofs ORDER BY proof_key")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(keys, vec!["preimage-hex", "zap-event-id"]);
    assert!(
        sqlx::query("INSERT INTO payment_proofs (proof_key, order_id) VALUES (?, ?)")
            .bind("zap-event-id")
            .bind("bolt-order")
            .execute(&pool)
            .await
            .is_err()
    );
}
