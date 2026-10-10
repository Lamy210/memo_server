use async_trait::async_trait;
use uuid::Uuid;

use super::entity::Memo;
use crate::error::{AppError, AppResult};

#[derive(Debug, Clone)]
pub struct MemoListPage {
    pub items: Vec<Memo>,
    pub has_more: bool,
}

#[derive(Debug, Clone)]
pub struct MemoSearchPage {
    pub items: Vec<Memo>,
    pub total: usize,
}

#[async_trait]
pub trait MemoRepository: Send + Sync {
    async fn find_by_id(&self, user_id: Uuid, id: Uuid) -> AppResult<Option<Memo>>;
    async fn list_page_by_user_id(
        &self,
        _user_id: Uuid,
        _after: Option<Uuid>,
        _limit: usize,
    ) -> AppResult<MemoListPage> {
        Err(AppError::ServiceUnavailable(
            "Bounded memo list pagination is not implemented for this repository".into(),
        ))
    }
    async fn find_many_by_ids(&self, user_id: Uuid, ids: &[Uuid]) -> AppResult<Vec<Memo>>;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memo_list_page_tracks_items_and_has_more() {
        let owner = Uuid::new_v4();
        let memo = Memo::new("title".into(), "content".into(), vec!["tag".into()], owner);

        let page = MemoListPage {
            items: vec![memo.clone()],
            has_more: true,
        };

        assert_eq!(page.items, vec![memo]);
        assert!(page.has_more);
    }
}
