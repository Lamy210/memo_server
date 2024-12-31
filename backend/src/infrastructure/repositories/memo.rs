// src/infrastructure/repositories/memo.rs

use std::sync::Arc;
use async_trait::async_trait;
use uuid::Uuid;
use crate::{
    domain::memo::{entity::Memo, repository::MemoRepository},
    error::AppResult,
    infrastructure::persistence::{
        scylla::ScyllaDB,
        redis::RedisCache,
        elasticsearch::ElasticsearchClient,
    },
};

const CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(3600); // 1時間

pub struct MemoRepositoryImpl {
    scylla: Arc<ScyllaDB>,
    redis: Arc<RedisCache>,
    elasticsearch: Arc<ElasticsearchClient>,
}

impl MemoRepositoryImpl {
    pub async fn new(
        scylla: Arc<ScyllaDB>,
        redis: Arc<RedisCache>,
        elasticsearch: Arc<ElasticsearchClient>,
    ) -> AppResult<Self> {
        Ok(Self {
            scylla,
            redis,
            elasticsearch,
        })
    }

    fn cache_key(id: Uuid) -> String {
        format!("memo:{}", id)
    }
}

#[async_trait]
impl MemoRepository for MemoRepositoryImpl {
    async fn find_by_id(&self, id: Uuid) -> AppResult<Option<Memo>> {
        // キャッシュから取得を試みる
        let cache_key = Self::cache_key(id);
        if let Some(memo) = self.redis.get::<Memo>(&cache_key).await? {
            return Ok(Some(memo));
        }

        // ScyllaDBから取得
        if let Some(memo) = self.scylla.find_by_id(id).await? {
            // キャッシュに保存
            self.redis.set(&cache_key, &memo, Some(CACHE_TTL)).await?;
            return Ok(Some(memo));
        }

        Ok(None)
    }

    async fn find_all_by_user_id(&self, user_id: Uuid) -> AppResult<Vec<Memo>> {
        self.scylla.find_all_by_user_id(user_id).await
    }

    async fn save(&self, memo: &Memo) -> AppResult<()> {
        // ScyllaDBに保存
        self.scylla.save(memo).await?;

        // Elasticsearchにインデックス
        self.elasticsearch.index_memo(memo).await?;

        // キャッシュを更新
        let cache_key = Self::cache_key(memo.id);
        self.redis.set(&cache_key, memo, Some(CACHE_TTL)).await?;

        Ok(())
    }

    async fn delete(&self, id: Uuid) -> AppResult<()> {
        // ScyllaDBから削除
        self.scylla.delete(id).await?;

        // Elasticsearchから削除
        self.elasticsearch.delete_memo(id).await?;

        // キャッシュから削除
        self.redis.delete(&Self::cache_key(id)).await?;

        Ok(())
    }

    async fn search(&self, query: &str, tag: Option<String>, user_id: Uuid) -> AppResult<Vec<Memo>> {
        self.elasticsearch.search_memos(query, tag, user_id).await
    }

    async fn exists(&self, id: Uuid) -> AppResult<bool> {
        // キャッシュをチェック
        let cache_key = Self::cache_key(id);
        if self.redis.exists(&cache_key).await? {
            return Ok(true);
        }

        // データベースをチェック
        self.scylla.exists(id).await
    }
}