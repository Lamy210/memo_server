use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use uuid::Uuid;

use crate::{
    domain::memo::{entity::Memo, repository::MemoRepository},
    error::AppResult,
    infrastructure::{
        persistence::{
            elasticsearch::ElasticsearchClient,
            redis::RedisCache,
            scylla::{ScyllaDB, PROJECTION_DELETE_TARGET},
        },
        reconciliation::ProjectionReconciler,
    },
};

const CACHE_TTL: Duration = Duration::from_secs(3600);

pub struct MemoRepositoryImpl {
    scylla: Arc<ScyllaDB>,
    redis: Arc<RedisCache>,
    elasticsearch: Arc<ElasticsearchClient>,
    reconciler: Arc<ProjectionReconciler>,
}

impl MemoRepositoryImpl {
    pub fn new(
        scylla: Arc<ScyllaDB>,
        redis: Arc<RedisCache>,
        elasticsearch: Arc<ElasticsearchClient>,
        reconciler: Arc<ProjectionReconciler>,
    ) -> Self {
        Self {
            scylla,
            redis,
            elasticsearch,
            reconciler,
        }
    }

    fn cache_key(user_id: Uuid, id: Uuid) -> String {
        format!("memo:{user_id}:{id}")
    }
}

#[async_trait]
impl MemoRepository for MemoRepositoryImpl {
    async fn find_by_id(&self, user_id: Uuid, id: Uuid) -> AppResult<Option<Memo>> {
        let cache_key = Self::cache_key(user_id, id);
        match self.redis.get::<Memo>(&cache_key).await {
            Ok(Some(memo)) => return Ok(Some(memo)),
            Ok(None) => {}
            Err(error) => {
                log::warn!(
                    "Redis lookup failed; falling back to Scylla: memo_id={id} user_id={user_id} error={error}"
                );
            }
        }

        if let Some(memo) = self.scylla.find_by_id(user_id, id).await? {
            if let Err(error) = self.redis.set(&cache_key, &memo, Some(CACHE_TTL)).await {
                log::warn!(
                    "Redis cache fill failed after Scylla read: memo_id={id} user_id={user_id} error={error}"
                );
            }
            return Ok(Some(memo));
        }

        Ok(None)
    }

    async fn find_all_by_user_id(&self, user_id: Uuid) -> AppResult<Vec<Memo>> {
        self.scylla.find_all_by_user_id(user_id).await
    }

    async fn save(&self, memo: &Memo) -> AppResult<()> {
        let intent = self
            .reconciler
            .prepare(memo.user_id, memo.id, memo.version)
            .await?;

        if let Err(error) = self.scylla.save(memo).await {
            self.reconciler.cancel(&intent).await;
            return Err(error);
        }

        self.reconciler.reconcile_now(&intent).await;
        Ok(())
    }

    async fn delete(&self, user_id: Uuid, id: Uuid) -> AppResult<()> {
        let intent = self
            .reconciler
            .prepare(user_id, id, PROJECTION_DELETE_TARGET)
            .await?;

        if let Err(error) = self.scylla.delete(user_id, id).await {
            self.reconciler.cancel(&intent).await;
            return Err(error);
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
        self.elasticsearch
            .search_memos(query, tag, user_id, page, limit)
            .await
    }

    async fn exists(&self, user_id: Uuid, id: Uuid) -> AppResult<bool> {
        let cache_key = Self::cache_key(user_id, id);
        match self.redis.exists(&cache_key).await {
            Ok(true) => return Ok(true),
            Ok(false) => {}
            Err(error) => {
                log::warn!(
                    "Redis existence check failed; falling back to Scylla: memo_id={id} user_id={user_id} error={error}"
                );
            }
        }

        self.scylla.exists(user_id, id).await
    }
}
