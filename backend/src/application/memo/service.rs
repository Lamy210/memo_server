use std::{cmp::Reverse, sync::Arc};

use uuid::Uuid;

use super::dto::{CreateMemoDto, MemoResponse, SearchResponse, UpdateMemoDto};
use crate::{
    application::{
        crypto_search_orchestration::HighSearchQueryReader,
        high_search_routing::HighSearchQueryRoute,
        high_search_shadow::{HighSearchShadowObservation, HighSearchShadowObserver},
        maintenance::{
            HighSearchQueryGuard, HighSearchQueryPermit, MemoMutationGuard, MemoMutationPermit,
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
    memo_repository: Arc<dyn MemoRepository>,
    mutation_guard: Arc<dyn MemoMutationGuard>,
    high_search_query_guard: Arc<dyn HighSearchQueryGuard>,
    high_search_query_reader: Option<Arc<dyn HighSearchQueryReader>>,
    high_search_shadow: Option<Arc<HighSearchShadowObserver>>,
}

impl MemoService {
    pub fn new(
        memo_repository: Arc<dyn MemoRepository>,
        mutation_guard: Arc<dyn MemoMutationGuard>,
        high_search_query_guard: Arc<dyn HighSearchQueryGuard>,
        high_search_query_reader: Option<Arc<dyn HighSearchQueryReader>>,
        high_search_shadow: Option<Arc<HighSearchShadowObserver>>,
    ) -> Self {
        Self {
            memo_repository,
            mutation_guard,
            high_search_query_guard,
            high_search_query_reader,
            high_search_shadow,
        }
    }

    pub async fn create_memo(&self, dto: CreateMemoDto, user_id: Uuid) -> AppResult<MemoResponse> {
        let memo = Memo::new(dto.title, dto.content, dto.tags, user_id);
        Self::validate_memo(&memo)?;
        let permit = self.mutation_guard.acquire_mutation().await?;
        let result = self
            .memo_repository
            .save(&memo)
            .await
            .map(|()| MemoResponse::from(memo));
        Self::finish_mutation(result, permit).await
    }

    pub async fn update_memo(
        &self,
        id: Uuid,
        dto: UpdateMemoDto,
        user_id: Uuid,
    ) -> AppResult<MemoResponse> {
        let mut memo = self
            .memo_repository
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
        let permit = self.mutation_guard.acquire_mutation().await?;
        let result = self
            .memo_repository
            .save(&memo)
            .await
            .map(|()| MemoResponse::from(memo));
        Self::finish_mutation(result, permit).await
    }

    pub async fn get_memo(&self, id: Uuid, user_id: Uuid) -> AppResult<MemoResponse> {
        let memo = self
            .memo_repository
            .find_by_id(user_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("Memo not found".into()))?;
        Ok(MemoResponse::from(memo))
    }

    pub async fn delete_memo(&self, id: Uuid, user_id: Uuid) -> AppResult<()> {
        if !self.memo_repository.exists(user_id, id).await? {
            return Err(AppError::NotFound("Memo not found".into()));
        }

        let permit = self.mutation_guard.acquire_mutation().await?;
        let result = self.memo_repository.delete(user_id, id).await;
        Self::finish_mutation(result, permit).await
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
        let mut memos = self.memo_repository.find_all_by_user_id(user_id).await?;
        memos.sort_by_key(|memo| Reverse(memo.updated_at));
        Ok(memos.into_iter().map(MemoResponse::from).collect())
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
        let (search_page, route) = self
            .search_memos_routed(query, tag.as_deref(), user_id, page, limit)
            .await?;

        if route == HighSearchQueryRoute::Legacy {
            if let Some(observer) = self.high_search_shadow.as_ref() {
                observer.observe(HighSearchShadowObservation {
                    query,
                    tag: tag.as_deref(),
                    owner_partition: user_id,
                    page,
                    limit,
                    legacy_memo_ids: search_page.items.iter().map(|memo| memo.id),
                    legacy_total: search_page.total,
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
    ) -> AppResult<(MemoSearchPage, HighSearchQueryRoute)> {
        let permit = self.high_search_query_guard.acquire_query().await?;
        let route = permit.route_snapshot().route;

        let result = match route {
            HighSearchQueryRoute::Legacy => {
                self.memo_repository
                    .search(query, tag.map(str::to_owned), user_id, page, limit)
                    .await
            }
            HighSearchQueryRoute::Protected => {
                let reader = self.high_search_query_reader.as_ref().ok_or_else(|| {
                    AppError::ServiceUnavailable(
                        "protected HIGH search route is active but no protected query reader is available"
                            .into(),
                    )
                });

                match reader {
                    Ok(reader) => {
                        let hits = reader
                            .search_memo_ids(user_id, query, tag, page, limit)
                            .await?;
                        let items = self
                            .memo_repository
                            .find_many_by_ids(user_id, &hits.memo_ids)
                            .await?;
                        Ok(MemoSearchPage {
                            items,
                            total: hits.total,
                        })
                    }
                    Err(error) => Err(error),
                }
            }
        };

        Self::finish_query(result, permit)
            .await
            .map(|page| (page, route))
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
