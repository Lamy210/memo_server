use std::time::Duration;

use async_trait::async_trait;
use redis::{AsyncCommands, Client};
use serde::{de::DeserializeOwned, Serialize};
use tracing::error;

use crate::{
    application::{
        crypto::HighEncryptedMemoEnvelope, crypto_cache::HighEncryptedMemoCache,
        health::HealthProbe,
    },
    domain::memo::entity::Memo,
    error::{AppError, AppResult},
};

use super::ports::MemoCache;

const LEGACY_CACHE_NAMESPACE: &str = "memo";
const HIGH_CACHE_NAMESPACE: &str = "memo:high:v1";

pub struct RedisCache {
    client: Client,
}

impl RedisCache {
    pub fn new(uri: &str) -> AppResult<Self> {
        let client = Client::open(uri).map_err(|error| {
            error!("Failed to create Redis client: {error}");
            AppError::DatabaseError(format!("Failed to create Redis client: {error}"))
        })?;
        Ok(Self { client })
    }

    pub async fn get<T: DeserializeOwned>(&self, key: &str) -> AppResult<Option<T>> {
        let mut connection = self.connection().await?;
        let value: Option<String> = connection.get(key).await.map_err(|error| {
            error!("Failed to get Redis value: {error}");
            AppError::DatabaseError(error.to_string())
        })?;

        value
            .map(|serialized| {
                serde_json::from_str(&serialized).map_err(|error| {
                    error!("Failed to deserialize Redis value: {error}");
                    AppError::DatabaseError(error.to_string())
                })
            })
            .transpose()
    }

    pub async fn set<T: Serialize>(
        &self,
        key: &str,
        value: &T,
        expiration: Option<Duration>,
    ) -> AppResult<()> {
        let mut connection = self.connection().await?;
        let serialized = serde_json::to_string(value).map_err(|error| {
            error!("Failed to serialize Redis value: {error}");
            AppError::DatabaseError(error.to_string())
        })?;

        match expiration {
            Some(expiration) => {
                let _: () = connection
                    .set_ex(key, serialized, expiration.as_secs())
                    .await
                    .map_err(|error| {
                        error!("Failed to set Redis value with expiration: {error}");
                        AppError::DatabaseError(error.to_string())
                    })?;
            }
            None => {
                let _: () = connection.set(key, serialized).await.map_err(|error| {
                    error!("Failed to set Redis value: {error}");
                    AppError::DatabaseError(error.to_string())
                })?;
            }
        }

        Ok(())
    }

    pub async fn delete(&self, key: &str) -> AppResult<()> {
        let mut connection = self.connection().await?;
        let _: usize = connection.del(key).await.map_err(|error| {
            error!("Failed to delete Redis key: {error}");
            AppError::DatabaseError(error.to_string())
        })?;
        Ok(())
    }

    pub async fn exists(&self, key: &str) -> AppResult<bool> {
        let mut connection = self.connection().await?;
        connection.exists(key).await.map_err(|error| {
            error!("Failed to check Redis key existence: {error}");
            AppError::DatabaseError(error.to_string())
        })
    }

    pub async fn health_check(&self) -> AppResult<bool> {
        let mut connection = self.connection().await?;
        let pong: String = redis::cmd("PING")
            .query_async(&mut connection)
            .await
            .map_err(|error| {
                error!("Redis health check failed: {error}");
                AppError::DatabaseError(error.to_string())
            })?;
        Ok(pong == "PONG")
    }

    async fn connection(&self) -> AppResult<redis::aio::MultiplexedConnection> {
        self.client
            .get_multiplexed_async_connection()
            .await
            .map_err(|error| {
                error!("Failed to get Redis connection: {error}");
                AppError::DatabaseError(error.to_string())
            })
    }

    fn legacy_cache_key(owner_partition: uuid::Uuid, memo_id: uuid::Uuid) -> String {
        format!("{LEGACY_CACHE_NAMESPACE}:{owner_partition}:{memo_id}")
    }

    fn high_cache_key(owner_partition: uuid::Uuid, memo_id: uuid::Uuid) -> String {
        format!("{HIGH_CACHE_NAMESPACE}:{owner_partition}:{memo_id}")
    }

    fn validate_cached_memo(
        owner_partition: uuid::Uuid,
        memo_id: uuid::Uuid,
        memo: &Memo,
    ) -> AppResult<()> {
        if memo.user_id != owner_partition || memo.id != memo_id {
            return Err(AppError::DatabaseError(
                "Legacy cache memo identity does not match the requested cache key".into(),
            ));
        }
        Ok(())
    }

