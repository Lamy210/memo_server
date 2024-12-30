// src/infrastructure/repositories/memo.rs

use std::{sync::Arc, time::Duration};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use uuid::Uuid;
use scylla::{
    Row,
    FromRow,
    error::FromRowError,
    frame::value::{Value, CqlTimestamp},
    SerializeRow,
    IntoTypedRows,
};
use crate::{
    domain::memo::{entity::Memo, repository::MemoRepository},
    error::{AppError, AppResult},
    infrastructure::persistence::{
        scylla::{ScyllaDB, ScyllaDBPreparedStatements},
        redis::RedisCache,
        elasticsearch::ElasticsearchClient,
    },
};

/// メモリポジトリの実装
/// 
/// DDDのリポジトリパターンに基づく、複合永続化戦略を実装
pub struct MemoRepositoryImpl {
    scylla: Arc<ScyllaDB>,
    prepared_statements: Arc<ScyllaDBPreparedStatements>,
    redis: Arc<RedisCache>,
    elasticsearch: Arc<ElasticsearchClient>,
}

impl MemoRepositoryImpl {
    /// 新しいリポジトリインスタンスを生成
    pub async fn new(
        scylla: Arc<ScyllaDB>,
        redis: Arc<RedisCache>,
        elasticsearch: Arc<ElasticsearchClient>,
    ) -> AppResult<Self> {
        let prepared_statements = Arc::new(
            ScyllaDBPreparedStatements::prepare(scylla.get_session())
                .await
                .map_err(|e| AppError::DatabaseError(e.to_string()))?,
        );

        Ok(Self {
            scylla,
            prepared_statements,
            redis,
            elasticsearch,
        })
    }

    /// キャッシュからメモを取得
    async fn get_from_cache(&self, id: Uuid) -> AppResult<Option<Memo>> {
        let cache_key = format!("memo:{}", id);
        self.redis.get(&cache_key).await
    }

    /// メモをキャッシュに保存（1時間の有効期限）
    async fn set_cache(&self, memo: &Memo) -> AppResult<()> {
        let cache_key = format!("memo:{}", memo.id);
        self.redis
            .set(&cache_key, memo, Some(Duration::from_secs(3600)))
            .await
    }

    /// キャッシュを無効化
    async fn invalidate_cache(&self, id: Uuid) -> AppResult<()> {
        let cache_key = format!("memo:{}", id);
        self.redis.delete(&cache_key).await
    }
}

/// Implement `FromRow` for `Memo` to allow scylla to convert rows directly into `Memo` instances.
impl FromRow for Memo {
    fn from_row(row: &Row) -> Result<Self, FromRowError> {
        Ok(Memo {
            id: row
                .get("id")
                .ok_or_else(|| FromRowError::MissingColumn("id".to_string()))?,
            title: row
                .get("title")
                .ok_or_else(|| FromRowError::MissingColumn("title".to_string()))?,
            content: row
                .get("content")
                .ok_or_else(|| FromRowError::MissingColumn("content".to_string()))?,
            tags: row
                .get("tags")
                .ok_or_else(|| FromRowError::MissingColumn("tags".to_string()))?
                .into_iter()
                .map(|s| s.to_string())
                .collect(),
            user_id: row
                .get("user_id")
                .ok_or_else(|| FromRowError::MissingColumn("user_id".to_string()))?,
            created_at: DateTime::<Utc>::from(
                row.get::<CqlTimestamp>("created_at")
                    .ok_or_else(|| FromRowError::MissingColumn("created_at".to_string()))?
                    .to_chrono(),
            ),
            updated_at: DateTime::<Utc>::from(
                row.get::<CqlTimestamp>("updated_at")
                    .ok_or_else(|| FromRowError::MissingColumn("updated_at".to_string()))?
                    .to_chrono(),
            ),
            version: row
                .get("version")
                .ok_or_else(|| FromRowError::MissingColumn("version".to_string()))?,
        })
    }
}

#[async_trait]
impl MemoRepository for MemoRepositoryImpl {
    async fn find_by_id(&self, id: Uuid) -> AppResult<Option<Memo>> {
        // まずキャッシュを確認
        if let Some(memo) = self.get_from_cache(id).await? {
            return Ok(Some(memo));
        }

        // ScyllaDBから取得
        let values: &[&dyn Value] = &[&id as &dyn Value];
        let result = self
            .scylla
            .get_session()
            .execute(&self.prepared_statements.select_memo_by_id, values)
            .await
            .map_err(|e| AppError::DatabaseError(e.to_string()))?;

        let memo = match result.first_row()? {
            Some(row) => {
                let memo: Memo = row.into()?;
                // キャッシュを更新
                self.set_cache(&memo).await?;
                Some(memo)
            }
            None => None,
        };

        Ok(memo)
    }

