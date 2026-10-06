use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use memo_app_backend::{
    application::{
        maintenance::{
            UnrestrictedHighMemoAccessGuard, UnrestrictedHighSearchQueryGuard,
            UnrestrictedMemoMutationGuard,
        },
        memo::{pagination::format_cursor_v1, service::MemoService},
    },
    domain::memo::{
        entity::Memo,
        repository::{MemoListPage, MemoRepository, MemoSearchPage},
    },
    error::{AppError, AppResult},
};
use uuid::Uuid;

struct PagingRepository {
    calls: Mutex<Vec<(Uuid, Option<Uuid>, usize)>>,
    items: Vec<Memo>,
    has_more: bool,
}

#[async_trait]
impl MemoRepository for PagingRepository {
    async fn find_by_id(&self, _user_id: Uuid, _id: Uuid) -> AppResult<Option<Memo>> {
        Ok(None)
    }

    async fn find_all_by_user_id(&self, _user_id: Uuid) -> AppResult<Vec<Memo>> {
        panic!("paged list must not fall back to an unbounded owner read")
    }

    async fn list_page_by_user_id(
        &self,
        user_id: Uuid,
        after: Option<Uuid>,
        limit: usize,
    ) -> AppResult<MemoListPage> {
        self.calls.lock().unwrap().push((user_id, after, limit));
        Ok(MemoListPage {
            items: self.items.clone(),
            has_more: self.has_more,
        })
    }

    async fn find_many_by_ids(&self, _user_id: Uuid, _ids: &[Uuid]) -> AppResult<Vec<Memo>> {
        Ok(Vec::new())
    }

    async fn save(&self, _memo: &Memo) -> AppResult<()> {
        Ok(())
    }

    async fn delete(&self, _user_id: Uuid, _id: Uuid) -> AppResult<()> {
        Ok(())
    }

    async fn search(
        &self,
        _query: &str,
        _tag: Option<String>,
        _user_id: Uuid,
        _page: usize,
        _limit: usize,
    ) -> AppResult<MemoSearchPage> {
        Ok(MemoSearchPage {
            items: Vec::new(),
            total: 0,
        })
    }

    async fn exists(&self, _user_id: Uuid, _id: Uuid) -> AppResult<bool> {
        Ok(false)
    }
}

fn memo(user_id: Uuid, id: &str, title: &str) -> Memo {
    let mut memo = Memo::new(title.into(), "content".into(), Vec::new(), user_id);
    memo.id = Uuid::parse_str(id).unwrap();
    memo
}

fn service(repository: Arc<PagingRepository>) -> MemoService {
    MemoService::new(
        repository,
        None,
        Arc::new(UnrestrictedMemoMutationGuard),
        Arc::new(UnrestrictedHighMemoAccessGuard),
        Arc::new(UnrestrictedHighSearchQueryGuard),
        None,
        None,
    )
}

#[tokio::test]
async fn memo_list_page_defaults_to_first_page_and_builds_next_cursor() {
    let user_id = Uuid::new_v4();
    let first = memo(user_id, "550e8400-e29b-41d4-a716-446655440002", "first");
    let second = memo(user_id, "550e8400-e29b-41d4-a716-446655440001", "second");
    let repository = Arc::new(PagingRepository {
        calls: Mutex::new(Vec::new()),
        items: vec![first.clone(), second.clone()],
        has_more: true,
    });
    let service = service(repository.clone());

    let response = service
        .get_user_memos_page(user_id, None, None)
        .await
        .unwrap();

    assert_eq!(&*repository.calls.lock().unwrap(), &[(user_id, None, 20)]);
    assert_eq!(response.pagination, "cursor-v1");
    assert_eq!(response.limit, 20);
    assert_eq!(response.items.len(), 2);
    assert_eq!(response.items[0].id, first.id);
    assert_eq!(response.items[1].id, second.id);
    assert_eq!(response.next_cursor, Some(format_cursor_v1(second.id)));
}

#[tokio::test]
async fn memo_list_page_parses_explicit_cursor_and_limit() {
    let user_id = Uuid::new_v4();
    let after = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440010").unwrap();
    let repository = Arc::new(PagingRepository {
        calls: Mutex::new(Vec::new()),
        items: vec![memo(
            user_id,
            "550e8400-e29b-41d4-a716-446655440009",
            "next",
        )],
        has_more: false,
    });
    let service = service(repository.clone());
    let cursor = format_cursor_v1(after);

    let response = service
        .get_user_memos_page(user_id, Some(&cursor), Some(100))
        .await
        .unwrap();

    assert_eq!(
        &*repository.calls.lock().unwrap(),
        &[(user_id, Some(after), 100)]
    );
    assert_eq!(response.pagination, "cursor-v1");
    assert_eq!(response.limit, 100);
    assert_eq!(response.next_cursor, None);
}

#[tokio::test]
async fn memo_list_page_rejects_invalid_input_before_repository_access() {
    let user_id = Uuid::new_v4();
    let repository = Arc::new(PagingRepository {
        calls: Mutex::new(Vec::new()),
        items: Vec::new(),
        has_more: false,
    });
    let service = service(repository.clone());

    for cursor in [
        "v2.550e8400-e29b-41d4-a716-446655440000",
        "v1.550E8400-E29B-41D4-A716-446655440000",
        "v1.550e8400-e29b-11d4-a716-446655440000",
    ] {
        assert!(matches!(
            service
                .get_user_memos_page(user_id, Some(cursor), None)
                .await,
            Err(AppError::BadRequest(_))
        ));
    }

    for limit in [0, 101] {
        assert!(matches!(
            service
                .get_user_memos_page(user_id, None, Some(limit))
                .await,
            Err(AppError::BadRequest(_))
        ));
    }

    assert!(repository.calls.lock().unwrap().is_empty());
}
