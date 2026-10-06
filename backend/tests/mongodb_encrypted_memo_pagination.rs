use memo_app_backend::{
    application::crypto::{
        HighEncryptedMemoEnvelope, MEMO_HIGH_SCHEMA_VERSION, MEMO_HIGH_SUITE_ID,
    },
    infrastructure::persistence::{
        mongodb::MongoDbAuthoritativeStore, ports::HighEncryptedMemoAuthoritativeStore,
    },
};
use mongodb::Client;
use uuid::Uuid;

const TEST_DATABASE_NAME: &str = "memo_app_encrypted_pagination_test";
const OWNER_IDS: [&str; 4] = [
    "550e8400-e29b-41d4-a716-446655440004",
    "550e8400-e29b-41d4-a716-446655440003",
    "550e8400-e29b-41d4-a716-446655440002",
    "550e8400-e29b-41d4-a716-446655440001",
];
const OTHER_OWNER_ID: &str = "550e8400-e29b-41d4-a716-4466554400ff";

fn envelope(owner_partition: Uuid, id: &str) -> HighEncryptedMemoEnvelope {
    HighEncryptedMemoEnvelope {
        memo_id: Uuid::parse_str(id).unwrap(),
        owner_partition,
        ciphertext: vec![0x11; 32],
        nonce: vec![0x22; 12],
        wrapped_dek: vec![0x33; 48],
        version: 1,
        crypto_suite_id: MEMO_HIGH_SUITE_ID.into(),
        key_version: "memo-key-v1".into(),
        schema_version: MEMO_HIGH_SCHEMA_VERSION,
    }
}

#[tokio::test]
#[ignore = "requires a local MongoDB replica set"]
async fn mongodb_encrypted_authoritative_store_preserves_atomic_outbox_and_versioning_paginates_boundedly(
) {
    let uri = std::env::var("MONGODB_TEST_URI")
        .unwrap_or_else(|_| "mongodb://localhost:27017/?replicaSet=rs0".to_string());
    let cleanup_client = Client::with_uri_str(&uri).await.unwrap();
    cleanup_client
        .database(TEST_DATABASE_NAME)
        .drop()
        .await
        .unwrap();

    let store = MongoDbAuthoritativeStore::new(&uri, TEST_DATABASE_NAME)
        .await
        .unwrap();
    let owner = Uuid::new_v4();
    let other_owner = Uuid::new_v4();

    for id in OWNER_IDS {
        store
            .stage_encrypted_memo_for_migration(&envelope(owner, id))
            .await
            .unwrap();
    }
    store
        .stage_encrypted_memo_for_migration(&envelope(other_owner, OTHER_OWNER_ID))
        .await
        .unwrap();

    let first =
        HighEncryptedMemoAuthoritativeStore::page_envelopes_by_owner(&store, owner, None, 2)
            .await
            .unwrap();
    assert_eq!(first.len(), 2);
    assert_eq!(
        first
            .iter()
            .map(|envelope| envelope.memo_id)
            .collect::<Vec<_>>(),
        OWNER_IDS[..2]
            .iter()
            .map(|id| Uuid::parse_str(id).unwrap())
            .collect::<Vec<_>>()
    );

    let second = HighEncryptedMemoAuthoritativeStore::page_envelopes_by_owner(
        &store,
        owner,
        Some(first[1].memo_id),
        2,
    )
    .await
    .unwrap();
    assert_eq!(second.len(), 2);
    assert_eq!(
        second
            .iter()
            .map(|envelope| envelope.memo_id)
            .collect::<Vec<_>>(),
        OWNER_IDS[2..]
            .iter()
            .map(|id| Uuid::parse_str(id).unwrap())
            .collect::<Vec<_>>()
    );
    assert!(first
        .iter()
        .chain(second.iter())
        .all(|envelope| envelope.owner_partition == owner));

    cleanup_client
        .database(TEST_DATABASE_NAME)
        .drop()
        .await
        .unwrap();
}