    async fn find_all_by_user_id(&self, user_id: Uuid) -> AppResult<Vec<Memo>> {
        let values: &[&dyn Value] = &[&user_id as &dyn Value];
        let result = self
            .scylla
            .get_session()
            .execute(&self.prepared_statements.select_memos_by_user_id, values)
            .await
            .map_err(|e| AppError::DatabaseError(e.to_string()))?;

        let mut memos = Vec::new();
        if let Some(rows) = result.rows {
            for row in rows {
                let memo: Memo = row.into()?;
                memos.push(memo);
            }
        }

        Ok(memos)
    }

    async fn save(&self, memo: &Memo) -> AppResult<()> {
        if memo.version == 1 {
            // 新規作成
            let values: &[&dyn Value] = &[
                &memo.id as &dyn Value,
                &memo.title as &dyn Value,
                &memo.content as &dyn Value,
                &memo.tags as &dyn Value,
                &memo.user_id as &dyn Value,
                &CqlTimestamp::from(memo.created_at) as &dyn Value,
                &CqlTimestamp::from(memo.updated_at) as &dyn Value,
                &memo.version as &dyn Value,
            ];

            self.scylla
                .get_session()
                .execute(&self.prepared_statements.insert_memo, values)
                .await
                .map_err(|e| AppError::DatabaseError(e.to_string()))?;
        } else {
            // 更新（楽観的ロック使用）
            let values: &[&dyn Value] = &[
                &memo.title as &dyn Value,
                &memo.content as &dyn Value,
                &memo.tags as &dyn Value,
                &CqlTimestamp::from(memo.updated_at) as &dyn Value,
                &memo.version as &dyn Value,
                &memo.id as &dyn Value,
                &(memo.version - 1) as &dyn Value,
            ];

            let result = self
                .scylla
                .get_session()
                .execute(&self.prepared_statements.update_memo, values)
                .await
                .map_err(|e| AppError::DatabaseError(e.to_string()))?;

            if let Some(rows) = result.rows {
                // Check if the IF condition was met
                if rows.is_empty() {
                    return Err(AppError::Conflict("Memo has been modified".into()));
                }
            } else {
                return Err(AppError::Conflict("Memo has been modified".into()));
            }
        }

        // キャッシュを無効化
        self.invalidate_cache(memo.id).await?;

        // Elasticsearchにインデックス
        self.elasticsearch.index_memo(memo.id, memo).await?;

        Ok(())
    }

    async fn delete(&self, id: Uuid) -> AppResult<()> {
        let values: &[&dyn Value] = &[&id as &dyn Value];

        // ScyllaDBから削除
        self.scylla
            .get_session()
            .execute(&self.prepared_statements.delete_memo, values)
            .await
            .map_err(|e| AppError::DatabaseError(e.to_string()))?;

        // キャッシュを無効化
        self.invalidate_cache(id).await?;

        // Elasticsearchから削除
        self.elasticsearch.delete_memo(id).await?;

        Ok(())
    }

    async fn search(&self, query: &str, tag: Option<String>, user_id: Uuid) -> AppResult<Vec<Memo>> {
        self.elasticsearch
            .search_memos(query, tag, user_id)
            .await
    }

    async fn exists(&self, id: Uuid) -> AppResult<bool> {
        // まずキャッシュをチェック
        let cache_key = format!("memo:{}", id);
        if self.redis.exists(&cache_key).await? {
            return Ok(true);
        }

        // ScyllaDBをチェック
        let values: &[&dyn Value] = &[&id as &dyn Value];
        let result = self
            .scylla
            .get_session()
            .execute(&self.prepared_statements.select_memo_by_id, values)
            .await
            .map_err(|e| AppError::DatabaseError(e.to_string()))?;

        Ok(result.rows.map_or(false, |rows| !rows.is_empty()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio;

    #[tokio::test]
    async fn test_memo_crud_operations() {
        // TODO: テストケースの実装
        // モックを使用したCRUD操作のテスト
    }
}
