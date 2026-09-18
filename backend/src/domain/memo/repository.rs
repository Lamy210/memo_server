use async_trait::async_trait;
use uuid::Uuid;

use super::entity::Memo;
use crate::error::AppResult;

#[derive(Debug)]
pub struct MemoSearchPage {
    pub items: Vec<Memo>,
    pub total: usize,
}

#[async_trait]
pub trait MemoRepository: Send + Sync {
    async fn find_by_id(&self, user_id: Uuid, id: Uuid) -> AppResult<Option<Memo>>;
    async fn find_all_by_user_id(&self, user_id: Uuid) -> AppResult<Vec<Memo>>;
    async fn save(&self, memo: &Memo) -> AppResult<()>;
    async fn delete(&self, user_id: Uuid, id: Uuid) -> AppResult<()>;
    async fn search(
        &self,
        query: &str,
        tag: Option<String>,
        user_id: Uuid,
        page: usize,
        limit: usize,
    ) -> AppResult<MemoSearchPage>;
    async fn exists(&self, user_id: Uuid, id: Uuid) -> AppResult<bool>;
}
