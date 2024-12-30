//src/infrastructure/persistence/redis.rs
use std::time::Duration;
use redis::{Client, AsyncCommands, RedisError};
use serde::{Serialize, de::DeserializeOwned};
use tracing::error;
use crate::error::{AppError, AppResult};

/// Redisキャッシュ層の実装
/// 
/// この実装は以下の特徴を持ちます：
/// - 非同期処理による高性能な操作
/// - シリアライズ/デシリアライズの型安全性
/// - 包括的なエラーハンドリング
/// - タイムアウト制御
pub struct RedisCache {
    client: Client,
}

impl RedisCache {
    /// 新しいRedisクライアントインスタンスを生成
    ///
    /// # Arguments
    /// * `uri` - RedisサーバーのURI（例: "redis://localhost:6379"）
    pub fn new(uri: &str) -> AppResult<Self> {
        let client = Client::open(uri).map_err(|e| {
            error!("Failed to connect to Redis: {}", e);
            AppError::DatabaseError(format!("Failed to connect to Redis: {}", e))
        })?;

        Ok(Self { client })
    }

    /// キーに対応する値を取得
    ///
    /// # Type Parameters
    /// * `T` - デシリアライズ対象の型（DeserializeOwned トレイトを実装している必要あり）
    pub async fn get<T: DeserializeOwned>(&self, key: &str) -> AppResult<Option<T>> {
        let mut conn = self.client.get_async_connection().await.map_err(|e| {
            error!("Failed to get Redis connection: {}", e);
            AppError::DatabaseError(e.to_string())
        })?;

        let result: Option<String> = conn.get(key).await.map_err(|e| {
            error!("Failed to get value from Redis: {}", e);
            AppError::DatabaseError(e.to_string())
        })?;

        match result {
            Some(value) => {
                let parsed = serde_json::from_str(&value).map_err(|e| {
                    error!("Failed to parse Redis value: {}", e);
                    AppError::DatabaseError(e.to_string())
                })?;
                Ok(Some(parsed))
            }
            None => Ok(None),
        }
    }

    /// キーに対して値を設定（オプションでTTLを指定可能）
    ///
    /// # Type Parameters
    /// * `T` - シリアライズ対象の型（Serialize トレイトを実装している必要あり）
    pub async fn set<T: Serialize>(
        &self,
        key: &str,
        value: &T,
        expiration: Option<Duration>,
    ) -> AppResult<()> {
        let mut conn = self.client.get_async_connection().await.map_err(|e| {
            error!("Failed to get Redis connection: {}", e);
            AppError::DatabaseError(e.to_string())
        })?;

        let serialized = serde_json::to_string(value).map_err(|e| {
            error!("Failed to serialize value: {}", e);
            AppError::DatabaseError(e.to_string())
        })?;

        match expiration {
            Some(exp) => {
                // TTLの単位を秒に変換（u64として扱う）
                let seconds: u64 = exp.as_secs();
                conn.set_ex(key, serialized, seconds).await.map_err(|e| {
                    error!("Failed to set value in Redis with expiration: {}", e);
                    AppError::DatabaseError(e.to_string())
                })?;
            }
            None => {
                conn.set(key, serialized).await.map_err(|e| {
                    error!("Failed to set value in Redis: {}", e);
                    AppError::DatabaseError(e.to_string())
                })?;
            }
        }

        Ok(())
    }

    /// キーを削除
    pub async fn delete(&self, key: &str) -> AppResult<()> {
        let mut conn = self.client.get_async_connection().await.map_err(|e| {
            error!("Failed to get Redis connection: {}", e);
            AppError::DatabaseError(e.to_string())
        })?;

        conn.del(key).await.map_err(|e| {
            error!("Failed to delete key from Redis: {}", e);
            AppError::DatabaseError(e.to_string())
        })?;

        Ok(())
    }

    /// キーの存在確認
    pub async fn exists(&self, key: &str) -> AppResult<bool> {
        let mut conn = self.client.get_async_connection().await.map_err(|e| {
            error!("Failed to get Redis connection: {}", e);
            AppError::DatabaseError(e.to_string())
        })?;

        let exists: bool = conn.exists(key).await.map_err(|e| {
            error!("Failed to check key existence in Redis: {}", e);
            AppError::DatabaseError(e.to_string())
        })?;

        Ok(exists)
    }
}

/// ヘルスチェック用の関数
/// PINGコマンドを使用してRedisサーバーの応答を確認
pub async fn health_check(redis_client: &Client) -> Result<(), RedisError> {
    let mut conn = redis_client.get_async_connection().await?;
    redis::cmd("PING").query_async(&mut conn).await
}