use async_trait::async_trait;
use memo_app_backend::{
    application::crypto::{
        HighEncryptedMemoEnvelope, MEMO_HIGH_SCHEMA_VERSION, MEMO_HIGH_SUITE_ID,
    },
    error::AppResult,
    infrastructure::persistence::ports::{
        HighEncryptedMemoAuthoritativeStore, ProjectionIntent, ProjectionTarget,
    },
};
use std::sync::Mutex;
use uuid::Uuid;

const OWNER_IDS: [&str; 4] = [
    "550e8400-e29b-41d4-a716-446655440004",
    "550e8400-e29b-41d4-a716-446655440003",
    "550e8400-e29b-41d4-a716-446655440002",
    "550e8400-e29b-41d4-a716-446655440001",
];
const OTHER_OWNER_ID: &str = "550e8400-e29b-41d4-a716-4466554400ff";

#[derive(Default)]
struct FakeEncryptedStore {
    envelopes: Mutex<Vec<HighEncryptedMemoEnvelope>>,
}

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

#[async_trait]
impl HighEncryptedMemoAuthoritativeStore for FakeEncryptedStore {
    async fn find_envelope_by_id(
        &self,
        _owner_partition: Uuid,
        _memo_id: Uuid,
    ) -> AppResult<Option<HighEncryptedMemoEnvelope>> {
        Ok(None)
    }

    async fn page_envelopes_by_owner(
        &self,
        owner_partition: Uuid,
        after: Option<Uuid>,
        limit: usize,
    ) -> AppResult<Vec<HighEncryptedMemoEnvelope>> {
        let mut envelopes = self
            .envelopes
            .lock()
            .unwrap()
            .iter()
            .filter(|envelope| envelope.owner_partition == owner_partition)
            .filter(|envelope| after.is_none_or(|cursor| envelope.memo_id < cursor))
            .cloned()
            .collect::<Vec<_>>();
        envelopes.sort_by_key(|envelope| std::cmp::Reverse(envelope.memo_id));
        envelopes.truncate(limit);
        Ok(envelopes)
    }

    async fn find_many_envelopes_by_ids(
        &self,
        _owner_partition: Uuid,
        _memo_ids: &[Uuid],
    ) -> AppResult<Vec<HighEncryptedMemoEnvelope>> {
        Ok(Vec::new())
    }

    async fn save_envelope_with_projection_intent(
        &self,
        envelope: &HighEncryptedMemoEnvelope,
    ) -> AppResult<ProjectionIntent> {
        Ok(ProjectionIntent::new(
            envelope.owner_partition,
            envelope.memo_id,
            ProjectionTarget::Version(envelope.version),
        ))
    }

    async fn delete_envelope_with_projection_intent(
        &self,
        owner_partition: Uuid,
        memo_id: Uuid,
    ) -> AppResult<ProjectionIntent> {
        Ok(ProjectionIntent::new(
            owner_partition,
            memo_id,
            ProjectionTarget::Deleted,
        ))
    }

    async fn enqueue_encrypted_projection_intent(
        &self,
        user_id: Uuid,
        memo_id: Uuid,
        target: ProjectionTarget,
    ) -> AppResult<ProjectionIntent> {
        Ok(ProjectionIntent::new(user_id, memo_id, target))
    }

    async fn list_encrypted_projection_intents(&self) -> AppResult<Vec<ProjectionIntent>> {
        Ok(Vec::new())
    }

    async fn acknowledge_encrypted_projection_intent(
        &self,
        _event: &ProjectionIntent,
    ) -> AppResult<()> {
        Ok(())
    }
}

#[tokio::test]
async fn encrypted_page_contract_is_owner_scoped_descending_and_exclusive() {
    let owner = Uuid::new_v4();
    let other_owner = Uuid::new_v4();
    let store = FakeEncryptedStore::default();

    {
        let mut envelopes = store.envelopes.lock().unwrap();
        envelopes.extend(OWNER_IDS.map(|id| envelope(owner, id)));
        envelopes.push(envelope(other_owner, OTHER_OWNER_ID));
    }

    let first = store.page_envelopes_by_owner(owner, None, 3).await.unwrap();
    assert_eq!(
        first
            .iter()
            .map(|envelope| envelope.memo_id)
            .collect::<Vec<_>>(),
        OWNER_IDS[..3]
            .iter()
            .map(|id| Uuid::parse_str(id).unwrap())
            .collect::<Vec<_>>()
    );

    let second = store
        .page_envelopes_by_owner(owner, Some(first[1].memo_id), 3)
        .await
        .unwrap();
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
}
