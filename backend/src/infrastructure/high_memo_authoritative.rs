// Staged MEMO-HIGH-1 authoritative adapter.
//
// This adapter is startup-composed only when MEMO-HIGH-1 AWS KMS runtime
// configuration is explicitly enabled. The persisted memo route still defaults
// to legacy plaintext, so composition alone does not make this store authoritative.
#![allow(dead_code)]

use std::sync::Arc;

use async_trait::async_trait;
use uuid::Uuid;

use crate::{
    application::crypto::{HighEncryptedMemoEnvelope, HighMemoCryptography},
    domain::memo::{entity::Memo, repository::MemoListPage},
    error::{AppError, AppResult},
    infrastructure::persistence::ports::{
        HighEncryptedMemoAuthoritativeStore, MemoAuthoritativeStore, ProjectionIntent,
        ProjectionTarget,
    },
};

pub(crate) struct HighMemoAuthoritativeAdapter {
    encrypted_store: Arc<dyn HighEncryptedMemoAuthoritativeStore>,
    cryptography: Arc<dyn HighMemoCryptography>,
}

impl HighMemoAuthoritativeAdapter {
    pub(crate) fn new(
        encrypted_store: Arc<dyn HighEncryptedMemoAuthoritativeStore>,
        cryptography: Arc<dyn HighMemoCryptography>,
    ) -> Self {
        Self {
            encrypted_store,
            cryptography,
        }
    }

    async fn decrypt_checked(&self, envelope: &HighEncryptedMemoEnvelope) -> AppResult<Memo> {
        envelope.validate_structure()?;
        let memo = self.cryptography.decrypt_memo(envelope).await?;

        if memo.id != envelope.memo_id
            || memo.user_id != envelope.owner_partition
            || memo.version != envelope.version
            || !memo.validate()
        {
            return Err(AppError::DatabaseError(
                "Decrypted HIGH authoritative memo identity/version is inconsistent".into(),
            ));
        }

        Ok(memo)
    }

    fn validate_projection_event(
        memo: &Memo,
        event: &ProjectionIntent,
        expected: ProjectionTarget,
    ) -> AppResult<()> {
        if event.user_id != memo.user_id || event.memo_id != memo.id || event.target != expected {
            return Err(AppError::InternalServerError(
                "Encrypted authoritative store returned an inconsistent projection intent".into(),
            ));
        }
        Ok(())
    }

    fn validate_delete_event(
        owner_partition: Uuid,
        memo_id: Uuid,
        event: &ProjectionIntent,
    ) -> AppResult<()> {
        if event.user_id != owner_partition
            || event.memo_id != memo_id
            || event.target != ProjectionTarget::Deleted
        {
            return Err(AppError::InternalServerError(
                "Encrypted authoritative store returned an inconsistent delete projection intent"
                    .into(),
            ));
        }
        Ok(())
    }
}

#[async_trait]
impl MemoAuthoritativeStore for HighMemoAuthoritativeAdapter {
    async fn find_by_id(&self, user_id: Uuid, id: Uuid) -> AppResult<Option<Memo>> {
        match self
            .encrypted_store
            .find_envelope_by_id(user_id, id)
            .await?
        {
            Some(envelope) => self.decrypt_checked(&envelope).await.map(Some),
            None => Ok(None),
        }
    }

    async fn list_page_by_user_id(
        &self,
        user_id: Uuid,
        after: Option<Uuid>,
        limit: usize,
    ) -> AppResult<MemoListPage> {
        let physical_limit = limit.checked_add(1).ok_or_else(|| {
            AppError::DatabaseError("HIGH authoritative memo page size is too large".into())
        })?;
        let mut envelopes = self
            .encrypted_store
            .page_envelopes_by_owner(user_id, after, physical_limit)
            .await?;
        let has_more = envelopes.len() > limit;
        envelopes.truncate(limit);

        let mut items = Vec::with_capacity(envelopes.len());
        for envelope in envelopes {
            items.push(self.decrypt_checked(&envelope).await?);
        }

        Ok(MemoListPage { items, has_more })
    }

    async fn find_many_by_ids(&self, user_id: Uuid, ids: &[Uuid]) -> AppResult<Vec<Memo>> {
        let envelopes = self
            .encrypted_store
            .find_many_envelopes_by_ids(user_id, ids)
            .await?;
        let mut memos = Vec::with_capacity(envelopes.len());
        for envelope in envelopes {
            memos.push(self.decrypt_checked(&envelope).await?);
        }
        Ok(memos)
    }

