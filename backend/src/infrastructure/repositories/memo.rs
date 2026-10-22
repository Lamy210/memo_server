use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use uuid::Uuid;

use crate::{
    domain::memo::{
        entity::Memo,
        repository::{MemoListPage, MemoRepository},
    },
    error::AppResult,
    infrastructure::{
        persistence::ports::{MemoAuthoritativeStore, MemoCache, MemoSearchProjection},
        reconciliation::ProjectionReconciler,
    },
};

const CACHE_TTL: Duration = Duration::from_secs(3600);

pub struct MemoRepositoryImpl {
    authoritative_store: Arc<dyn MemoAuthoritativeStore>,
    cache: Arc<dyn MemoCache>,
    search_projection: Arc<dyn MemoSearchProjection>,
    reconciler: Arc<ProjectionReconciler>,
}

impl MemoRepositoryImpl {
    pub fn new(
        authoritative_store: Arc<dyn MemoAuthoritativeStore>,
        cache: Arc<dyn MemoCache>,
        search_projection: Arc<dyn MemoSearchProjection>,
        reconciler: Arc<ProjectionReconciler>,
    ) -> Self {
        Self {
            authoritative_store,
            cache,
            search_projection,
            reconciler,
        }
    }
}

#[async_trait]
impl MemoRepository for MemoRepositoryImpl {
    async fn find_by_id(&self, user_id: Uuid, id: Uuid) -> AppResult<Option<Memo>> {
        match self.cache.get_memo(user_id, id).await {
            Ok(Some(memo)) => return Ok(Some(memo)),
            Ok(None) => {}
            Err(error) => {
                log::warn!(
                    "Cache lookup failed; falling back to authoritative store: memo_id={id} user_id={user_id} error={error}"
                );
            }
        }

        if let Some(memo) = self.authoritative_store.find_by_id(user_id, id).await? {
            if let Err(error) = self.cache.set_memo(&memo, Some(CACHE_TTL)).await {
                log::warn!(
                    "Cache fill failed after authoritative store read: memo_id={id} user_id={user_id} error={error}"
                );
            }
            return Ok(Some(memo));
        }

        Ok(None)
    }

    async fn list_page_by_user_id(
        &self,
        user_id: Uuid,
        after: Option<Uuid>,
        limit: usize,
    ) -> AppResult<MemoListPage> {
        self.authoritative_store
            .list_page_by_user_id(user_id, after, limit)
            .await
    }

    async fn find_many_by_ids(&self, user_id: Uuid, ids: &[Uuid]) -> AppResult<Vec<Memo>> {
        self.authoritative_store
            .find_many_by_ids(user_id, ids)
            .await
    }

    async fn save(&self, memo: &Memo) -> AppResult<()> {
        let intent = self
            .authoritative_store
            .save_with_projection_intent(memo)
            .await?;
        // Do not depend on the search/maintenance barrier being available to
        // invalidate an old cache entry. The primary mutation has committed;
        // a failed synchronous projection must not leave an old cached version
        // visible to subsequent reads while the durable outbox catches up.
        if let Err(error) = self.cache.delete_memo(memo.user_id, memo.id).await {
            log::warn!(
                "Post-commit memo cache invalidation failed; durable projection intent retained: memo_id={} user_id={} error={error}",
                memo.id,
                memo.user_id
            );
        }
        self.reconciler.reconcile_now(&intent).await;
        Ok(())
    }

    async fn delete(&self, user_id: Uuid, id: Uuid) -> AppResult<()> {
        let intent = self
            .authoritative_store
            .delete_with_projection_intent(user_id, id)
            .await?;
        // Deletions are particularly sensitive: without early eviction an
        // offline projector can leave a deleted memo readable from Valkey.
        // Retain outbox-based retries if cache invalidation itself fails.
        if let Err(error) = self.cache.delete_memo(user_id, id).await {
            log::warn!(
                "Post-commit deleted memo cache invalidation failed; durable projection intent retained: memo_id={id} user_id={user_id} error={error}"
            );
        }
        self.reconciler.reconcile_now(&intent).await;
        Ok(())
    }

    async fn search(
        &self,
        query: &str,
        tag: Option<String>,
        user_id: Uuid,
        page: usize,
        limit: usize,
    ) -> AppResult<crate::domain::memo::repository::MemoSearchPage> {
        let hits = self
            .search_projection
            .search_memo_ids(query, tag, user_id, page, limit)
            .await?;
        let items = self
            .authoritative_store
            .find_many_by_ids(user_id, &hits.memo_ids)
            .await?;

        Ok(crate::domain::memo::repository::MemoSearchPage {
            items,
            total: hits.total,
        })
    }

    async fn exists(&self, user_id: Uuid, id: Uuid) -> AppResult<bool> {
        match self.cache.memo_exists(user_id, id).await {
            Ok(true) => return Ok(true),
            Ok(false) => {}
            Err(error) => {
                log::warn!(
                    "Cache existence check failed; falling back to authoritative store: memo_id={id} user_id={user_id} error={error}"
                );
            }
        }

        self.authoritative_store.exists(user_id, id).await
    }
}
