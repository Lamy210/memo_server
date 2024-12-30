use async_trait::async_trait;
use uuid::Uuid;
use crate::error::AppResult;
use super::entity::Memo;

#[async_trait]
pub trait MemoRepository: Send + Sync {
    async fn find_by_id(&self, id: Uuid) -> AppResult<Option<Memo>>;
    async fn find_all_by_user_id(&self, user_id: Uuid) -> AppResult<Vec<Memo>>;
    async fn save(&self, memo: &Memo) -> AppResult<()>;
    async fn delete(&self, id: Uuid) -> AppResult<()>;
    async fn search(&self, query: &str, tag: Option<String>, user_id: Uuid) -> AppResult<Vec<Memo>>;
    async fn exists(&self, id: Uuid) -> AppResult<bool>;
}