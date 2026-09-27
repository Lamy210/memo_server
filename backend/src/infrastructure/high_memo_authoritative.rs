// Staged MEMO-HIGH-1 authoritative adapter.
//
// This adapter is intentionally not startup-wired yet. It proves that the
// encrypted MongoDB collection can satisfy the existing domain-facing
// MemoAuthoritativeStore contract without persisting semantic plaintext.
#![allow(dead_code)]

use std::sync::Arc;

use async_trait::async_trait;
use uuid::Uuid;

use crate::{
    application::crypto::{HighEncryptedMemoEnvelope, HighMemoCryptography},
    domain::memo::entity::Memo,
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

    async fn find_all_by_user_id(&self, user_id: Uuid) -> AppResult<Vec<Memo>> {
        let envelopes = self
            .encrypted_store
            .find_all_envelopes_by_owner(user_id)
            .await?;
        let mut memos = Vec::with_capacity(envelopes.len());
        for envelope in envelopes {
            memos.push(self.decrypt_checked(&envelope).await?);
        }

        // updated_at is intentionally encrypted inside the payload, so the
        // persistence adapter cannot sort on it without leaking new metadata.
        memos.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
        Ok(memos)
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
        Self::validate_projection_event(
            memo,
            &event,
            ProjectionTarget::Version(memo.version),
        )?;
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
            .enqueue_projection_intent(user_id, memo_id, target)
            .await
    }

    async fn list_projection_intents(&self) -> AppResult<Vec<ProjectionIntent>> {
        self.encrypted_store.list_projection_intents().await
    }

    async fn acknowledge_projection_intent(&self, event: &ProjectionIntent) -> AppResult<()> {
        self.encrypted_store
            .acknowledge_projection_intent(event)
            .await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use chrono::{TimeZone, Utc};

    use super::*;
    use crate::application::crypto::{
        HighEncryptedMemoEnvelope, MEMO_HIGH_SCHEMA_VERSION, MEMO_HIGH_SUITE_ID,
    };

    #[derive(Default)]
    struct FakeEncryptedStore {
        envelopes: Mutex<Vec<HighEncryptedMemoEnvelope>>,
        intents: Mutex<Vec<ProjectionIntent>>,
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
            Ok(self
                .envelopes
                .lock()
                .unwrap()
                .iter()
                .filter(|envelope| envelope.owner_partition == owner_partition)
                .cloned()
                .collect())
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
            let event =
                ProjectionIntent::new(owner_partition, memo_id, ProjectionTarget::Deleted);
            self.intents.lock().unwrap().push(event.clone());
            Ok(event)
        }

        async fn enqueue_projection_intent(
            &self,
            user_id: Uuid,
            memo_id: Uuid,
            target: ProjectionTarget,
        ) -> AppResult<ProjectionIntent> {
            let event = ProjectionIntent::new(user_id, memo_id, target);
            self.intents.lock().unwrap().push(event.clone());
            Ok(event)
        }

        async fn list_projection_intents(&self) -> AppResult<Vec<ProjectionIntent>> {
            Ok(self.intents.lock().unwrap().clone())
        }

        async fn acknowledge_projection_intent(&self, event: &ProjectionIntent) -> AppResult<()> {
            self.intents
                .lock()
                .unwrap()
                .retain(|candidate| candidate.event_id != event.event_id);
            Ok(())
        }
    }

    struct FakeCrypto;

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
        HighMemoAuthoritativeAdapter::new(store, Arc::new(FakeCrypto))
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
        assert_eq!(restored, memo);
        assert!(adapter.exists(owner, id).await.unwrap());
        assert_eq!(store.intents.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn list_sorts_after_decrypt_without_plaintext_sort_metadata() {
        let owner = Uuid::new_v4();
        let store = Arc::new(FakeEncryptedStore::default());
        let adapter = adapter(store.clone());

        let older = memo(owner, Uuid::new_v4(), 1, 1_700_000_001_000);
        let newer = memo(owner, Uuid::new_v4(), 1, 1_700_000_010_000);
        adapter.save_with_projection_intent(&older).await.unwrap();
        adapter.save_with_projection_intent(&newer).await.unwrap();

        let listed = adapter.find_all_by_user_id(owner).await.unwrap();
        assert_eq!(listed.iter().map(|memo| memo.id).collect::<Vec<_>>(), vec![
            newer.id, older.id
        ]);
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
