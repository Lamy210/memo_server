use std::{cmp::Reverse, sync::Arc};

use uuid::Uuid;

use super::dto::{CreateMemoDto, MemoResponse, SearchResponse, UpdateMemoDto};
use crate::{
    application::{
        crypto_search_orchestration::HighSearchQueryReader,
        high_memo_routing::HighMemoDataRoute,
        high_search_routing::{HighSearchQueryRoute, HighSearchQueryRouteSnapshot},
        high_search_shadow::{HighSearchShadowObservation, HighSearchShadowObserver},
        maintenance::{
            HighMemoAccessGuard, HighMemoAccessPermit, HighSearchQueryGuard, HighSearchQueryPermit,
            MemoMutationGuard, MemoMutationPermit,
        },
    },
    domain::memo::{
        entity::{Memo, MAX_MEMO_TAGS, MAX_MEMO_TAG_CHARS, MAX_MEMO_TITLE_CHARS},
        repository::{MemoRepository, MemoSearchPage},
    },
    error::{AppError, AppResult},
};

const MAX_SEARCH_QUERY_CHARS: usize = 512;

pub struct MemoService {
    legacy_memo_repository: Arc<dyn MemoRepository>,
    encrypted_memo_repository: Option<Arc<dyn MemoRepository>>,
    mutation_guard: Arc<dyn MemoMutationGuard>,
    high_memo_access_guard: Arc<dyn HighMemoAccessGuard>,
    high_search_query_guard: Arc<dyn HighSearchQueryGuard>,
    high_search_query_reader: Option<Arc<dyn HighSearchQueryReader>>,
    high_search_shadow: Option<Arc<HighSearchShadowObserver>>,
}

impl MemoService {
    pub fn new(
        legacy_memo_repository: Arc<dyn MemoRepository>,
        encrypted_memo_repository: Option<Arc<dyn MemoRepository>>,
        mutation_guard: Arc<dyn MemoMutationGuard>,
        high_memo_access_guard: Arc<dyn HighMemoAccessGuard>,
        high_search_query_guard: Arc<dyn HighSearchQueryGuard>,
        high_search_query_reader: Option<Arc<dyn HighSearchQueryReader>>,
        high_search_shadow: Option<Arc<HighSearchShadowObserver>>,
    ) -> Self {
        Self {
            legacy_memo_repository,
            encrypted_memo_repository,
            mutation_guard,
            high_memo_access_guard,
            high_search_query_guard,
            high_search_query_reader,
            high_search_shadow,
        }
    }

    fn repository_for_route(
        &self,
        route: HighMemoDataRoute,
    ) -> AppResult<&Arc<dyn MemoRepository>> {
        match route {
            HighMemoDataRoute::LegacyPlaintext => Ok(&self.legacy_memo_repository),
            HighMemoDataRoute::Encrypted => self.encrypted_memo_repository.as_ref().ok_or_else(|| {
                AppError::ServiceUnavailable(
                    "encrypted MEMO-HIGH-1 route is active but no encrypted repository is available"
                        .into(),
                )
            }),
        }
    }

    pub async fn create_memo(&self, dto: CreateMemoDto, user_id: Uuid) -> AppResult<MemoResponse> {
        let memo = Memo::new(dto.title, dto.content, dto.tags, user_id);
        Self::validate_memo(&memo)?;

        let access = self.high_memo_access_guard.acquire_access().await?;
        let route = access.route_snapshot();
        let result = async {
            let repository = self.repository_for_route(route.route)?;
            let mutation = self.mutation_guard.acquire_mutation().await?;
            let result = repository
                .save(&memo)
                .await
                .map(|()| MemoResponse::from(memo));
            Self::finish_mutation(result, mutation).await
        }
        .await;

        Self::finish_access(result, access).await
    }

    pub async fn update_memo(
        &self,
        id: Uuid,
        dto: UpdateMemoDto,
        user_id: Uuid,
    ) -> AppResult<MemoResponse> {
        let access = self.high_memo_access_guard.acquire_access().await?;
        let route = access.route_snapshot();
        let result = async {
            let repository = self.repository_for_route(route.route)?;
            let mut memo = repository
                .find_by_id(user_id, id)
                .await?
                .ok_or_else(|| AppError::NotFound("Memo not found".into()))?;

            if memo.user_id != user_id {
                return Err(AppError::Unauthorized(
                    "Not authorized to update this memo".into(),
                ));
            }
            if memo.version != dto.version {
                return Err(AppError::Conflict(
                    "Memo has been updated by another client".into(),
                ));
            }

            memo.update(dto.title, dto.content, dto.tags);
            Self::validate_memo(&memo)?;

            let mutation = self.mutation_guard.acquire_mutation().await?;
            let result = repository
                .save(&memo)
                .await
                .map(|()| MemoResponse::from(memo));
            Self::finish_mutation(result, mutation).await
        }
        .await;

        Self::finish_access(result, access).await
    }

