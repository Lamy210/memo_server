use std::time::Duration;

use async_trait::async_trait;
use uuid::Uuid;

use crate::{
    application::crypto::HighEncryptedMemoEnvelope, domain::memo::entity::Memo, error::AppResult,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionTarget {
    Version(i32),
    Deleted,
}

#[derive(Debug, Clone)]
pub struct ProjectionIntent {
    pub event_id: Uuid,
    pub user_id: Uuid,
    pub memo_id: Uuid,
    pub target: ProjectionTarget,
}

impl ProjectionIntent {
    pub fn new(user_id: Uuid, memo_id: Uuid, target: ProjectionTarget) -> Self {
        Self {
            event_id: Uuid::new_v4(),
            user_id,
            memo_id,
            target,
        }
    }
}

#[async_trait]
pub trait LegacyMemoPlaintextRetirementInspector: Send + Sync {
    /// Exact count of plaintext authoritative memo documents.
    ///
    /// Callers must hold the shared maintenance window so the count can be
    /// used as a stable dry-run retirement plan input.
    async fn count_plaintext_memos_for_retirement(&self) -> AppResult<u64>;
}

#[async_trait]
pub trait LegacyMemoPlaintextRetirementAdmin:
    LegacyMemoPlaintextRetirementInspector + Send + Sync
{
    /// Irreversibly delete every plaintext authoritative memo document.
    ///
    /// Callers must first advance the shared retirement state to
    /// `in_progress` while holding the maintenance window. The operation is
    /// intentionally resumable: a partial/ambiguous delete leaves retirement
    /// fenced and a later operator invocation can continue from the remaining
    /// document count.
    async fn delete_all_plaintext_memos_for_retirement(&self) -> AppResult<u64>;
}

#[async_trait]
pub trait HighEncryptedMemoIntegritySource: Send + Sync {
    async fn count_encrypted_memos_for_integrity(&self) -> AppResult<u64>;

    /// Stable bounded traversal of the encrypted authoritative collection.
    /// Callers must hold the shared maintenance window so the cursor cannot
    /// race inserts/deletes while verifying retirement readiness.
    async fn page_encrypted_memos_for_integrity(
        &self,
        after: Option<Uuid>,
        limit: usize,
    ) -> AppResult<Vec<HighEncryptedMemoEnvelope>>;
}

#[async_trait]
pub trait HighEncryptedMemoAuthoritativeStore: Send + Sync {
    async fn find_envelope_by_id(
        &self,
        owner_partition: Uuid,
        memo_id: Uuid,
    ) -> AppResult<Option<HighEncryptedMemoEnvelope>>;

    async fn find_all_envelopes_by_owner(
        &self,
        owner_partition: Uuid,
    ) -> AppResult<Vec<HighEncryptedMemoEnvelope>>;

    /// Load encrypted envelopes in the same order as the requested IDs.
    /// Missing IDs are omitted.
    async fn find_many_envelopes_by_ids(
        &self,
        owner_partition: Uuid,
        memo_ids: &[Uuid],
    ) -> AppResult<Vec<HighEncryptedMemoEnvelope>>;

    /// Persist an encrypted authoritative memo and its projection intent in one
    /// atomic transaction.
    async fn save_envelope_with_projection_intent(
        &self,
        envelope: &HighEncryptedMemoEnvelope,
    ) -> AppResult<ProjectionIntent>;

    /// Delete an encrypted authoritative memo and persist its projection intent
    /// in the same atomic transaction.
    async fn delete_envelope_with_projection_intent(
        &self,
        owner_partition: Uuid,
        memo_id: Uuid,
    ) -> AppResult<ProjectionIntent>;

    async fn enqueue_encrypted_projection_intent(
        &self,
        user_id: Uuid,
        memo_id: Uuid,
        target: ProjectionTarget,
    ) -> AppResult<ProjectionIntent>;

    async fn list_encrypted_projection_intents(&self) -> AppResult<Vec<ProjectionIntent>>;

    async fn acknowledge_encrypted_projection_intent(
        &self,
        event: &ProjectionIntent,
    ) -> AppResult<()>;
}

#[async_trait]
pub trait MemoAuthoritativeStore: Send + Sync {
    async fn find_by_id(&self, user_id: Uuid, id: Uuid) -> AppResult<Option<Memo>>;
    async fn find_all_by_user_id(&self, user_id: Uuid) -> AppResult<Vec<Memo>>;

    /// Load authoritative memos in the same order as the requested IDs.
    ///
    /// Missing IDs are omitted. Implementations may optimize this as a bulk
    /// read, but must preserve the input ordering for the memos they return.
    async fn find_many_by_ids(&self, user_id: Uuid, ids: &[Uuid]) -> AppResult<Vec<Memo>>;

    /// Persist a memo mutation together with a durable projection intent.
    ///
    /// A successful return guarantees that both the primary mutation and its
    /// projection intent are durable. Transaction-capable stores should commit
    /// both records atomically.
    async fn save_with_projection_intent(&self, memo: &Memo) -> AppResult<ProjectionIntent>;

    /// Persist a memo deletion together with a durable projection intent.
    ///
    /// A successful return guarantees that the delete and its projection
    /// intent are durable. Transaction-capable stores should commit both
    /// records atomically.
    async fn delete_with_projection_intent(
        &self,
        user_id: Uuid,
        id: Uuid,
    ) -> AppResult<ProjectionIntent>;

    async fn exists(&self, user_id: Uuid, id: Uuid) -> AppResult<bool>;

    /// Enqueue a corrective intent for an already-observed source state.
    async fn enqueue_projection_intent(
        &self,
        user_id: Uuid,
        memo_id: Uuid,
        target: ProjectionTarget,
    ) -> AppResult<ProjectionIntent>;

    async fn list_projection_intents(&self) -> AppResult<Vec<ProjectionIntent>>;

    async fn acknowledge_projection_intent(&self, event: &ProjectionIntent) -> AppResult<()>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LegacyMemoCacheSweepStats {
    pub scanned_candidates: u64,
    pub legacy_keys: u64,
    pub deleted_keys: u64,
}

#[async_trait]
pub trait LegacyMemoPlaintextCacheMaintenance: Send + Sync {
    async fn inspect_legacy_plaintext_memo_cache(
        &self,
        scan_count: usize,
    ) -> AppResult<LegacyMemoCacheSweepStats>;

    async fn purge_legacy_plaintext_memo_cache(
        &self,
        scan_count: usize,
    ) -> AppResult<LegacyMemoCacheSweepStats>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HighMemoCacheSweepStats {
    pub scanned_candidates: u64,
    pub encrypted_keys: u64,
    pub deleted_keys: u64,
}

#[async_trait]
pub trait HighMemoCiphertextCacheMaintenance: Send + Sync {
    async fn inspect_high_encrypted_memo_cache(
        &self,
        scan_count: usize,
    ) -> AppResult<HighMemoCacheSweepStats>;

    async fn purge_high_encrypted_memo_cache(
        &self,
        scan_count: usize,
    ) -> AppResult<HighMemoCacheSweepStats>;
}

#[async_trait]
pub trait MemoCache: Send + Sync {
    async fn get_memo(&self, owner_partition: Uuid, memo_id: Uuid) -> AppResult<Option<Memo>>;

    async fn set_memo(&self, memo: &Memo, expiration: Option<Duration>) -> AppResult<()>;

    async fn delete_memo(&self, owner_partition: Uuid, memo_id: Uuid) -> AppResult<()>;

    async fn memo_exists(&self, owner_partition: Uuid, memo_id: Uuid) -> AppResult<bool>;
}

#[derive(Debug, Clone)]
pub struct MemoSearchHitPage {
    pub memo_ids: Vec<Uuid>,
    pub total: usize,
}

#[async_trait]
pub trait MemoSearchProjection: Send + Sync {
    async fn index_memo(&self, memo: &Memo) -> AppResult<()>;
    async fn search_memo_ids(
        &self,
        query: &str,
        tag: Option<String>,
        user_id: Uuid,
        page: usize,
        limit: usize,
    ) -> AppResult<MemoSearchHitPage>;
    async fn delete_memo(&self, id: Uuid) -> AppResult<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_intents_receive_unique_event_ids() {
        let user_id = Uuid::new_v4();
        let memo_id = Uuid::new_v4();

        let first = ProjectionIntent::new(user_id, memo_id, ProjectionTarget::Version(1));
        let second = ProjectionIntent::new(user_id, memo_id, ProjectionTarget::Deleted);

        assert_ne!(first.event_id, second.event_id);
        assert_eq!(first.memo_id, memo_id);
        assert_eq!(first.target, ProjectionTarget::Version(1));
        assert_eq!(second.target, ProjectionTarget::Deleted);
    }
}
