use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use memo_app_backend::{
    application::{
        maintenance::{
            UnrestrictedHighMemoAccessGuard, UnrestrictedHighSearchQueryGuard,
            UnrestrictedMemoMutationGuard,
        },
        memo::service::MemoService,
    },
    domain::memo::{
        entity::Memo,
        repository::{MemoListPage, MemoRepository, MemoSearchPage},
    },
    error::{AppError, AppResult},
};
use uuid::Uuid;

struct PagingRepository {
    items: Vec<Memo>,
    has_more: bool,
    page_calls: AtomicUsize,
    page_limits: Mutex<Vec<usize>>,
    page_after: Mutex<Vec<Option<Uuid>>>,
    unbounded_reads: AtomicUsize,
}

impl PagingRepository {
    fn new(items: Vec<Memo>, has_more: bool) -> Self {
        Self {
            items,
            has_more,
            page_calls: AtomicUsize::new(0),
            page_limits: Mutex::new(Vec::new()),
            page_after: Mutex::new(Vec::new()),
            unbounded_reads: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl MemoRepository for PagingRepository {
    async fn find_by_id(&self, _user_id: Uuid, _id: Uuid) -> AppResult<Option<Memo>> {
        Ok(None)
    }

    async fn find_all_by_user_id(&self, _user_id: Uuid) -> AppResult<Vec<Memo>> {
        self.unbounded_reads.fetch_add(1, Ordering::SeqCst);
        Err(AppError::ServiceUnavailable(
            "unbounded memo list read invoked".into(),
        ))
    }

    async fn list_page_by_user_id(
        &self,
        _user_id: Uuid,
        after: Option<Uuid>,
        limit: usize,
    ) -> AppResult<MemoListPage> {
        self.page_calls.fetch_add(1, Ordering::SeqCst);
        self.page_limits.lock().unwrap().push(limit);
        self.page_after.lock().unwrap().push(after);
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

fn memo(owner: Uuid, id: &str, updated_at_ms: i64) -> Memo {
    let timestamp = Utc.timestamp_millis_opt(updated_at_ms).unwrap();
    Memo {
        id: Uuid::parse_str(id).unwrap(),
        title: id.into(),
        content: "content".into(),
        tags: Vec::new(),
        user_id: owner,
        created_at: timestamp,
        updated_at: timestamp,
        version: 1,
    }
}

#[tokio::test]
async fn legacy_list_is_bounded_to_one_hundred_and_preserves_recency_order() {
    let owner = Uuid::new_v4();
    let older = memo(
        owner,
        "550e8400-e29b-41d4-a716-446655440001",
        1_700_000_001_000,
    );
    let newer = memo(
        owner,
        "550e8400-e29b-41d4-a716-446655440002",
        1_700_000_010_000,
    );
    let repository = Arc::new(PagingRepository::new(
        vec![older.clone(), newer.clone()],
        false,
    ));
    let service = service(repository.clone());

    let response = service.get_user_memos(owner).await.unwrap();

    assert_eq!(
        response.iter().map(|memo| memo.id).collect::<Vec<_>>(),
        vec![newer.id, older.id]
    );
    assert_eq!(repository.page_calls.load(Ordering::SeqCst), 1);
    assert_eq!(*repository.page_limits.lock().unwrap(), vec![100]);
    assert_eq!(*repository.page_after.lock().unwrap(), vec![None]);
    assert_eq!(repository.unbounded_reads.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn legacy_list_requires_cursor_pagination_when_more_than_one_hundred_exist() {
    let owner = Uuid::new_v4();
    let repository = Arc::new(PagingRepository::new(
        vec![memo(
            owner,
            "550e8400-e29b-41d4-a716-446655440001",
            1_700_000_001_000,
        )],
        true,
    ));
    let service = service(repository.clone());

    let error = service.get_user_memos(owner).await.unwrap_err();

    assert!(matches!(
        error,
        AppError::BadRequest(ref message)
            if message == "Memo list pagination is required; use pagination=cursor-v1"
    ));
    assert_eq!(repository.page_calls.load(Ordering::SeqCst), 1);
    assert_eq!(*repository.page_limits.lock().unwrap(), vec![100]);
    assert_eq!(repository.unbounded_reads.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn cursor_v1_page_returns_exclusive_next_cursor_from_last_visible_item() {
    let owner = Uuid::new_v4();
    let first = memo(
        owner,
        "550e8400-e29b-41d4-a716-446655440004",
        1_700_000_004_000,
    );
    let second = memo(
        owner,
        "550e8400-e29b-41d4-a716-446655440003",
        1_700_000_003_000,
    );
    let repository = Arc::new(PagingRepository::new(
        vec![first.clone(), second.clone()],
        true,
    ));
    let service = service(repository.clone());

    let response = service
        .get_user_memos_page(owner, None, Some(2))
        .await
        .unwrap();

    assert_eq!(response.pagination, "cursor-v1");
    assert_eq!(response.limit, 2);
    assert_eq!(
        response
            .items
            .iter()
            .map(|memo| memo.id)
            .collect::<Vec<_>>(),
        vec![first.id, second.id]
    );
    assert_eq!(
        response.next_cursor.as_deref(),
        Some("v1.550e8400-e29b-41d4-a716-446655440003")
    );
    assert_eq!(repository.page_calls.load(Ordering::SeqCst), 1);
    assert_eq!(*repository.page_limits.lock().unwrap(), vec![2]);
    assert_eq!(*repository.page_after.lock().unwrap(), vec![None]);
}