    pub async fn get_memo(&self, id: Uuid, user_id: Uuid) -> AppResult<MemoResponse> {
        let access = self.high_memo_access_guard.acquire_access().await?;
        let route = access.route_snapshot();
        let result = async {
            let repository = self.repository_for_route(route.route)?;
            let memo = repository
                .find_by_id(user_id, id)
                .await?
                .ok_or_else(|| AppError::NotFound("Memo not found".into()))?;
            Ok(MemoResponse::from(memo))
        }
        .await;

        Self::finish_access(result, access).await
    }

    pub async fn delete_memo(&self, id: Uuid, user_id: Uuid) -> AppResult<()> {
        let access = self.high_memo_access_guard.acquire_access().await?;
        let route = access.route_snapshot();
        let result = async {
            let repository = self.repository_for_route(route.route)?;
            if !repository.exists(user_id, id).await? {
                return Err(AppError::NotFound("Memo not found".into()));
            }

            let mutation = self.mutation_guard.acquire_mutation().await?;
            let result = repository.delete(user_id, id).await;
            Self::finish_mutation(result, mutation).await
        }
        .await;

        Self::finish_access(result, access).await
    }

    async fn finish_access<T>(
        result: AppResult<T>,
        permit: Box<dyn HighMemoAccessPermit>,
    ) -> AppResult<T> {
        match (result, permit.release().await) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(primary), Ok(())) => Err(primary),
            (Ok(_), Err(release)) => Err(release),
            (Err(primary), Err(release)) => Err(AppError::ServiceUnavailable(format!(
                "memo data-path request failed and memo-access lease release also failed; primary={primary}; release={release}"
            ))),
        }
    }

    async fn finish_mutation<T>(
        result: AppResult<T>,
        permit: Box<dyn MemoMutationPermit>,
    ) -> AppResult<T> {
        match (result, permit.release().await) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(primary), Ok(())) => Err(primary),
            (Ok(_), Err(release)) => Err(release),
            (Err(primary), Err(release)) => Err(AppError::ServiceUnavailable(format!(
                "memo mutation failed and maintenance writer lease release also failed; primary={primary}; release={release}"
            ))),
        }
    }

    pub async fn get_user_memos(&self, user_id: Uuid) -> AppResult<Vec<MemoResponse>> {
        let access = self.high_memo_access_guard.acquire_access().await?;
        let route = access.route_snapshot();
        let result = async {
            let repository = self.repository_for_route(route.route)?;
            let mut memos = repository.find_all_by_user_id(user_id).await?;
            memos.sort_by_key(|memo| Reverse(memo.updated_at));
            Ok(memos.into_iter().map(MemoResponse::from).collect())
        }
        .await;

        Self::finish_access(result, access).await
    }

    pub async fn search_memos(
        &self,
        query: &str,
        tag: Option<String>,
        user_id: Uuid,
        page: usize,
        limit: usize,
    ) -> AppResult<SearchResponse> {
        if query.chars().count() > MAX_SEARCH_QUERY_CHARS {
            return Err(AppError::ValidationError(format!(
                "Search query must not exceed {MAX_SEARCH_QUERY_CHARS} characters"
            )));
        }
        if tag
            .as_ref()
            .is_some_and(|tag| tag.chars().count() > MAX_MEMO_TAG_CHARS)
        {
            return Err(AppError::ValidationError(format!(
                "Search tag must not exceed {MAX_MEMO_TAG_CHARS} characters"
            )));
        }

        let page = page.max(1);
        let limit = limit.clamp(1, 100);
        let (search_page, route_snapshot) = self
            .search_memos_routed(query, tag.as_deref(), user_id, page, limit)
            .await?;

        if route_snapshot.route == HighSearchQueryRoute::Legacy {
            if let Some(observer) = self.high_search_shadow.as_ref() {
                observer.observe(HighSearchShadowObservation {
                    query,
                    tag: tag.as_deref(),
                    owner_partition: user_id,
                    page,
                    limit,
                    legacy_memo_ids: search_page.items.iter().map(|memo| memo.id),
                    legacy_total: search_page.total,
                    legacy_route_generation: route_snapshot.generation,
                });
            }
        }

        let total_pages = search_page.total.div_ceil(limit);
        let items = search_page
            .items
            .into_iter()
            .map(MemoResponse::from)
            .collect();

        Ok(SearchResponse {
            items,
            total: search_page.total,
            page,
            total_pages,
        })
    }

    async fn search_memos_routed(
        &self,
        query: &str,
        tag: Option<&str>,
        user_id: Uuid,
        page: usize,
        limit: usize,
    ) -> AppResult<(MemoSearchPage, HighSearchQueryRouteSnapshot)> {
        let access = self.high_memo_access_guard.acquire_access().await?;
        let memo_route = access.route_snapshot();
        let result = async {
            let repository = self.repository_for_route(memo_route.route)?;
            let query_permit = self.high_search_query_guard.acquire_query().await?;
            let search_route = query_permit.route_snapshot();

            let result = match search_route.route {
                HighSearchQueryRoute::Legacy => {
                    repository
                        .search(query, tag.map(str::to_owned), user_id, page, limit)
                        .await
                }
                HighSearchQueryRoute::Protected => {
                    self.search_memos_protected(
                        repository,
                        query,
                        tag,
                        user_id,
                        page,
                        limit,
                    )
                    .await
                }
            };

            Self::finish_query(result, query_permit)
                .await
                .map(|page| (page, search_route))
        }
        .await;

        Self::finish_access(result, access).await
    }

    async fn search_memos_protected(
        &self,
        repository: &Arc<dyn MemoRepository>,
        query: &str,
        tag: Option<&str>,
        user_id: Uuid,
        page: usize,
        limit: usize,
    ) -> AppResult<MemoSearchPage> {
        let reader = self.high_search_query_reader.as_ref().ok_or_else(|| {
            AppError::ServiceUnavailable(
                "protected HIGH search route is active but no protected query reader is available"
                    .into(),
            )
        })?;
        let hits = reader
            .search_memo_ids(user_id, query, tag, page, limit)
            .await?;
        let items = repository
            .find_many_by_ids(user_id, &hits.memo_ids)
            .await?;

        Ok(MemoSearchPage {
            items,
            total: hits.total,
        })
    }

    async fn finish_query<T>(
        result: AppResult<T>,
        permit: Box<dyn HighSearchQueryPermit>,
    ) -> AppResult<T> {
        match (result, permit.release().await) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(primary), Ok(())) => Err(primary),
            (Ok(_), Err(release)) => Err(release),
            (Err(primary), Err(release)) => Err(AppError::ServiceUnavailable(format!(
                "memo search failed and HIGH search query lease release also failed; primary={primary}; release={release}"
            ))),
        }
    }

    fn validate_memo(memo: &Memo) -> AppResult<()> {
        if memo.validate() {
            Ok(())
        } else {
            Err(AppError::ValidationError(
                format!(
                    "Title and content are required; title must not exceed {MAX_MEMO_TITLE_CHARS} characters; tags must be non-empty, limited to {MAX_MEMO_TAGS}, and each tag must not exceed {MAX_MEMO_TAG_CHARS} characters"
                ),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    };

    use async_trait::async_trait;

    use super::*;
    use crate::application::{
        crypto_search_projection::HighSearchProjectionPage,
        maintenance::{UnrestrictedHighMemoAccessGuard, UnrestrictedMemoMutationGuard},
    };

    struct FakeRepository {
        legacy_search_calls: AtomicUsize,
        legacy_items: Vec<Memo>,
        legacy_total: usize,
        hydrated_ids: Mutex<Vec<Uuid>>,
        hydrated_items: Vec<Memo>,
    }

    #[async_trait]
    impl MemoRepository for FakeRepository {
        async fn find_by_id(&self, _user_id: Uuid, _id: Uuid) -> AppResult<Option<Memo>> {
            Ok(None)
        }

        async fn find_all_by_user_id(&self, _user_id: Uuid) -> AppResult<Vec<Memo>> {
            Ok(Vec::new())
        }

        async fn find_many_by_ids(&self, _user_id: Uuid, ids: &[Uuid]) -> AppResult<Vec<Memo>> {
            *self.hydrated_ids.lock().unwrap() = ids.to_vec();
            Ok(self.hydrated_items.clone())
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
            self.legacy_search_calls.fetch_add(1, Ordering::Relaxed);
            Ok(MemoSearchPage {
                items: self.legacy_items.clone(),
                total: self.legacy_total,
            })
        }

        async fn exists(&self, _user_id: Uuid, _id: Uuid) -> AppResult<bool> {
            Ok(false)
        }
    }

    struct FakeQueryReader {
        calls: AtomicUsize,
        owner: Mutex<Option<Uuid>>,
        memo_ids: Vec<Uuid>,
        total: usize,
        fail: bool,
    }

    #[async_trait]
    impl HighSearchQueryReader for FakeQueryReader {
        async fn search_memo_ids(
            &self,
            owner_partition: Uuid,
            _query: &str,
            _tag: Option<&str>,
            _page: usize,
            _limit: usize,
        ) -> AppResult<HighSearchProjectionPage> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            *self.owner.lock().unwrap() = Some(owner_partition);
            if self.fail {
                Err(AppError::ServiceUnavailable(
                    "protected reader failed".into(),
                ))
            } else {
                Ok(HighSearchProjectionPage {
                    memo_ids: self.memo_ids.clone(),
                    total: self.total,
                })
            }
        }
    }

    struct FakeMemoAccessGuard {
        route: HighMemoDataRoute,
        release_fail: bool,
        events: Arc<Mutex<Vec<&'static str>>>,
    }

    struct FakeMemoAccessPermit {
        route: HighMemoDataRoute,
        release_fail: bool,
        events: Arc<Mutex<Vec<&'static str>>>,
    }

    #[async_trait]
    impl HighMemoAccessGuard for FakeMemoAccessGuard {
        async fn acquire_access(&self) -> AppResult<Box<dyn HighMemoAccessPermit>> {
            self.events.lock().unwrap().push("memo-acquire");
            Ok(Box::new(FakeMemoAccessPermit {
                route: self.route,
                release_fail: self.release_fail,
                events: self.events.clone(),
            }))
        }
    }

    #[async_trait]
    impl HighMemoAccessPermit for FakeMemoAccessPermit {
        fn route_snapshot(&self) -> crate::application::high_memo_routing::HighMemoDataRouteSnapshot {
            crate::application::high_memo_routing::HighMemoDataRouteSnapshot {
                route: self.route,
                generation: 3,
            }
        }

        async fn release(self: Box<Self>) -> AppResult<()> {
            self.events.lock().unwrap().push("memo-release");
            if self.release_fail {
                Err(AppError::ServiceUnavailable(
                    "memo access lease release failed".into(),
                ))
            } else {
                Ok(())
            }
        }
    }

    struct FakeQueryGuard {
        route: HighSearchQueryRoute,
        release_fail: bool,
        events: Arc<Mutex<Vec<&'static str>>>,
    }

    struct FakeQueryPermit {
        route: HighSearchQueryRoute,
        release_fail: bool,
        events: Arc<Mutex<Vec<&'static str>>>,
    }

    #[async_trait]
    impl HighSearchQueryGuard for FakeQueryGuard {
        async fn acquire_query(&self) -> AppResult<Box<dyn HighSearchQueryPermit>> {
            self.events.lock().unwrap().push("acquire");
            Ok(Box::new(FakeQueryPermit {
                route: self.route,
                release_fail: self.release_fail,
                events: self.events.clone(),
            }))
        }
    }

    #[async_trait]
    impl HighSearchQueryPermit for FakeQueryPermit {
        fn route_snapshot(&self) -> HighSearchQueryRouteSnapshot {
            HighSearchQueryRouteSnapshot {
                route: self.route,
                generation: 7,
            }
        }

        async fn release(self: Box<Self>) -> AppResult<()> {
            self.events.lock().unwrap().push("release");
            if self.release_fail {
                Err(AppError::ServiceUnavailable(
                    "query lease release failed".into(),
                ))
            } else {
                Ok(())
            }
        }
    }

    fn memo(user_id: Uuid, id: Uuid, title: &str) -> Memo {
        let mut memo = Memo::new(title.into(), "content".into(), Vec::new(), user_id);
        memo.id = id;
        memo
    }

    fn service(
        repository: Arc<FakeRepository>,
        route: HighSearchQueryRoute,
        reader: Option<Arc<FakeQueryReader>>,
        release_fail: bool,
        events: Arc<Mutex<Vec<&'static str>>>,
    ) -> MemoService {
        MemoService::new(
            repository,
            None,
            Arc::new(UnrestrictedMemoMutationGuard),
            Arc::new(UnrestrictedHighMemoAccessGuard),
            Arc::new(FakeQueryGuard {
                route,
                release_fail,
                events,
            }),
            reader.map(|reader| -> Arc<dyn HighSearchQueryReader> { reader }),
            None,
        )
    }

    #[tokio::test]
    async fn encrypted_memo_route_without_repository_fails_closed_and_releases_access() {
        let repository = Arc::new(FakeRepository {
            legacy_search_calls: AtomicUsize::new(0),
            legacy_items: Vec::new(),
            legacy_total: 0,
            hydrated_ids: Mutex::new(Vec::new()),
            hydrated_items: Vec::new(),
        });
        let memo_events = Arc::new(Mutex::new(Vec::new()));
        let query_events = Arc::new(Mutex::new(Vec::new()));
        let service = MemoService::new(
            repository,
            None,
            Arc::new(UnrestrictedMemoMutationGuard),
            Arc::new(FakeMemoAccessGuard {
                route: HighMemoDataRoute::Encrypted,
                release_fail: false,
                events: memo_events.clone(),
            }),
            Arc::new(FakeQueryGuard {
                route: HighSearchQueryRoute::Legacy,
                release_fail: false,
                events: query_events,
            }),
            None,
            None,
        );

        assert!(matches!(
            service.get_user_memos(Uuid::new_v4()).await,
            Err(AppError::ServiceUnavailable(_))
        ));
        assert_eq!(
            *memo_events.lock().unwrap(),
            vec!["memo-acquire", "memo-release"]
        );
    }

    #[tokio::test]
    async fn memo_access_release_failure_fails_closed() {
        let repository = Arc::new(FakeRepository {
            legacy_search_calls: AtomicUsize::new(0),
            legacy_items: Vec::new(),
            legacy_total: 0,
            hydrated_ids: Mutex::new(Vec::new()),
            hydrated_items: Vec::new(),
        });
        let memo_events = Arc::new(Mutex::new(Vec::new()));
        let service = MemoService::new(
            repository,
            None,
            Arc::new(UnrestrictedMemoMutationGuard),
            Arc::new(FakeMemoAccessGuard {
                route: HighMemoDataRoute::LegacyPlaintext,
                release_fail: true,
                events: memo_events.clone(),
            }),
            Arc::new(FakeQueryGuard {
                route: HighSearchQueryRoute::Legacy,
                release_fail: false,
                events: Arc::new(Mutex::new(Vec::new())),
            }),
            None,
            None,
        );

        assert!(matches!(
            service.get_user_memos(Uuid::new_v4()).await,
            Err(AppError::ServiceUnavailable(_))
        ));
        assert_eq!(
            *memo_events.lock().unwrap(),
            vec!["memo-acquire", "memo-release"]
        );
    }

    #[tokio::test]
    async fn legacy_route_uses_legacy_search_and_releases_query_lease() {
        let user_id = Uuid::new_v4();
        let legacy = memo(user_id, Uuid::new_v4(), "legacy");
        let repository = Arc::new(FakeRepository {
            legacy_search_calls: AtomicUsize::new(0),
            legacy_items: vec![legacy.clone()],
            legacy_total: 1,
            hydrated_ids: Mutex::new(Vec::new()),
            hydrated_items: Vec::new(),
        });
        let reader = Arc::new(FakeQueryReader {
            calls: AtomicUsize::new(0),
            owner: Mutex::new(None),
            memo_ids: Vec::new(),
            total: 0,
            fail: false,
        });
        let events = Arc::new(Mutex::new(Vec::new()));
        let service = service(
            repository.clone(),
            HighSearchQueryRoute::Legacy,
            Some(reader.clone()),
            false,
            events.clone(),
        );

        let response = service
            .search_memos("legacy", None, user_id, 1, 20)
            .await
            .unwrap();

        assert_eq!(response.items.len(), 1);
        assert_eq!(response.items[0].id, legacy.id);
        assert_eq!(repository.legacy_search_calls.load(Ordering::Relaxed), 1);
        assert!(repository.hydrated_ids.lock().unwrap().is_empty());
        assert_eq!(reader.calls.load(Ordering::Relaxed), 0);
        assert_eq!(*events.lock().unwrap(), vec!["acquire", "release"]);
    }

    #[tokio::test]
    async fn protected_route_uses_reader_then_owner_scoped_hydration() {
        let user_id = Uuid::new_v4();
        let protected = memo(user_id, Uuid::new_v4(), "protected");
        let repository = Arc::new(FakeRepository {
            legacy_search_calls: AtomicUsize::new(0),
            legacy_items: Vec::new(),
            legacy_total: 0,
            hydrated_ids: Mutex::new(Vec::new()),
            hydrated_items: vec![protected.clone()],
        });
        let reader = Arc::new(FakeQueryReader {
            calls: AtomicUsize::new(0),
            owner: Mutex::new(None),
            memo_ids: vec![protected.id],
            total: 1,
            fail: false,
        });
        let events = Arc::new(Mutex::new(Vec::new()));
        let service = service(
            repository.clone(),
            HighSearchQueryRoute::Protected,
            Some(reader.clone()),
            false,
            events.clone(),
        );

        let response = service
            .search_memos("protected", Some("tag".into()), user_id, 2, 10)
            .await
            .unwrap();

        assert_eq!(response.items.len(), 1);
        assert_eq!(response.items[0].id, protected.id);
        assert_eq!(repository.legacy_search_calls.load(Ordering::Relaxed), 0);
        assert_eq!(*repository.hydrated_ids.lock().unwrap(), vec![protected.id]);
        assert_eq!(*reader.owner.lock().unwrap(), Some(user_id));
        assert_eq!(reader.calls.load(Ordering::Relaxed), 1);
        assert_eq!(*events.lock().unwrap(), vec!["acquire", "release"]);
    }

    #[tokio::test]
    async fn protected_reader_failure_still_releases_query_lease() {
        let user_id = Uuid::new_v4();
        let repository = Arc::new(FakeRepository {
            legacy_search_calls: AtomicUsize::new(0),
            legacy_items: Vec::new(),
            legacy_total: 0,
            hydrated_ids: Mutex::new(Vec::new()),
            hydrated_items: Vec::new(),
        });
        let reader = Arc::new(FakeQueryReader {
            calls: AtomicUsize::new(0),
            owner: Mutex::new(None),
            memo_ids: Vec::new(),
            total: 0,
            fail: true,
        });
        let events = Arc::new(Mutex::new(Vec::new()));
        let service = service(
            repository,
            HighSearchQueryRoute::Protected,
            Some(reader),
            false,
            events.clone(),
        );

        assert!(matches!(
            service
                .search_memos("protected", None, user_id, 1, 20)
                .await,
            Err(AppError::ServiceUnavailable(_))
        ));
        assert_eq!(*events.lock().unwrap(), vec!["acquire", "release"]);
    }

    #[tokio::test]
    async fn missing_protected_reader_still_releases_query_lease() {
        let user_id = Uuid::new_v4();
        let repository = Arc::new(FakeRepository {
            legacy_search_calls: AtomicUsize::new(0),
            legacy_items: Vec::new(),
            legacy_total: 0,
            hydrated_ids: Mutex::new(Vec::new()),
            hydrated_items: Vec::new(),
        });
        let events = Arc::new(Mutex::new(Vec::new()));
        let service = service(
            repository,
            HighSearchQueryRoute::Protected,
            None,
            false,
            events.clone(),
        );

        assert!(matches!(
            service
                .search_memos("protected", None, user_id, 1, 20)
                .await,
            Err(AppError::ServiceUnavailable(_))
        ));
        assert_eq!(*events.lock().unwrap(), vec!["acquire", "release"]);
    }

    #[tokio::test]
    async fn query_lease_release_failure_fails_closed() {
        let user_id = Uuid::new_v4();
        let repository = Arc::new(FakeRepository {
            legacy_search_calls: AtomicUsize::new(0),
            legacy_items: Vec::new(),
            legacy_total: 0,
            hydrated_ids: Mutex::new(Vec::new()),
            hydrated_items: Vec::new(),
        });
        let events = Arc::new(Mutex::new(Vec::new()));
        let service = service(
            repository,
            HighSearchQueryRoute::Legacy,
            None,
            true,
            events.clone(),
        );

        assert!(matches!(
            service.search_memos("", None, user_id, 1, 20).await,
            Err(AppError::ServiceUnavailable(_))
        ));
        assert_eq!(*events.lock().unwrap(), vec!["acquire", "release"]);
    }
}