    async fn save_with_projection_intent(&self, memo: &Memo) -> AppResult<ProjectionIntent> {
        if !memo.validate() {
            return Err(AppError::ValidationError(
                "Memo violates domain invariants before HIGH authoritative encryption".into(),
            ));
        }

        let envelope = self.cryptography.encrypt_memo(memo).await?;
        if envelope.memo_id != memo.id
            || envelope.owner_partition != memo.user_id
            || envelope.version != memo.version
        {
            return Err(AppError::InternalServerError(
                "HIGH encryption returned an envelope for a different memo identity/version".into(),
            ));
        }

        let event = self
            .encrypted_store
            .save_envelope_with_projection_intent(&envelope)
            .await?;
        Self::validate_projection_event(memo, &event, ProjectionTarget::Version(memo.version))?;
        Ok(event)
    }

    async fn delete_with_projection_intent(
        &self,
        user_id: Uuid,
        id: Uuid,
    ) -> AppResult<ProjectionIntent> {
        let event = self
            .encrypted_store
            .delete_envelope_with_projection_intent(user_id, id)
            .await?;
        Self::validate_delete_event(user_id, id, &event)?;
        Ok(event)
    }

    async fn exists(&self, user_id: Uuid, id: Uuid) -> AppResult<bool> {
        Ok(self.find_by_id(user_id, id).await?.is_some())
    }

    async fn enqueue_projection_intent(
        &self,
        user_id: Uuid,
        memo_id: Uuid,
        target: ProjectionTarget,
    ) -> AppResult<ProjectionIntent> {
        self.encrypted_store
            .enqueue_encrypted_projection_intent(user_id, memo_id, target)
            .await
    }

    async fn list_projection_intents(&self) -> AppResult<Vec<ProjectionIntent>> {
        self.encrypted_store
            .list_encrypted_projection_intents()
            .await
    }

    async fn acknowledge_projection_intent(&self, event: &ProjectionIntent) -> AppResult<()> {
        self.encrypted_store
            .acknowledge_encrypted_projection_intent(event)
            .await
    }
}

