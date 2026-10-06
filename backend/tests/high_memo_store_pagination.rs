use memo_app_backend::{
    application::crypto::{
        HighEncryptedMemoEnvelope, MEMO_HIGH_SCHEMA_VERSION, MEMO_HIGH_SUITE_ID,
    },
    infrastructure::persistence::{
        mongodb::MongoDbAuthoritativeStore,
        ports::{HighEncryptedMemoAuthoritativeStore, ProjectionTarget},
    },
};
use uuid::Uuid;

const OWNER_IDS: [&str; 4] = [
    "550e8400-e29b-41d4-a716-446655440004",
    "550e8400-e29b-41d4-a716-446655440003",
    "550e8400-e29b-41d4-a716-446655440002",
    "550e8400-e29b-41d4-a716-446655440001",
];
const OTHER_OWNER_ID: &str = "550e8400-e29b-41d4-a716-4466554400ff";

fn envelope(owner: Uuid, id: &str, marker: u8) -> HighEncryptedMemoEnvelope {
    HighEncryptedMemoEnvelope {
        memo_id: Uuid::parse_str(id).unwrap(),
        owner_partition: owner,
        ciphertext: vec![marker; 32],
        nonce: vec![marker.wrapping_add(1); 12],
        wrapped_dek: vec![marker.wrapping_add(2); 48],
        version: 1,
        crypto_suite_id: MEMO_HIGH_SUITE_ID.into(),
        key_version: "memo-key-v1".into(),
        schema_version: MEMO_HIGH_SCHEMA_VERSION,
    }
}

#[tokio::test]
#[ignore = "requires a local MongoDB replica set"]
async fn mongodb_encrypted_page_is_owner_scoped_and_uses_exclusive_uuid_cursor() {
    let uri = std::env::var("MONGODB_TEST_URI")
        .unwrap_or_else(|_| "mongodb://localhost:27017/?replicaSet=rs0".to_string());
    let database_name = "memo_app_high_pagination_test";
    let cleanup_client = mongodb::Client::with_uri_str(&uri).await.unwrap();
    cleanup_client.database(database_name).drop().await.unwrap();

    let store = MongoDbAuthoritativeStore::new(&uri, database_name)
        .await
        .unwrap();
    let owner = Uuid::new_v4();
    let other_owner = Uuid::new_v4();

    let owner_envelopes = OWNER_IDS
        .iter()
        .enumerate()
        .map(|(index, id)| envelope(owner, id, 0x10 + index as u8))
        .collect::<Vec<_>>();
    let other = envelope(other_owner, OTHER_OWNER_ID, 0x70);

    for item in owner_envelopes.iter().chain(std::iter::once(&other)) {
        let intent = HighEncryptedMemoAuthoritativeStore::save_envelope_with_projection_intent(
            &store, item,
        )
        .await
        .unwrap();
        assert_eq!(intent.target, ProjectionTarget::Version(1));
        HighEncryptedMemoAuthoritativeStore::acknowledge_encrypted_projection_intent(
            &store, &intent,
        )
        .await
        .unwrap();
    }

    let first = HighEncryptedMemoAuthoritativeStore::page_envelopes_by_owner(
        &store, owner, None, 3,
    )
    .await
    .unwrap();
    assert_eq!(
        first
            .iter()
            .map(|envelope| envelope.memo_id)
            .collect::<Vec<_>>(),
        OWNER_IDS[..3]
            .iter()
            .map(|value| Uuid::parse_str(value).unwrap())
            .collect::<Vec<_>>()
    );

    let cursor = Uuid::parse_str(OWNER_IDS[1]).unwrap();
    let second = HighEncryptedMemoAuthoritativeStore::page_envelopes_by_owner(
        &store,
        owner,
        Some(cursor),
        3,
    )
    .await
    .unwrap();
    assert_eq!(
        second
            .iter()
            .map(|envelope| envelope.memo_id)
            .collect::<Vec<_>>(),
        OWNER_IDS[2..]
            .iter()
            .map(|value| Uuid::parse_str(value).unwrap())
            .collect::<Vec<_>>()
    );
    assert!(!second
        .iter()
        .any(|envelope| envelope.owner_partition == other_owner));

    cleanup_client.database(database_name).drop().await.unwrap();
}
