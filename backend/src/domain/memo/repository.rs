use async_trait::async_trait;
use uuid::Uuid;

use super::entity::Memo;
use crate::error::{AppError, AppResult};

#[derive(Debug)]
pub struct MemoListPage {
    pub items: Vec<Memo>,
    pub has_more: bool,
}

#[derive(Debug)]
pub struct MemoSearchPage {
    pub items: Vec<Memo>,
    pub total: usize,
}

#[async_trait]
pub trait MemoRepository: Send + Sync {
    async fn find_by_id(&self, user_id: Uuid, id: Uuid) -> AppResult<Option<Memo>>;
    async fn find_all_by_user_id(&self, user_id: Uuid) -> AppResult<Vec<Memo>>;
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
    use uuid::Uuid;

    use super::MemoListPage;
    use crate::domain::memo::entity::Memo;

    #[test]
    fn memo_list_page_tracks_items_and_has_more() {
        let user_id = Uuid::new_v4();
        let first = Memo::new("first".into(), "content".into(), Vec::new(), user_id);
        let second = Memo::new("second".into(), "content".into(), Vec::new(), user_id);

        let page = MemoListPage {
            items: vec![first, second],
            has_more: true,
        };

        assert_eq!(page.items.len(), 2);
        assert!(page.has_more);
    }
}