#[cfg(test)]
mod tests {
    use std::{
        cmp::Reverse,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Mutex,
        },
    };

    use chrono::{TimeZone, Utc};

    use super::*;
    use crate::application::crypto::{
        HighEncryptedMemoEnvelope, MEMO_HIGH_SCHEMA_VERSION, MEMO_HIGH_SUITE_ID,
    };

    #[derive(Default)]
    struct FakeEncryptedStore {
        envelopes: Mutex<Vec<HighEncryptedMemoEnvelope>>,
        intents: Mutex<Vec<ProjectionIntent>>,
        unbounded_reads: AtomicUsize,
        page_limits: Mutex<Vec<usize>>,
    }

    #[async_trait]
    impl HighEncryptedMemoAuthoritativeStore for FakeEncryptedStore {
        async fn find_envelope_by_id(
            &self,
            owner_partition: Uuid,
            memo_id: Uuid,
        ) -> AppResult<Option<HighEncryptedMemoEnvelope>> {
            Ok(self
                .envelopes
                .lock()
                .unwrap()
                .iter()
                .find(|envelope| {
                    envelope.owner_partition == owner_partition && envelope.memo_id == memo_id
                })
                .cloned())
        }

        async fn find_all_envelopes_by_owner(
            &self,
            owner_partition: Uuid,
        ) -> AppResult<Vec<HighEncryptedMemoEnvelope>> {
            self.unbounded_reads.fetch_add(1, Ordering::SeqCst);
            Ok(self
                .envelopes
                .lock()
                .unwrap()
                .iter()
                .filter(|envelope| envelope.owner_partition == owner_partition)
                .cloned()
                .collect())
        }

        async fn page_envelopes_by_owner(
            &self,
            owner_partition: Uuid,
            after: Option<Uuid>,
            limit: usize,
        ) -> AppResult<Vec<HighEncryptedMemoEnvelope>> {
            self.page_limits.lock().unwrap().push(limit);
            let mut envelopes = self
                .envelopes
                .lock()
                .unwrap()
                .iter()
                .filter(|envelope| envelope.owner_partition == owner_partition)
                .filter(|envelope| after.is_none_or(|cursor| envelope.memo_id < cursor))
                .cloned()
                .collect::<Vec<_>>();
            envelopes.sort_by_key(|envelope| Reverse(envelope.memo_id));
            envelopes.truncate(limit);
            Ok(envelopes)
        }

        async fn find_many_envelopes_by_ids(
            &self,
            owner_partition: Uuid,
            memo_ids: &[Uuid],
        ) -> AppResult<Vec<HighEncryptedMemoEnvelope>> {
            let envelopes = self.envelopes.lock().unwrap();
            Ok(memo_ids
                .iter()
                .filter_map(|memo_id| {
                    envelopes
                        .iter()
                        .find(|envelope| {
                            envelope.owner_partition == owner_partition
                                && envelope.memo_id == *memo_id
                        })
                        .cloned()
                })
                .collect())
        }

        async fn save_envelope_with_projection_intent(
            &self,
            envelope: &HighEncryptedMemoEnvelope,
        ) -> AppResult<ProjectionIntent> {
            self.envelopes.lock().unwrap().push(envelope.clone());
            let event = ProjectionIntent::new(
                envelope.owner_partition,
                envelope.memo_id,
                ProjectionTarget::Version(envelope.version),
            );
            self.intents.lock().unwrap().push(event.clone());
            Ok(event)
        }

        async fn delete_envelope_with_projection_intent(
            &self,
            owner_partition: Uuid,
            memo_id: Uuid,
        ) -> AppResult<ProjectionIntent> {
            self.envelopes.lock().unwrap().retain(|envelope| {
                envelope.owner_partition != owner_partition || envelope.memo_id != memo_id
            });
            let event = ProjectionIntent::new(owner_partition, memo_id, ProjectionTarget::Deleted);
            self.intents.lock().unwrap().push(event.clone());
            Ok(event)
        }

        async fn enqueue_encrypted_projection_intent(
            &self,
            user_id: Uuid,
            memo_id: Uuid,
            target: ProjectionTarget,
        ) -> AppResult<ProjectionIntent> {
            let event = ProjectionIntent::new(user_id, memo_id, target);
            self.intents.lock().unwrap().push(event.clone());
            Ok(event)
        }

        async fn list_encrypted_projection_intents(&self) -> AppResult<Vec<ProjectionIntent>> {
            Ok(self.intents.lock().unwrap().clone())
        }

        async fn acknowledge_encrypted_projection_intent(
            &self,
            event: &ProjectionIntent,
        ) -> AppResult<()> {
            self.intents
                .lock()
                .unwrap()
                .retain(|candidate| candidate.event_id != event.event_id);
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeCrypto {
        decrypts: AtomicUsize,
    }

    #[async_trait]
    impl HighMemoCryptography for FakeCrypto {
        async fn encrypt_memo(&self, memo: &Memo) -> AppResult<HighEncryptedMemoEnvelope> {
            let payload = serde_json::to_vec(&(
                memo.title.clone(),
                memo.content.clone(),
                memo.tags.clone(),
                memo.created_at.timestamp_millis(),
                memo.updated_at.timestamp_millis(),
            ))
            .unwrap();

            Ok(HighEncryptedMemoEnvelope {
                memo_id: memo.id,
                owner_partition: memo.user_id,
                ciphertext: payload,
                nonce: vec![0xBB; 12],
                wrapped_dek: vec![0xCC; 48],
                version: memo.version,
                crypto_suite_id: MEMO_HIGH_SUITE_ID.into(),
                key_version: "memo-key-v1".into(),
                schema_version: MEMO_HIGH_SCHEMA_VERSION,
            })
        }

        async fn decrypt_memo(&self, envelope: &HighEncryptedMemoEnvelope) -> AppResult<Memo> {
            self.decrypts.fetch_add(1, Ordering::SeqCst);
            let (title, content, tags, created_at_ms, updated_at_ms): (
                String,
                String,
                Vec<String>,
                i64,
                i64,
            ) = serde_json::from_slice(&envelope.ciphertext).unwrap();

            Ok(Memo {
                id: envelope.memo_id,
                title,
                content,
                tags,
                user_id: envelope.owner_partition,
                created_at: Utc.timestamp_millis_opt(created_at_ms).unwrap(),
                updated_at: Utc.timestamp_millis_opt(updated_at_ms).unwrap(),
                version: envelope.version,
            })
        }
    }

    fn memo(owner: Uuid, id: Uuid, version: i32, updated_at_ms: i64) -> Memo {
        Memo {
            id,
            title: format!("title-{version}"),
            content: "content".into(),
            tags: vec!["tag".into()],
            user_id: owner,
            created_at: Utc.timestamp_millis_opt(1_700_000_000_000).unwrap(),
            updated_at: Utc.timestamp_millis_opt(updated_at_ms).unwrap(),
            version,
        }
    }

    fn adapter(store: Arc<FakeEncryptedStore>) -> HighMemoAuthoritativeAdapter {
        HighMemoAuthoritativeAdapter::new(store, Arc::new(FakeCrypto::default()))
    }

    #[tokio::test]
    async fn encrypted_round_trip_preserves_domain_contract_and_projection_intent() {
        let owner = Uuid::new_v4();
        let id = Uuid::new_v4();
        let store = Arc::new(FakeEncryptedStore::default());
        let adapter = adapter(store.clone());
        let memo = memo(owner, id, 1, 1_700_000_001_000);

        let event = adapter.save_with_projection_intent(&memo).await.unwrap();
        assert_eq!(event.target, ProjectionTarget::Version(1));

        let restored = adapter.find_by_id(owner, id).await.unwrap().unwrap();
        assert_eq!(restored.id, memo.id);
        assert_eq!(restored.user_id, memo.user_id);
        assert_eq!(restored.title, memo.title);
        assert_eq!(restored.content, memo.content);
        assert_eq!(restored.tags, memo.tags);
        assert_eq!(restored.version, memo.version);
        assert_eq!(
            restored.created_at.timestamp_millis(),
            memo.created_at.timestamp_millis()
        );
        assert_eq!(
            restored.updated_at.timestamp_millis(),
            memo.updated_at.timestamp_millis()
        );
        assert!(adapter.exists(owner, id).await.unwrap());
        assert_eq!(store.intents.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn bounded_page_uses_limit_plus_one_without_unbounded_reads_or_probe_decryption() {
        let owner = Uuid::new_v4();
        let other_owner = Uuid::new_v4();
        let store = Arc::new(FakeEncryptedStore::default());
        let crypto = Arc::new(FakeCrypto::default());
        let adapter = HighMemoAuthoritativeAdapter::new(store.clone(), crypto.clone());
        let ids = [
            "550e8400-e29b-41d4-a716-446655440004",
            "550e8400-e29b-41d4-a716-446655440003",
            "550e8400-e29b-41d4-a716-446655440002",
            "550e8400-e29b-41d4-a716-446655440001",
        ];

        for (index, id) in ids.iter().enumerate() {
            let memo = memo(
                owner,
                Uuid::parse_str(id).unwrap(),
                1,
                1_700_000_001_000 + index as i64,
            );
            let envelope = crypto.encrypt_memo(&memo).await.unwrap();
            store.envelopes.lock().unwrap().push(envelope);
        }
        let other = memo(
            other_owner,
            Uuid::parse_str("550e8400-e29b-41d4-a716-4466554400ff").unwrap(),
            1,
            1_700_000_010_000,
        );
        let other_envelope = crypto.encrypt_memo(&other).await.unwrap();
        store.envelopes.lock().unwrap().push(other_envelope);

        let first = adapter.list_page_by_user_id(owner, None, 2).await.unwrap();
        assert!(first.has_more);
        assert_eq!(
            first.items.iter().map(|memo| memo.id).collect::<Vec<_>>(),
            ids[..2]
                .iter()
                .map(|id| Uuid::parse_str(id).unwrap())
                .collect::<Vec<_>>()
        );

        let second = adapter
            .list_page_by_user_id(owner, Some(first.items[1].id), 2)
            .await
            .unwrap();
        assert!(!second.has_more);
        assert_eq!(
            second.items.iter().map(|memo| memo.id).collect::<Vec<_>>(),
            ids[2..]
                .iter()
                .map(|id| Uuid::parse_str(id).unwrap())
                .collect::<Vec<_>>()
        );
        assert_eq!(*store.page_limits.lock().unwrap(), vec![3, 3]);
        assert_eq!(store.unbounded_reads.load(Ordering::SeqCst), 0);
        assert_eq!(crypto.decrypts.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn ordered_bulk_hydration_and_delete_preserve_existing_contracts() {
        let owner = Uuid::new_v4();
        let store = Arc::new(FakeEncryptedStore::default());
        let adapter = adapter(store);

        let first = memo(owner, Uuid::new_v4(), 1, 1_700_000_001_000);
        let second = memo(owner, Uuid::new_v4(), 1, 1_700_000_002_000);
        adapter.save_with_projection_intent(&first).await.unwrap();
        adapter.save_with_projection_intent(&second).await.unwrap();

        let hydrated = adapter
            .find_many_by_ids(owner, &[second.id, Uuid::new_v4(), first.id])
            .await
            .unwrap();
        assert_eq!(
            hydrated.iter().map(|memo| memo.id).collect::<Vec<_>>(),
            vec![second.id, first.id]
        );

        let deleted = adapter
            .delete_with_projection_intent(owner, first.id)
            .await
            .unwrap();
        assert_eq!(deleted.target, ProjectionTarget::Deleted);
        assert!(!adapter.exists(owner, first.id).await.unwrap());
    }
}
