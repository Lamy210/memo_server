use memo_app_backend::{
    domain::memo::entity::Memo,
    infrastructure::persistence::{
        mongodb::MongoDbAuthoritativeStore, ports::MemoAuthoritativeStore, scylla::ScyllaDB,
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

fn memo(owner: Uuid, id: &str, title: &str) -> Memo {
    let mut memo = Memo::new(title.into(), "content".into(), Vec::new(), owner);
    memo.id = Uuid::parse_str(id).unwrap();
    memo
}

fn expected_owner_ids() -> Vec<Uuid> {
    OWNER_IDS
        .iter()
        .map(|value| Uuid::parse_str(value).unwrap())
        .collect()
}

async fn assert_two_page_contract(store: &dyn MemoAuthoritativeStore, owner: Uuid) {
    let expected = expected_owner_ids();

    let first = store.list_page_by_user_id(owner, None, 2).await.unwrap();
    assert_eq!(
        first.items.iter().map(|memo| memo.id).collect::<Vec<_>>(),
        expected[..2]
    );
    assert!(first.has_more);

    let cursor = first.items.last().unwrap().id;
    let second = store
        .list_page_by_user_id(owner, Some(cursor), 2)
        .await
        .unwrap();
    assert_eq!(
        second.items.iter().map(|memo| memo.id).collect::<Vec<_>>(),
        expected[2..]
    );
    assert!(!second.has_more);

    let mut combined = first
        .items
        .iter()
        .chain(second.items.iter())
        .map(|memo| memo.id)
        .collect::<Vec<_>>();
    combined.sort_unstable();
    combined.dedup();
    assert_eq!(combined.len(), 4);
    assert!(!combined.contains(&Uuid::parse_str(OTHER_OWNER_ID).unwrap()));
}

#[tokio::test]
#[ignore = "requires a local ScyllaDB instance"]
async fn scylla_list_page_is_owner_scoped_and_uses_exclusive_uuid_cursor() {
    let store = ScyllaDB::new("127.0.0.1:9042").await.unwrap();
    let owner = Uuid::new_v4();
    let other_owner = Uuid::new_v4();

    let owner_memos = OWNER_IDS
        .iter()
        .enumerate()
        .map(|(index, id)| memo(owner, id, &format!("owner-{index}")))
        .collect::<Vec<_>>();
    let other = memo(other_owner, OTHER_OWNER_ID, "other-owner");

    for item in owner_memos.iter().chain(std::iter::once(&other)) {
        store.save(item).await.unwrap();
    }

    assert_two_page_contract(&store, owner).await;

    for item in owner_memos.iter().chain(std::iter::once(&other)) {
        store.delete(item.user_id, item.id).await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires a local MongoDB replica set"]
async fn mongodb_list_page_is_owner_scoped_and_uses_exclusive_uuid_cursor() {
    let uri = std::env::var("MONGODB_TEST_URI")
        .unwrap_or_else(|_| "mongodb://localhost:27017/?replicaSet=rs0".to_string());
    let database_name = "memo_app_pagination_test";
    let cleanup_client = mongodb::Client::with_uri_str(&uri).await.unwrap();
    cleanup_client.database(database_name).drop().await.unwrap();

    let store = MongoDbAuthoritativeStore::new(&uri, database_name)
        .await
        .unwrap();
    let owner = Uuid::new_v4();
    let other_owner = Uuid::new_v4();

    let owner_memos = OWNER_IDS
        .iter()
        .enumerate()
        .map(|(index, id)| memo(owner, id, &format!("owner-{index}")))
        .collect::<Vec<_>>();
    let other = memo(other_owner, OTHER_OWNER_ID, "other-owner");

    for item in owner_memos.iter().chain(std::iter::once(&other)) {
        let intent = MemoAuthoritativeStore::save_with_projection_intent(&store, item)
            .await
            .unwrap();
        MemoAuthoritativeStore::acknowledge_projection_intent(&store, &intent)
            .await
            .unwrap();
    }

    assert_two_page_contract(&store, owner).await;

    cleanup_client.database(database_name).drop().await.unwrap();
}
