use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use uuid::Uuid;

use crate::{
    application::{
        crypto::HighMemoCryptography,
        crypto_cache::HighEncryptedMemoCache,
    },
    domain::memo::entity::Memo,
    error::{AppError, AppResult},
    infrastructure::persistence::ports::MemoCache,
};

/// Domain-facing cache adapter that guarantees Redis/Valkey receives only
/// encrypted HIGH memo envelopes.
///
/// This adapter is intentionally independent from request-path startup wiring.
/// A deployment can stage and test it before switching the application cache
/// mode away from the legacy plaintext namespace.
pub(crate) struct HighMemoCiphertextCacheAdapter {
    encrypted_cache: Arc<dyn HighEncryptedMemoCache>,
    cryptography: Arc<dyn HighMemoCryptography>,
}

impl HighMemoCiphertextCacheAdapter {
    pub(crate) fn new(
        encrypted_cache: Arc<dyn HighEncryptedMemoCache>,
        cryptography: Arc<dyn HighMemoCryptography>,
    ) -> Self {
        Self {
            encrypted_cache,
            cryptography,
        }
    }

    fn validate_expiration(expiration: Option<Duration>) -> AppResult<Duration> {
        let expiration = expiration.ok_or_else(|| {
            AppError::ValidationError(
                "HIGH memo ciphertext cache requires an explicit finite TTL".into(),
            )
        })?;

        if expiration.is_zero() {
            return Err(AppError::ValidationError(
                "HIGH memo ciphertext cache TTL must be positive".into(),
            ));
        }

        Ok(expiration)
    }

    async fn purge_invalid_entry(
        &self,
        owner_partition: Uuid,
        memo_id: Uuid,
        primary: AppError,
    ) -> AppError {
        match self
            .encrypted_cache
            .delete_envelope(owner_partition, memo_id)
            .await
        {
            Ok(()) => primary,
            Err(purge) => AppError::DatabaseError(format!(
                "HIGH memo cache entry validation failed and invalid-envelope purge also failed; primary={primary}; purge={purge}"
            )),
        }
    }
}

#[async_trait]
impl MemoCache for HighMemoCiphertextCacheAdapter {
    async fn get_memo(&self, owner_partition: Uuid, memo_id: Uuid) -> AppResult<Option<Memo>> {
        let Some(envelope) = self
            .encrypted_cache
            .get_envelope(owner_partition, memo_id)
            .await?
        else {
            return Ok(None);
        };

        let memo = match self.cryptography.decrypt_memo(&envelope).await {
            Ok(memo) => memo,
            Err(error) => {
                return Err(
                    self.purge_invalid_entry(owner_partition, memo_id, error)
                        .await,
                );
            }
        };

        if memo.user_id != owner_partition || memo.id != memo_id {
            let error = AppError::DatabaseError(
                "HIGH memo cache decrypted identity does not match requested owner/memo".into(),
            );
            return Err(
                self.purge_invalid_entry(owner_partition, memo_id, error)
                    .await,
            );
        }

        Ok(Some(memo))
    }

    async fn set_memo(&self, memo: &Memo, expiration: Option<Duration>) -> AppResult<()> {
        let expiration = Self::validate_expiration(expiration)?;
        let envelope = self.cryptography.encrypt_memo(memo).await?;

        if envelope.owner_partition != memo.user_id || envelope.memo_id != memo.id {
            return Err(AppError::InternalServerError(
                "HIGH memo encryption returned an envelope for a different memo identity".into(),
            ));
        }

        self.encrypted_cache
            .set_envelope(&envelope, Some(expiration))
            .await
    }

    async fn delete_memo(&self, owner_partition: Uuid, memo_id: Uuid) -> AppResult<()> {
        self.encrypted_cache
            .delete_envelope(owner_partition, memo_id)
            .await
    }

