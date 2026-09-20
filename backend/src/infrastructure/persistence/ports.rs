use std::time::Duration;

use async_trait::async_trait;
use uuid::Uuid;

use crate::{
    domain::memo::{entity::Memo, repository::MemoSearchPage},
    error::AppResult,
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
pub trait MemoAuthoritativeStore: Send + Sync {
    async fn find_by_id(&self, user_id: Uuid, id: Uuid) -> AppResult<Option<Memo>>;
    async fn find_all_by_user_id(&self, user_id: Uuid) -> AppResult<Vec<Memo>>;

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

#[async_trait]
pub trait MemoCache: Send + Sync {
    async fn get_memo(&self, key: &str) -> AppResult<Option<Memo>>;
    async fn set_memo(&self, key: &str, memo: &Memo, expiration: Option<Duration>)
        -> AppResult<()>;
    async fn delete(&self, key: &str) -> AppResult<()>;
    async fn exists(&self, key: &str) -> AppResult<bool>;
}

#[async_trait]
pub trait MemoSearchProjection: Send + Sync {
    async fn index_memo(&self, memo: &Memo) -> AppResult<()>;
    async fn search_memos(
        &self,
        query: &str,
        tag: Option<String>,
        user_id: Uuid,
        page: usize,
        limit: usize,
    ) -> AppResult<MemoSearchPage>;
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
