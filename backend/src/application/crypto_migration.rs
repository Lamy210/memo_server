use std::sync::Arc;

use async_trait::async_trait;
use uuid::Uuid;

use crate::{
    application::crypto::HighEncryptedMemoEnvelope,
    domain::memo::entity::Memo,
    error::{AppError, AppResult},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncryptedMemoStageResult {
    Inserted,
    AlreadyPresent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HighMemoMigrationResult {
    InsertedAndVerified,
    AlreadyPresentVerified,
}

/// Cryptography boundary used only by non-authoritative HIGH migration staging.
///
/// Unlike the normal request-path cryptography port, this boundary may exercise
/// a structurally accepted suite before it is marked DEPLOYED. It must never be
/// wired into authoritative CRUD paths.
#[async_trait]
pub trait HighMemoStagingCryptography: Send + Sync {
    async fn encrypt_for_staging(&self, memo: &Memo) -> AppResult<HighEncryptedMemoEnvelope>;

    async fn decrypt_staged(&self, envelope: &HighEncryptedMemoEnvelope) -> AppResult<Memo>;
}

/// Persistence boundary for the isolated encrypted migration collection.
#[async_trait]
pub trait HighEncryptedMemoStagingStore: Send + Sync {
    async fn find_staged(
        &self,
        owner_partition: Uuid,
        memo_id: Uuid,
    ) -> AppResult<Option<HighEncryptedMemoEnvelope>>;

    /// Atomically insert an envelope only when the memo ID is absent.
    ///
    /// AlreadyPresent does not imply byte-for-byte envelope equality. Parallel
    /// writers may produce different valid ciphertext for identical plaintext
    /// because each encryption uses a fresh DEK and nonce. Callers must read
    /// back, decrypt, and compare authoritative plaintext before accepting it.
    async fn stage_if_absent(
        &self,
        envelope: &HighEncryptedMemoEnvelope,
    ) -> AppResult<EncryptedMemoStageResult>;

    async fn count_staged(&self) -> AppResult<u64>;
}

pub struct HighMemoMigrationService {
    cryptography: Arc<dyn HighMemoStagingCryptography>,
    staging: Arc<dyn HighEncryptedMemoStagingStore>,
}

impl HighMemoMigrationService {
    pub fn new(
        cryptography: Arc<dyn HighMemoStagingCryptography>,
        staging: Arc<dyn HighEncryptedMemoStagingStore>,
    ) -> Self {
        Self {
            cryptography,
            staging,
        }
    }

    /// Stage one plaintext authoritative memo as a verified encrypted envelope.
    ///
    /// Existing staged data is decrypted and compared before it is accepted as
    /// an idempotent rerun. This is important because HIGH encryption uses a
    /// fresh DEK and nonce, so re-encrypting identical plaintext does not
    /// produce an envelope that can be compared byte-for-byte.
    pub async fn stage_memo(&self, memo: &Memo) -> AppResult<HighMemoMigrationResult> {
        if !memo.validate() {
            return Err(AppError::ValidationError(
                "Memo violates domain invariants before HIGH migration staging".into(),
            ));
        }

        if let Some(existing) = self.staging.find_staged(memo.user_id, memo.id).await? {
            self.verify_envelope_matches_memo(memo, &existing).await?;
            return Ok(HighMemoMigrationResult::AlreadyPresentVerified);
        }

        let envelope = self.cryptography.encrypt_for_staging(memo).await?;
        validate_generated_envelope_identity(memo, &envelope)?;

        let stage_result = self.staging.stage_if_absent(&envelope).await?;

        // Always read back and decrypt the persisted representation. Besides
        // verifying storage serialization, this also handles a concurrent
        // insert race: if another writer won, the winner must decrypt to the
        // exact same logical memo or the migration fails closed.
        let persisted = self
            .staging
            .find_staged(memo.user_id, memo.id)
            .await?
            .ok_or_else(|| {
                AppError::DatabaseError(format!(
                    "Encrypted migration memo {} disappeared after staging",
                    memo.id
                ))
            })?;
        self.verify_envelope_matches_memo(memo, &persisted).await?;

        Ok(match stage_result {
            EncryptedMemoStageResult::Inserted => HighMemoMigrationResult::InsertedAndVerified,
            EncryptedMemoStageResult::AlreadyPresent => {
                HighMemoMigrationResult::AlreadyPresentVerified
            }
        })
    }

    pub async fn staged_count(&self) -> AppResult<u64> {
        self.staging.count_staged().await
    }

    async fn verify_envelope_matches_memo(
        &self,
        expected: &Memo,
        envelope: &HighEncryptedMemoEnvelope,
    ) -> AppResult<()> {
        envelope.validate_structure()?;
        if envelope.memo_id != expected.id
            || envelope.owner_partition != expected.user_id
            || envelope.version != expected.version
        {
            return Err(AppError::Conflict(format!(
                "Encrypted migration target identity/version differs from authoritative memo {}",
                expected.id
            )));
        }

        let decrypted = self.cryptography.decrypt_staged(envelope).await?;

        if memos_match_at_storage_precision(expected, &decrypted) {
            return Ok(());
        }

        Err(AppError::Conflict(format!(
            "Encrypted migration target differs from authoritative memo {}",
            expected.id
        )))
    }
}

fn validate_generated_envelope_identity(
    memo: &Memo,
    envelope: &HighEncryptedMemoEnvelope,
) -> AppResult<()> {
    envelope.validate_structure()?;

    if envelope.memo_id != memo.id
        || envelope.owner_partition != memo.user_id
        || envelope.version != memo.version
    {
        return Err(AppError::InternalServerError(
            "HIGH staging cryptography returned envelope identity that does not match the memo"
                .into(),
        ));
    }

    Ok(())
}

fn memos_match_at_storage_precision(expected: &Memo, actual: &Memo) -> bool {
    expected.id == actual.id
        && expected.user_id == actual.user_id
        && expected.title == actual.title
        && expected.content == actual.content
        && expected.tags == actual.tags
        && expected.version == actual.version
        && expected.created_at.timestamp_millis() == actual.created_at.timestamp_millis()
        && expected.updated_at.timestamp_millis() == actual.updated_at.timestamp_millis()
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Mutex,
        },
    };

    use chrono::Utc;

    use super::*;
    use crate::application::crypto::{MEMO_HIGH_SCHEMA_VERSION, MEMO_HIGH_SUITE_ID};

    struct FakeCryptography {
        encrypt_calls: AtomicUsize,
        corrupt_identity: bool,
    }

    impl FakeCryptography {
        fn new() -> Self {
            Self {
                encrypt_calls: AtomicUsize::new(0),
                corrupt_identity: false,
            }
        }

        fn corrupt_identity() -> Self {
            Self {
                encrypt_calls: AtomicUsize::new(0),
                corrupt_identity: true,
            }
        }
    }

    #[async_trait]
    impl HighMemoStagingCryptography for FakeCryptography {
        async fn encrypt_for_staging(&self, memo: &Memo) -> AppResult<HighEncryptedMemoEnvelope> {
            self.encrypt_calls.fetch_add(1, Ordering::Relaxed);
            let mut memo_id = memo.id;
            if self.corrupt_identity {
                memo_id = Uuid::new_v4();
            }

            Ok(HighEncryptedMemoEnvelope {
                memo_id,
                owner_partition: memo.user_id,
                ciphertext: serde_json::to_vec(memo).unwrap(),
                nonce: vec![0x22; 12],
                wrapped_dek: vec![0x33; 48],
                version: memo.version,
                crypto_suite_id: MEMO_HIGH_SUITE_ID.into(),
                key_version: "test-key-v1".into(),
                schema_version: MEMO_HIGH_SCHEMA_VERSION,
            })
        }

        async fn decrypt_staged(&self, envelope: &HighEncryptedMemoEnvelope) -> AppResult<Memo> {
            serde_json::from_slice(&envelope.ciphertext)
                .map_err(|error| AppError::DatabaseError(format!("fake decrypt failed: {error}")))
        }
    }

    #[derive(Default)]
    struct FakeStagingStore {
        envelopes: Mutex<HashMap<Uuid, HighEncryptedMemoEnvelope>>,
    }

    #[async_trait]
    impl HighEncryptedMemoStagingStore for FakeStagingStore {
        async fn find_staged(
            &self,
            owner_partition: Uuid,
            memo_id: Uuid,
        ) -> AppResult<Option<HighEncryptedMemoEnvelope>> {
            Ok(self
                .envelopes
                .lock()
                .unwrap()
                .get(&memo_id)
                .filter(|envelope| envelope.owner_partition == owner_partition)
                .cloned())
        }

        async fn stage_if_absent(
            &self,
            envelope: &HighEncryptedMemoEnvelope,
        ) -> AppResult<EncryptedMemoStageResult> {
            let mut envelopes = self.envelopes.lock().unwrap();
            if envelopes.contains_key(&envelope.memo_id) {
                return Ok(EncryptedMemoStageResult::AlreadyPresent);
            }

            envelopes.insert(envelope.memo_id, envelope.clone());
            Ok(EncryptedMemoStageResult::Inserted)
        }

        async fn count_staged(&self) -> AppResult<u64> {
            Ok(self.envelopes.lock().unwrap().len() as u64)
        }
    }

    fn memo() -> Memo {
        Memo {
            id: Uuid::new_v4(),
            title: "Migration title".into(),
            content: "Migration content".into(),
            tags: vec!["encrypted".into()],
            user_id: Uuid::new_v4(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            version: 3,
        }
    }

    #[tokio::test]
    async fn stages_and_verifies_new_envelope() {
        let crypto = Arc::new(FakeCryptography::new());
        let store = Arc::new(FakeStagingStore::default());
        let service = HighMemoMigrationService::new(crypto.clone(), store);

        assert_eq!(
            service.stage_memo(&memo()).await.unwrap(),
            HighMemoMigrationResult::InsertedAndVerified
        );
        assert_eq!(crypto.encrypt_calls.load(Ordering::Relaxed), 1);
        assert_eq!(service.staged_count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn identical_rerun_verifies_existing_without_reencrypting() {
        let crypto = Arc::new(FakeCryptography::new());
        let store = Arc::new(FakeStagingStore::default());
        let service = HighMemoMigrationService::new(crypto.clone(), store);
        let memo = memo();

        service.stage_memo(&memo).await.unwrap();
        assert_eq!(
            service.stage_memo(&memo).await.unwrap(),
            HighMemoMigrationResult::AlreadyPresentVerified
        );
        assert_eq!(crypto.encrypt_calls.load(Ordering::Relaxed), 1);
    }

    struct RacingStagingStore {
        winner: HighEncryptedMemoEnvelope,
        visible: Mutex<bool>,
    }

    #[async_trait]
    impl HighEncryptedMemoStagingStore for RacingStagingStore {
        async fn find_staged(
            &self,
            owner_partition: Uuid,
            memo_id: Uuid,
        ) -> AppResult<Option<HighEncryptedMemoEnvelope>> {
            if !*self.visible.lock().unwrap() {
                return Ok(None);
            }

            Ok(
                (self.winner.owner_partition == owner_partition && self.winner.memo_id == memo_id)
                    .then(|| self.winner.clone()),
            )
        }

        async fn stage_if_absent(
            &self,
            _envelope: &HighEncryptedMemoEnvelope,
        ) -> AppResult<EncryptedMemoStageResult> {
            *self.visible.lock().unwrap() = true;
            Ok(EncryptedMemoStageResult::AlreadyPresent)
        }

        async fn count_staged(&self) -> AppResult<u64> {
            Ok(u64::from(u8::from(*self.visible.lock().unwrap())))
        }
    }

    #[tokio::test]
    async fn concurrent_different_envelope_is_accepted_only_after_plaintext_verification() {
        let memo = memo();
        let mut winner = HighEncryptedMemoEnvelope {
            memo_id: memo.id,
            owner_partition: memo.user_id,
            ciphertext: serde_json::to_vec(&memo).unwrap(),
            nonce: vec![0x77; 12],
            wrapped_dek: vec![0x88; 48],
            version: memo.version,
            crypto_suite_id: MEMO_HIGH_SUITE_ID.into(),
            key_version: "test-key-v1".into(),
            schema_version: MEMO_HIGH_SCHEMA_VERSION,
        };
        winner.nonce[0] ^= 0x01;

        let crypto = Arc::new(FakeCryptography::new());
        let store = Arc::new(RacingStagingStore {
            winner,
            visible: Mutex::new(false),
        });
        let service = HighMemoMigrationService::new(crypto, store);

        assert_eq!(
            service.stage_memo(&memo).await.unwrap(),
            HighMemoMigrationResult::AlreadyPresentVerified
        );
    }

    #[tokio::test]
    async fn divergent_existing_plaintext_fails_closed() {
        let crypto = Arc::new(FakeCryptography::new());
        let store = Arc::new(FakeStagingStore::default());
        let service = HighMemoMigrationService::new(crypto, store);
        let memo = memo();

        service.stage_memo(&memo).await.unwrap();

        let mut changed = memo.clone();
        changed.title = "Changed authoritative title".into();

        assert!(matches!(
            service.stage_memo(&changed).await,
            Err(AppError::Conflict(_))
        ));
    }

    #[tokio::test]
    async fn crypto_cannot_change_envelope_identity() {
        let crypto = Arc::new(FakeCryptography::corrupt_identity());
        let store = Arc::new(FakeStagingStore::default());
        let service = HighMemoMigrationService::new(crypto, store);

        assert!(matches!(
            service.stage_memo(&memo()).await,
            Err(AppError::InternalServerError(_))
        ));
        assert_eq!(service.staged_count().await.unwrap(), 0);
    }
}
