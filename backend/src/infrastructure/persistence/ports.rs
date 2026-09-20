use std::time::Duration;

use async_trait::async_trait;
use uuid::Uuid;

use crate::{
    domain::memo::{
        entity::Memo,
        repository::MemoSearchPage,
    },
    error::AppResult,
};

pub const PROJECTION_RETRY_BUCKETS: i32 = 16;
pub const PROJECTION_DELETE_TARGET: i32 = -1;

#[derive(Debug, Clone)]
pub struct ProjectionIntent {
    pub bucket: i32,
    pub event_id: Uuid,
    pub user_id: Uuid,
    pub memo_id: Uuid,
    pub target_version: i32,
}

impl ProjectionIntent {
    pub fn new(user_id: Uuid, memo_id: Uuid, target_version: i32) -> Self {
        Self {
            bucket: projection_bucket(memo_id),
            event_id: Uuid::new_v4(),
            user_id,
            memo_id,
            target_version,
        }
    }
}

pub fn projection_bucket(memo_id: Uuid) -> i32 {
    (memo_id.as_u128() % PROJECTION_RETRY_BUCKETS as u128) as i32
}

#[async_trait]
pub trait MemoAuthoritativeStore: Send + Sync {
    async fn find_by_id(&self, user_id: Uuid, id: Uuid) -> AppResult<Option<Memo>>;
    async fn find_all_by_user_id(&self, user_id: Uuid) -> AppResult<Vec<Memo>>;
    async fn save(&self, memo: &Memo) -> AppResult<()>;
    async fn delete(&self, user_id: Uuid, id: Uuid) -> AppResult<()>;
    async fn exists(&self, user_id: Uuid, id: Uuid) -> AppResult<bool>;

    async fn enqueue_projection_intent(
        &self,
        user_id: Uuid,
        memo_id: Uuid,
        target_version: i32,
    ) -> AppResult<ProjectionIntent>;

    async fn list_projection_intents(&self, bucket: i32) -> AppResult<Vec<ProjectionIntent>>;

    async fn acknowledge_projection_intent(&self, event: &ProjectionIntent) -> AppResult<()>;
}

#[async_trait]
pub trait MemoCache: Send + Sync {
    async fn get_memo(&self, key: &str) -> AppResult<Option<Memo>>;
    async fn set_memo(
        &self,
        key: &str,
        memo: &Memo,
        expiration: Option<Duration>,
    ) -> AppResult<()>;
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
    fn projection_intent_bucket_is_stable_and_bounded() {
        let memo_id = Uuid::new_v4();

        let first = ProjectionIntent::new(Uuid::new_v4(), memo_id, 1);
        let second = ProjectionIntent::new(Uuid::new_v4(), memo_id, 2);

        assert_eq!(first.bucket, second.bucket);
        assert!((0..PROJECTION_RETRY_BUCKETS).contains(&first.bucket));
    }
}
