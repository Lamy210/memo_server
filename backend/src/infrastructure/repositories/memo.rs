use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use tracing::warn;
use uuid::Uuid;

use crate::{
    domain::memo::{entity::Memo, repository::MemoRepository},
    error::AppResult,
    infrastructure::persistence::{
        elasticsearch::ElasticsearchClient, redis::RedisCache, scylla::ScyllaDB,
    },
};

const CACHE_TTL: Duration = Duration::from_secs(3600);

pub struct MemoRepositoryImpl {
    scylla: Arc<ScyllaDB>,
    redis: Arc<RedisCache>,
    elasticsearch: Arc<ElasticsearchClient>,
}

impl MemoRepositoryImpl {
    pub fn new(
        scylla: Arc<ScyllaDB>,
        redis: Arc<RedisCache>,
        elasticsearch: Arc<ElasticsearchClient>,
    ) -> Self {
        Self {
            scylla,
            redis,
            elasticsearch,
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
                warn!(
                    memo_id = %id,
                    user_id = %user_id,
                    error = %error,
                    "Redis lookup failed; falling back to Scylla"
                );
            }
        }

        if let Some(memo) = self.scylla.find_by_id(user_id, id).await? {
            if let Err(error) = self.redis.set(&cache_key, &memo, Some(CACHE_TTL)).await {
                warn!(
                    memo_id = %id,
                    user_id = %user_id,
                    error = %error,
                    "Redis cache fill failed after Scylla read"
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
        self.scylla.save(memo).await?;

        if let Err(error) = self.elasticsearch.index_memo(memo).await {
            warn!(
                memo_id = %memo.id,
                user_id = %memo.user_id,
                error = %error,
                "Elasticsearch projection update failed after Scylla commit"
            );
        }

        let cache_key = Self::cache_key(memo.user_id, memo.id);
        if let Err(error) = self.redis.set(&cache_key, memo, Some(CACHE_TTL)).await {
            warn!(
                memo_id = %memo.id,
                user_id = %memo.user_id,
                error = %error,
                "Redis cache update failed after Scylla commit"
            );
        }

        Ok(())
    }

    async fn delete(&self, user_id: Uuid, id: Uuid) -> AppResult<()> {
        self.scylla.delete(user_id, id).await?;

        if let Err(error) = self.elasticsearch.delete_memo(id).await {
            warn!(
                memo_id = %id,
                user_id = %user_id,
                error = %error,
                "Elasticsearch projection delete failed after Scylla delete"
            );
        }
        if let Err(error) = self.redis.delete(&Self::cache_key(user_id, id)).await {
            warn!(
                memo_id = %id,
                user_id = %user_id,
                error = %error,
                "Redis cache invalidation failed after Scylla delete"
            );
        }

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
                warn!(
                    memo_id = %id,
                    user_id = %user_id,
                    error = %error,
                    "Redis existence check failed; falling back to Scylla"
                );
            }
        }

        self.scylla.exists(user_id, id).await
    }
}
