use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
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
        if let Some(memo) = self.redis.get::<Memo>(&cache_key).await? {
            return Ok(Some(memo));
        }

        if let Some(memo) = self.scylla.find_by_id(user_id, id).await? {
            self.redis.set(&cache_key, &memo, Some(CACHE_TTL)).await?;
            return Ok(Some(memo));
        }

        Ok(None)
    }

    async fn find_all_by_user_id(&self, user_id: Uuid) -> AppResult<Vec<Memo>> {
        self.scylla.find_all_by_user_id(user_id).await
    }

    async fn save(&self, memo: &Memo) -> AppResult<()> {
        self.scylla.save(memo).await?;
        self.elasticsearch.index_memo(memo).await?;

        let cache_key = Self::cache_key(memo.user_id, memo.id);
        self.redis.set(&cache_key, memo, Some(CACHE_TTL)).await?;

        Ok(())
    }

    async fn delete(&self, user_id: Uuid, id: Uuid) -> AppResult<()> {
        self.scylla.delete(user_id, id).await?;
        self.elasticsearch.delete_memo(id).await?;
        self.redis.delete(&Self::cache_key(user_id, id)).await?;
        Ok(())
    }

    async fn search(
        &self,
        query: &str,
        tag: Option<String>,
        user_id: Uuid,
    ) -> AppResult<Vec<Memo>> {
        self.elasticsearch.search_memos(query, tag, user_id).await
    }

    async fn exists(&self, user_id: Uuid, id: Uuid) -> AppResult<bool> {
        let cache_key = Self::cache_key(user_id, id);
        if self.redis.exists(&cache_key).await? {
            return Ok(true);
        }

        self.scylla.exists(user_id, id).await
    }
}