    fn validate_cached_envelope(
        owner_partition: uuid::Uuid,
        memo_id: uuid::Uuid,
        envelope: &HighEncryptedMemoEnvelope,
    ) -> AppResult<()> {
        envelope.validate_structure()?;
        if envelope.owner_partition != owner_partition || envelope.memo_id != memo_id {
            return Err(AppError::DatabaseError(
                "HIGH cache envelope identity does not match the requested cache key".into(),
            ));
        }
        Ok(())
    }

    async fn purge_invalid_cached_value(
        &self,
        key: &str,
        context: &str,
        primary: AppError,
    ) -> AppError {
        match self.delete(key).await {
            Ok(()) => primary,
            Err(purge) => AppError::DatabaseError(format!(
                "{context} and invalid-entry purge also failed; primary={primary}; purge={purge}"
            )),
        }
    }
}

#[async_trait]
impl HighEncryptedMemoCache for RedisCache {
    async fn get_envelope(
        &self,
        owner_partition: uuid::Uuid,
        memo_id: uuid::Uuid,
    ) -> AppResult<Option<HighEncryptedMemoEnvelope>> {
        let key = Self::high_cache_key(owner_partition, memo_id);
        let envelope = match self.get::<HighEncryptedMemoEnvelope>(&key).await {
            Ok(envelope) => envelope,
            Err(error) => {
                return Err(self
                    .purge_invalid_cached_value(
                        &key,
                        "HIGH cache envelope deserialization failed",
                        error,
                    )
                    .await);
            }
        };

        if let Some(envelope) = envelope.as_ref() {
            if let Err(error) = Self::validate_cached_envelope(owner_partition, memo_id, envelope) {
                return Err(self
                    .purge_invalid_cached_value(
                        &key,
                        "HIGH cache envelope validation failed",
                        error,
                    )
                    .await);
            }
        }

        Ok(envelope)
    }

    async fn set_envelope(
        &self,
        envelope: &HighEncryptedMemoEnvelope,
        expiration: Duration,
    ) -> AppResult<()> {
        envelope.validate_structure()?;
        if expiration.as_secs() == 0 {
            return Err(AppError::ValidationError(
                "HIGH encrypted cache TTL must be at least one second".into(),
            ));
        }

        let key = Self::high_cache_key(envelope.owner_partition, envelope.memo_id);
        self.set(&key, envelope, Some(expiration)).await
    }

    async fn delete_envelope(
        &self,
        owner_partition: uuid::Uuid,
        memo_id: uuid::Uuid,
    ) -> AppResult<()> {
        let key = Self::high_cache_key(owner_partition, memo_id);
        self.delete(&key).await
    }

    async fn envelope_exists(
        &self,
        owner_partition: uuid::Uuid,
        memo_id: uuid::Uuid,
    ) -> AppResult<bool> {
        let key = Self::high_cache_key(owner_partition, memo_id);
        self.exists(&key).await
    }
}

#[async_trait]
impl MemoCache for RedisCache {
    async fn get_memo(
        &self,
        owner_partition: uuid::Uuid,
        memo_id: uuid::Uuid,
    ) -> AppResult<Option<Memo>> {
        let key = Self::legacy_cache_key(owner_partition, memo_id);
        let memo = match self.get::<Memo>(&key).await {
            Ok(memo) => memo,
            Err(error) => {
                return Err(self
                    .purge_invalid_cached_value(
                        &key,
                        "Legacy cache memo deserialization failed",
                        error,
                    )
                    .await);
            }
        };

        if let Some(memo) = memo.as_ref() {
            if let Err(error) = Self::validate_cached_memo(owner_partition, memo_id, memo) {
                return Err(self
                    .purge_invalid_cached_value(
                        &key,
                        "Legacy cache memo identity validation failed",
                        error,
                    )
                    .await);
            }
        }

        Ok(memo)
    }

    async fn set_memo(&self, memo: &Memo, expiration: Option<Duration>) -> AppResult<()> {
        self.set(
            &Self::legacy_cache_key(memo.user_id, memo.id),
            memo,
            expiration,
        )
        .await
    }

    async fn delete_memo(&self, owner_partition: uuid::Uuid, memo_id: uuid::Uuid) -> AppResult<()> {
        RedisCache::delete(self, &Self::legacy_cache_key(owner_partition, memo_id)).await
    }

