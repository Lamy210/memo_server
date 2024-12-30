// src/infrastructure/repositories/memo.rs

use std::sync::Arc;
use async_trait::async_trait;
use uuid::Uuid;
use scylla::{
    frame::response::result::Row,
    serialize::row::SerializeRow,
    deserialize::{DeserializeRow, TypeCheck, ColumnSpec, TypeCheckError, DeserializationError, ColumnIterator},
};
use tracing::{debug, instrument};

use crate::{
    domain::memo::{entity::Memo, repository::MemoRepository},
    error::{AppError, AppResult},
    infrastructure::persistence::{
        scylla::ScyllaDB,
        redis::RedisCache,
        elasticsearch::ElasticsearchClient,
    },
};

/// メモリポジトリの実装
pub struct MemoRepositoryImpl {
    scylla: Arc<ScyllaDB>,
    redis: Arc<RedisCache>,
    elasticsearch: Arc<ElasticsearchClient>,
}

impl MemoRepositoryImpl {
    /// 新しいリポジトリインスタンスを生成
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

    /// キャッシュからメモを取得
    async fn get_from_cache(&self, id: Uuid) -> AppResult<Option<Memo>> {
        let cache_key = format!("memo:{}", id);
        self.redis.get(&cache_key).await
    }

    /// メモをキャッシュに保存
    async fn set_cache(&self, memo: &Memo) -> AppResult<()> {
        let cache_key = format!("memo:{}", memo.id);
        self.redis.set(&cache_key, memo, None).await
    }

    /// キャッシュを無効化
    async fn invalidate_cache(&self, id: Uuid) -> AppResult<()> {
        let cache_key = format!("memo:{}", id);
        self.redis.delete(&cache_key).await
    }
}

/// ScyllaDBのデシリアライズ実装
impl<'a, 'b> DeserializeRow<'a, 'b> for Memo {
    fn type_check(specs: &[ColumnSpec]) -> Result<(), TypeCheckError> {
        if specs.len() != 8 {
            return Err(TypeCheckError::BadCount{
                expected: 8,
                actual: specs.len(),
            });
        }
        Ok(())
    }

    fn deserialize(mut iter: ColumnIterator<'a, 'b>) -> Result<Self, DeserializationError> {
        Ok(Self {
            id: iter.deserialize_next()?,
            title: iter.deserialize_next()?,
            content: iter.deserialize_next()?,
            tags: iter.deserialize_next()?,
            user_id: iter.deserialize_next()?,
            created_at: iter.deserialize_next()?,
            updated_at: iter.deserialize_next()?,
            version: iter.deserialize_next()?,
        })
    }
}

#[async_trait]
impl MemoRepository for MemoRepositoryImpl {
    #[instrument(skip(self))]
    async fn find_by_id(&self, id: Uuid) -> AppResult<Option<Memo>> {
        // キャッシュチェック
        if let Some(memo) = self.get_from_cache(id).await? {
            debug!("Cache hit for memo: {}", id);
            return Ok(Some(memo));
        }

        debug!("Cache miss for memo: {}, querying database", id);
        let statements = self.scylla.get_prepared_statements().await?;
        
        let result = self.scylla
            .execute_one::<Memo, _>(&statements.select_memo_by_id, (id,))
            .await?;

        // キャッシュの更新
        if let Some(memo) = result.as_ref() {
            debug!("Updating cache for memo: {}", id);
            self.set_cache(memo).await?;
        }

        Ok(result)
    }

    #[instrument(skip(self))]
    async fn find_all_by_user_id(&self, user_id: Uuid) -> AppResult<Vec<Memo>> {
        let statements = self.scylla.get_prepared_statements().await?;
        
        self.scylla
            .execute::<Memo, _>(&statements.select_memos_by_user_id, (user_id,))
            .await
    }

    #[instrument(skip(self, memo))]
    async fn save(&self, memo: &Memo) -> AppResult<()> {
        let statements = self.scylla.get_prepared_statements().await?;

        if memo.version == 1 {
            // 新規作成
            self.scylla
                .execute_one::<Memo, _>(
                    &statements.insert_memo,
                    (
                        memo.id,
                        &memo.title,
                        &memo.content,
                        &memo.tags,
                        memo.user_id,
                        memo.created_at,
                        memo.updated_at,
                        memo.version,
                    ),
                )
                .await?;
        } else {
            // 更新（楽観的ロック使用）
            let result = self.scylla
                .execute_one::<Memo, _>(
                    &statements.update_memo,
                    (
                        &memo.title,
                        &memo.content,
                        &memo.tags,
                        memo.updated_at,
                        memo.version,
                        memo.id,
                        memo.version - 1,
                    ),
                )
                .await?;

            if result.is_none() {
                return Err(AppError::Conflict("Memo has been modified".into()));
            }
        }

        // キャッシュの無効化
        self.invalidate_cache(memo.id).await?;

        // Elasticsearchのインデックス更新
        self.elasticsearch.index_memo(memo).await?;

        Ok(())
    }

    #[instrument(skip(self))]
    async fn delete(&self, id: Uuid) -> AppResult<()> {
        let statements = self.scylla.get_prepared_statements().await?;
        
        self.scylla
            .execute_one::<Memo, _>(&statements.delete_memo, (id,))
            .await?;

        // キャッシュの無効化
        self.invalidate_cache(id).await?;

        // Elasticsearchからの削除
        self.elasticsearch.delete_memo(id).await?;

        Ok(())
    }

    #[instrument(skip(self))]
    async fn search(&self, query: &str, tag: Option<String>, user_id: Uuid) -> AppResult<Vec<Memo>> {
        self.elasticsearch
            .search_memos(query, tag, user_id)
            .await
    }

    #[instrument(skip(self))]
    async fn exists(&self, id: Uuid) -> AppResult<bool> {
        // キャッシュチェック
        if let Some(_) = self.get_from_cache(id).await? {
            return Ok(true);
        }

        let statements = self.scylla.get_prepared_statements().await?;
        let result = self.scylla
            .execute_one::<i32, _>(&statements.select_memo_exists, (id,))
            .await?;

        Ok(result.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio;

    async fn setup_test_repository() -> MemoRepositoryImpl {
        let scylla = Arc::new(
            ScyllaDB::new("scylla://localhost:9042")
                .await
                .expect("Failed to connect to ScyllaDB")
        );
        
        let redis = Arc::new(
            RedisCache::new("redis://localhost:6379")
                .expect("Failed to connect to Redis")
        );
        
        let elasticsearch = Arc::new(
            ElasticsearchClient::new("http://localhost:9200")
                .await
                .expect("Failed to connect to Elasticsearch")
        );

        MemoRepositoryImpl::new(scylla, redis, elasticsearch)
    }

    #[tokio::test]
    async fn test_crud_operations() {
        let repo = setup_test_repository().await;
        
        // テストメモの作成
        let memo = Memo::new(
            "Test Title".to_string(),
            "Test Content".to_string(),
            vec!["test".to_string()],
            Uuid::new_v4(),
        );

        // 保存テスト
        repo.save(&memo).await.expect("Failed to save memo");

        // 検索テスト
        let found = repo.find_by_id(memo.id).await.expect("Failed to find memo");
        assert!(found.is_some());
        assert_eq!(found.unwrap().title, memo.title);

        // 削除テスト
        repo.delete(memo.id).await.expect("Failed to delete memo");
        
        // 削除確認
        let not_found = repo.find_by_id(memo.id).await.expect("Failed to check memo");
        assert!(not_found.is_none());
    }
}