use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

use async_trait::async_trait;
use memo_app_backend::{
    application::maintenance::UnrestrictedMemoMutationGuard,
    domain::memo::{
        entity::Memo,
        repository::{MemoListPage, MemoRepository},
    },
    error::AppResult,
    infrastructure::{
        persistence::ports::{
            MemoAuthoritativeStore, MemoCache, MemoSearchHitPage, MemoSearchProjection,
            ProjectionIntent, ProjectionTarget,
        },
        reconciliation::ProjectionReconciler,
        repositories::memo::MemoRepositoryImpl,
    },
};
use uuid::Uuid;

struct RecordingStore {
    page_calls: Mutex<Vec<(Uuid, Option<Uuid>, usize)>>,
    unbounded_calls: AtomicUsize,
    items: Vec<Memo>,
    has_more: bool,
}

#[async_trait]
impl MemoAuthoritativeStore for RecordingStore {
    async fn find_by_id(&self, _user_id: Uuid, _id: Uuid) -> AppResult<Option<Memo>> {
        Ok(None)
    }

    async fn find_all_by_user_id(&self, _user_id: Uuid) -> AppResult<Vec<Memo>> {
        self.unbounded_calls.fetch_add(1, Ordering::Relaxed);
        panic!("bounded repository paging must not call find_all_by_user_id")
    }

    async fn list_page_by_user_id(
        &self,
        user_id: Uuid,
        after: Option<Uuid>,
        limit: usize,
    ) -> AppResult<MemoListPage> {
        self.page_calls.lock().unwrap().push((user_id, after, limit));
        Ok(MemoListPage {
            items: self.items.clone(),
            has_more: self.has_more,
        })
    }

    async fn find_many_by_ids(&self, _user_id: Uuid, _ids: &[Uuid]) -> AppResult<Vec<Memo>> {
        Ok(Vec::new())
    }

    async fn save_with_projection_intent(&self, memo: &Memo) -> AppResult<ProjectionIntent> {
        Ok(ProjectionIntent::new(
            memo.user_id,
            memo.id,
            ProjectionTarget::Version(memo.version),
        ))
    }

    async fn delete_with_projection_intent(
        &self,
        user_id: Uuid,
        id: Uuid,
    ) -> AppResult<ProjectionIntent> {
        Ok(ProjectionIntent::new(user_id, id, ProjectionTarget::Deleted))
    }

    async fn exists(&self, _user_id: Uuid, _id: Uuid) -> AppResult<bool> {
        Ok(false)
    }

    async fn enqueue_projection_intent(
        &self,
        user_id: Uuid,
        memo_id: Uuid,
        target: ProjectionTarget,
    ) -> AppResult<ProjectionIntent> {
        Ok(ProjectionIntent::new(user_id, memo_id, target))
    }

    async fn list_projection_intents(&self) -> AppResult<Vec<ProjectionIntent>> {
        Ok(Vec::new())
    }

    async fn acknowledge_projection_intent(&self, _event: &ProjectionIntent) -> AppResult<()> {
        Ok(())
    }
}

struct EmptyCache;

#[async_trait]
impl MemoCache for EmptyCache {
    async fn get_memo(&self, _owner_partition: Uuid, _memo_id: Uuid) -> AppResult<Option<Memo>> {
        Ok(None)
    }

    async fn set_memo(
        &self,
        _memo: &Memo,
        _expiration: Option<std::time::Duration>,
    ) -> AppResult<()> {
        Ok(())
    }

    async fn delete_memo(&self, _owner_partition: Uuid, _memo_id: Uuid) -> AppResult<()> {
        Ok(())
    }

    async fn memo_exists(&self, _owner_partition: Uuid, _memo_id: Uuid) -> AppResult<bool> {
        Ok(false)
    }
}

struct EmptySearch;

#[async_trait]
impl MemoSearchProjection for EmptySearch {
    async fn index_memo(&self, _memo: &Memo) -> AppResult<()> {
        Ok(())
    }

    async fn search_memo_ids(
        &self,
        _query: &str,
        _tag: Option<String>,
        _user_id: Uuid,
        _page: usize,
        _limit: usize,
    ) -> AppResult<MemoSearchHitPage> {
        Ok(MemoSearchHitPage {
            memo_ids: Vec::new(),
            total: 0,
        })
    }

    async fn delete_memo(&self, _id: Uuid) -> AppResult<()> {
        Ok(())
    }
}

#[tokio::test]
async fn repository_delegates_list_page_without_unbounded_owner_read() {
    let owner = Uuid::new_v4();
    let after = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440010").unwrap();
    let mut item = Memo::new("memo".into(), "content".into(), Vec::new(), owner);
    item.id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440009").unwrap();

    let store = Arc::new(RecordingStore {
        page_calls: Mutex::new(Vec::new()),
        unbounded_calls: AtomicUsize::new(0),
        items: vec![item.clone()],
        has_more: true,
    });
    let cache = Arc::new(EmptyCache);
    let search = Arc::new(EmptySearch);
    let reconciler = Arc::new(ProjectionReconciler::new(
        store.clone(),
        cache.clone(),
        search.clone(),
        None,
        Arc::new(UnrestrictedMemoMutationGuard),
    ));
    let repository = MemoRepositoryImpl::new(store.clone(), cache, search, reconciler);

    let page = MemoRepository::list_page_by_user_id(&repository, owner, Some(after), 2)
        .await
        .unwrap();

    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].id, item.id);
    assert!(page.has_more);
    assert_eq!(&*store.page_calls.lock().unwrap(), &[(owner, Some(after), 2)]);
    assert_eq!(store.unbounded_calls.load(Ordering::Relaxed), 0);
}