    async fn memo_exists(
        &self,
        owner_partition: uuid::Uuid,
        memo_id: uuid::Uuid,
    ) -> AppResult<bool> {
        // Existence is integrity-sensitive too: a Redis key containing a memo
        // for another owner/id must never satisfy the repository fast path.
        Ok(self.get_memo(owner_partition, memo_id).await?.is_some())
    }
}

#[async_trait]
impl HealthProbe for RedisCache {
    async fn check(&self) -> bool {
        match self.health_check().await {
            Ok(healthy) => healthy,
            Err(error) => {
                log::warn!("Redis health check failed: {error}");
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::application::crypto::{MEMO_HIGH_SCHEMA_VERSION, MEMO_HIGH_SUITE_ID};

    fn envelope() -> HighEncryptedMemoEnvelope {
        HighEncryptedMemoEnvelope {
            memo_id: uuid::Uuid::new_v4(),
            owner_partition: uuid::Uuid::new_v4(),
            ciphertext: vec![0xAA; 32],
            nonce: vec![0xBB; 12],
            wrapped_dek: vec![0xCC; 48],
            version: 2,
            crypto_suite_id: MEMO_HIGH_SUITE_ID.into(),
            key_version: "test-kms-v1".into(),
            schema_version: MEMO_HIGH_SCHEMA_VERSION,
        }
    }

    #[test]
    fn legacy_cache_key_is_namespaced_and_owner_scoped() {
        let envelope = envelope();
        assert_eq!(
            RedisCache::legacy_cache_key(envelope.owner_partition, envelope.memo_id),
            format!("memo:{}:{}", envelope.owner_partition, envelope.memo_id)
        );
    }

    #[test]
    fn legacy_cached_memo_identity_must_match_requested_key() {
        let owner = uuid::Uuid::new_v4();
        let memo_id = uuid::Uuid::new_v4();
        let mut memo = Memo::new("title".into(), "content".into(), vec![], owner);
        memo.id = memo_id;

        assert!(RedisCache::validate_cached_memo(owner, memo_id, &memo).is_ok());
        assert!(RedisCache::validate_cached_memo(uuid::Uuid::new_v4(), memo_id, &memo).is_err());
        assert!(RedisCache::validate_cached_memo(owner, uuid::Uuid::new_v4(), &memo).is_err());
    }

    #[test]
    fn high_cache_key_is_namespaced_and_owner_scoped() {
        let envelope = envelope();
        let key = RedisCache::high_cache_key(envelope.owner_partition, envelope.memo_id);

        assert!(key.starts_with("memo:high:v1:"));
        assert!(key.contains(&envelope.owner_partition.to_string()));
        assert!(key.ends_with(&envelope.memo_id.to_string()));
    }

    #[test]
    fn high_cache_serialization_contains_only_envelope_fields() {
        let envelope = envelope();
        let value = serde_json::to_value(&envelope).unwrap();
        let object = value.as_object().unwrap();
        let fields = object.keys().cloned().collect::<BTreeSet<_>>();
        let expected = BTreeSet::from([
            "memo_id".to_string(),
            "owner_partition".to_string(),
            "ciphertext".to_string(),
            "nonce".to_string(),
            "wrapped_dek".to_string(),
            "version".to_string(),
            "crypto_suite_id".to_string(),
            "key_version".to_string(),
            "schema_version".to_string(),
        ]);

        assert_eq!(fields, expected);
        assert!(!object.contains_key("title"));
        assert!(!object.contains_key("content"));
        assert!(!object.contains_key("tags"));
        assert!(!object.contains_key("created_at"));
        assert!(!object.contains_key("updated_at"));
    }

    #[test]
    fn high_cache_ttl_requires_at_least_one_redis_second() {
        assert_eq!(Duration::from_secs(1).as_secs(), 1);
        assert_eq!(Duration::from_millis(999).as_secs(), 0);
    }

    #[test]
    fn high_cache_rejects_cross_owner_or_cross_memo_envelopes() {
        let envelope = envelope();

        assert!(RedisCache::validate_cached_envelope(
            envelope.owner_partition,
            envelope.memo_id,
            &envelope
        )
        .is_ok());

        assert!(RedisCache::validate_cached_envelope(
            uuid::Uuid::new_v4(),
            envelope.memo_id,
            &envelope
        )
        .is_err());

        assert!(RedisCache::validate_cached_envelope(
            envelope.owner_partition,
            uuid::Uuid::new_v4(),
            &envelope
        )
        .is_err());
    }
}