    async fn memo_exists(&self, owner_partition: Uuid, memo_id: Uuid) -> AppResult<bool> {
        self.encrypted_cache
            .envelope_exists(owner_partition, memo_id)
            .await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use chrono::{TimeZone, Utc};

    use super::*;
    use crate::application::crypto::{
        HighEncryptedMemoEnvelope, MEMO_HIGH_SCHEMA_VERSION, MEMO_HIGH_SUITE_ID,
    };

    #[derive(Default)]
    struct FakeEncryptedCache {
        envelope: Mutex<Option<HighEncryptedMemoEnvelope>>,
        deletes: Mutex<Vec<(Uuid, Uuid)>>,
        expirations: Mutex<Vec<Option<Duration>>>,
    }

    #[async_trait]
    impl HighEncryptedMemoCache for FakeEncryptedCache {
        async fn get_envelope(
            &self,
            _owner_partition: Uuid,
            _memo_id: Uuid,
        ) -> AppResult<Option<HighEncryptedMemoEnvelope>> {
            Ok(self.envelope.lock().unwrap().clone())
        }

        async fn set_envelope(
            &self,
            envelope: &HighEncryptedMemoEnvelope,
            expiration: Option<Duration>,
        ) -> AppResult<()> {
            *self.envelope.lock().unwrap() = Some(envelope.clone());
            self.expirations.lock().unwrap().push(expiration);
            Ok(())
        }

        async fn delete_envelope(
            &self,
            owner_partition: Uuid,
            memo_id: Uuid,
        ) -> AppResult<()> {
            self.deletes
                .lock()
                .unwrap()
                .push((owner_partition, memo_id));
            *self.envelope.lock().unwrap() = None;
            Ok(())
        }

        async fn envelope_exists(
            &self,
            _owner_partition: Uuid,
            _memo_id: Uuid,
        ) -> AppResult<bool> {
            Ok(self.envelope.lock().unwrap().is_some())
        }
    }

    struct FakeCrypto {
        decrypt_error: bool,
        wrong_identity: bool,
    }

    #[async_trait]
    impl HighMemoCryptography for FakeCrypto {
        async fn encrypt_memo(&self, memo: &Memo) -> AppResult<HighEncryptedMemoEnvelope> {
            Ok(HighEncryptedMemoEnvelope {
                memo_id: memo.id,
                owner_partition: memo.user_id,
                ciphertext: vec![0xAA; 32],
                nonce: vec![0xBB; 12],
                wrapped_dek: vec![0xCC; 48],
                version: memo.version,
                crypto_suite_id: MEMO_HIGH_SUITE_ID.into(),
                key_version: "memo-key-v1".into(),
                schema_version: MEMO_HIGH_SCHEMA_VERSION,
            })
        }

        async fn decrypt_memo(&self, envelope: &HighEncryptedMemoEnvelope) -> AppResult<Memo> {
            if self.decrypt_error {
                return Err(AppError::DatabaseError("tampered cache entry".into()));
            }

            Ok(Memo {
                id: if self.wrong_identity {
                    Uuid::new_v4()
                } else {
                    envelope.memo_id
                },
                title: "title".into(),
                content: "content".into(),
                tags: vec!["tag".into()],
                user_id: envelope.owner_partition,
                created_at: Utc.timestamp_millis_opt(1_700_000_000_000).unwrap(),
                updated_at: Utc.timestamp_millis_opt(1_700_000_001_000).unwrap(),
                version: envelope.version,
            })
        }
    }

    fn memo() -> Memo {
        Memo {
            id: Uuid::new_v4(),
            title: "title".into(),
            content: "content".into(),
            tags: vec!["tag".into()],
            user_id: Uuid::new_v4(),
            created_at: Utc.timestamp_millis_opt(1_700_000_000_000).unwrap(),
            updated_at: Utc.timestamp_millis_opt(1_700_000_001_000).unwrap(),
            version: 2,
        }
    }

    fn adapter(
        cache: Arc<FakeEncryptedCache>,
        decrypt_error: bool,
        wrong_identity: bool,
    ) -> HighMemoCiphertextCacheAdapter {
        HighMemoCiphertextCacheAdapter::new(
            cache,
            Arc::new(FakeCrypto {
                decrypt_error,
                wrong_identity,
            }),
        )
    }

    #[tokio::test]
    async fn stores_only_envelopes_with_explicit_positive_ttl() {
        let cache = Arc::new(FakeEncryptedCache::default());
        let adapter = adapter(cache.clone(), false, false);
        let memo = memo();

        adapter
            .set_memo(&memo, Some(Duration::from_secs(3600)))
            .await
            .unwrap();

        let stored = cache.envelope.lock().unwrap().clone().unwrap();
        assert_eq!(stored.memo_id, memo.id);
        assert_eq!(stored.owner_partition, memo.user_id);
        assert_eq!(
            cache.expirations.lock().unwrap().as_slice(),
            &[Some(Duration::from_secs(3600))]
        );

        assert!(adapter.set_memo(&memo, None).await.is_err());
        assert!(adapter
            .set_memo(&memo, Some(Duration::ZERO))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn decrypts_valid_cache_hits() {
        let cache = Arc::new(FakeEncryptedCache::default());
        let adapter = adapter(cache.clone(), false, false);
        let memo = memo();
        adapter
            .set_memo(&memo, Some(Duration::from_secs(60)))
            .await
            .unwrap();

        let restored = adapter
            .get_memo(memo.user_id, memo.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(restored.id, memo.id);
        assert_eq!(restored.user_id, memo.user_id);
        assert!(cache.deletes.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn invalid_ciphertext_is_purged_and_returned_as_cache_failure() {
        let cache = Arc::new(FakeEncryptedCache::default());
        let memo = memo();
        *cache.envelope.lock().unwrap() = Some(HighEncryptedMemoEnvelope {
            memo_id: memo.id,
            owner_partition: memo.user_id,
            ciphertext: vec![0xAA; 32],
            nonce: vec![0xBB; 12],
            wrapped_dek: vec![0xCC; 48],
            version: memo.version,
            crypto_suite_id: MEMO_HIGH_SUITE_ID.into(),
            key_version: "memo-key-v1".into(),
            schema_version: MEMO_HIGH_SCHEMA_VERSION,
        });
        let adapter = adapter(cache.clone(), true, false);

        assert!(adapter.get_memo(memo.user_id, memo.id).await.is_err());
        assert_eq!(
            cache.deletes.lock().unwrap().as_slice(),
            &[(memo.user_id, memo.id)]
        );
        assert!(cache.envelope.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn decrypted_identity_mismatch_is_purged() {
        let cache = Arc::new(FakeEncryptedCache::default());
        let memo = memo();
        let good = adapter(cache.clone(), false, false);
        good.set_memo(&memo, Some(Duration::from_secs(60)))
            .await
            .unwrap();

        let wrong = adapter(cache.clone(), false, true);
        assert!(wrong.get_memo(memo.user_id, memo.id).await.is_err());
        assert_eq!(
            cache.deletes.lock().unwrap().as_slice(),
            &[(memo.user_id, memo.id)]
        );
    }
}
