use adp_server::keystore::KeyStore;
use sqlx::sqlite::SqlitePoolOptions;

async fn test_keystore() -> KeyStore {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("connect in-memory sqlite");
    for statement in include_str!("../../migrations/0002_provisioning.sql").split(';') {
        let statement = statement.trim();
        if !statement.is_empty() {
            sqlx::query(statement)
                .execute(&pool)
                .await
                .expect("run provisioning migration");
        }
    }
    KeyStore::new(pool, [7u8; 32])
}

#[tokio::test]
async fn insert_then_fetch_round_trips_fulfillment_key() {
    let store = test_keystore().await;
    let keys = nostr_sdk::Keys::generate();
    let pubkey = keys.public_key().to_hex();

    store
        .insert_provisioned_key("developer-a", "", &keys, "attestation-1", 1_700_000_000)
        .await
        .expect("insert provisioned key");

    let fetched = store
        .key_for_fulfillment_pubkey(&pubkey)
        .await
        .expect("fetch key")
        .expect("active key exists");
    assert_eq!(fetched.public_key(), keys.public_key());
}

#[tokio::test]
async fn active_key_for_scope_returns_existing_active_key() {
    let store = test_keystore().await;
    let keys = nostr_sdk::Keys::generate();

    store
        .insert_provisioned_key(
            "developer-a",
            "game-1",
            &keys,
            "attestation-1",
            1_700_000_000,
        )
        .await
        .expect("insert provisioned key");

    let first = store
        .active_key_for_scope("developer-a", "game-1")
        .await
        .expect("lookup first")
        .expect("first active key");
    let second = store
        .active_key_for_scope("developer-a", "game-1")
        .await
        .expect("lookup second")
        .expect("second active key");

    assert_eq!(first.keys.public_key(), second.keys.public_key());
    assert_eq!(first.attestation_event_id, "attestation-1");
}

#[tokio::test]
async fn different_scopes_for_same_developer_use_different_keys() {
    let store = test_keystore().await;
    let game_one = nostr_sdk::Keys::generate();
    let game_two = nostr_sdk::Keys::generate();

    store
        .insert_provisioned_key(
            "developer-a",
            "game-1",
            &game_one,
            "attestation-1",
            1_700_000_000,
        )
        .await
        .expect("insert game one key");
    store
        .insert_provisioned_key(
            "developer-a",
            "game-2",
            &game_two,
            "attestation-2",
            1_700_000_000,
        )
        .await
        .expect("insert game two key");

    let first = store
        .active_key_for_scope("developer-a", "game-1")
        .await
        .expect("lookup game one")
        .expect("game one key");
    let second = store
        .active_key_for_scope("developer-a", "game-2")
        .await
        .expect("lookup game two")
        .expect("game two key");

    assert_ne!(first.keys.public_key(), second.keys.public_key());
}

#[tokio::test]
async fn unknown_fulfillment_pubkey_returns_none() {
    let store = test_keystore().await;
    let unknown = nostr_sdk::Keys::generate().public_key().to_hex();

    assert!(store
        .key_for_fulfillment_pubkey(&unknown)
        .await
        .expect("lookup unknown")
        .is_none());
}

#[tokio::test]
async fn revoke_removes_key_from_active_lookups() {
    let store = test_keystore().await;
    let old_keys = nostr_sdk::Keys::generate();
    let new_keys = nostr_sdk::Keys::generate();
    let old_pubkey = old_keys.public_key().to_hex();

    store
        .insert_provisioned_key(
            "developer-a",
            "game-1",
            &old_keys,
            "attestation-1",
            1_700_000_000,
        )
        .await
        .expect("insert old key");
    store
        .revoke("developer-a", &old_pubkey, 1_700_000_100)
        .await
        .expect("revoke old key");

    assert!(store
        .active_key_for_scope("developer-a", "game-1")
        .await
        .expect("lookup revoked scope")
        .is_none());
    assert!(store
        .key_for_fulfillment_pubkey(&old_pubkey)
        .await
        .expect("lookup revoked pubkey")
        .is_none());

    store
        .insert_provisioned_key(
            "developer-a",
            "game-1",
            &new_keys,
            "attestation-2",
            1_700_000_200,
        )
        .await
        .expect("insert replacement key");
    let replacement = store
        .active_key_for_scope("developer-a", "game-1")
        .await
        .expect("lookup replacement")
        .expect("replacement active key");
    assert_eq!(replacement.keys.public_key(), new_keys.public_key());
}
