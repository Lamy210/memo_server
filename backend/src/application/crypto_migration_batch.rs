use std::sync::Arc;

use async_trait::async_trait;
use uuid::Uuid;

use crate::{
    application::crypto_migration::{HighMemoMigrationResult, HighMemoMigrationService},
    domain::memo::entity::Memo,
    error::{AppError, AppResult},
};

const MAX_MIGRATION_PAGE_SIZE: usize = 1_000;

#[async_trait]
pub trait PlaintextMemoMigrationSource: Send + Sync {
    /// Count the currently authoritative plaintext memo rows.
    async fn count_source_memos(&self) -> AppResult<u64>;

    /// Return at most `limit` memos with IDs strictly greater than `after`,
    /// ordered by memo ID ascending.
    ///
    /// The final production migration pass still requires writes to be frozen;
    /// paging is a bounded-memory traversal contract, not a live CDC protocol.
    async fn page_source_memos(&self, after: Option<Uuid>, limit: usize) -> AppResult<Vec<Memo>>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HighMemoBatchMigrationStats {
    pub source_count: u64,
    pub staged_count: u64,
    pub migrated_visited: u64,
    pub verified_visited: u64,
    pub inserted_verified: u64,
    pub already_present_verified: u64,
}

pub struct HighMemoBatchMigrationService {
    source: Arc<dyn PlaintextMemoMigrationSource>,
    migration: Arc<HighMemoMigrationService>,
}

impl HighMemoBatchMigrationService {
    pub fn new(
        source: Arc<dyn PlaintextMemoMigrationSource>,
        migration: Arc<HighMemoMigrationService>,
    ) -> Self {
        Self { source, migration }
    }

    /// Execute a bounded-memory staging pass followed by a non-mutating
    /// verification pass over the authoritative plaintext source.
    ///
    /// This detects missing/extra staged records and source cardinality changes
    /// during the run. It does not make concurrent source writes safe: the
    /// final production pass still requires the authoritative source to be
    /// write-frozen for the entire migration and verification window.
    pub async fn migrate_all(&self, page_size: usize) -> AppResult<HighMemoBatchMigrationStats> {
        validate_page_size(page_size)?;

        let source_count_before = self.source.count_source_memos().await?;
        let (migrated_visited, inserted_verified, already_present_verified) =
            self.run_migration_pass(page_size).await?;
        let source_count_after_migration = self.source.count_source_memos().await?;

        if source_count_before != source_count_after_migration {
            return Err(AppError::Conflict(format!(
                "Authoritative memo count changed during HIGH migration staging: before={source_count_before} after={source_count_after_migration}"
            )));
        }
        if migrated_visited != source_count_after_migration {
            return Err(AppError::Conflict(format!(
                "HIGH migration paging visited {migrated_visited} memo(s) but authoritative source contains {source_count_after_migration}"
            )));
        }

        let verified_visited = self.run_verification_pass(page_size).await?;
        let source_count_after_verification = self.source.count_source_memos().await?;

        if source_count_after_migration != source_count_after_verification {
            return Err(AppError::Conflict(format!(
                "Authoritative memo count changed during HIGH migration verification: before={source_count_after_migration} after={source_count_after_verification}"
            )));
        }
        if verified_visited != source_count_after_verification {
            return Err(AppError::Conflict(format!(
                "HIGH migration verification visited {verified_visited} memo(s) but authoritative source contains {source_count_after_verification}"
            )));
        }

        let staged_count = self.migration.staged_count().await?;
        if staged_count != source_count_after_verification {
            return Err(AppError::Conflict(format!(
                "HIGH migration verification failed: authoritative source has {source_count_after_verification} memo(s) but encrypted staging has {staged_count}"
            )));
        }

        Ok(HighMemoBatchMigrationStats {
            source_count: source_count_after_verification,
            staged_count,
            migrated_visited,
            verified_visited,
            inserted_verified,
            already_present_verified,
        })
    }

    async fn run_migration_pass(&self, page_size: usize) -> AppResult<(u64, u64, u64)> {
        let mut cursor = None;
        let mut visited = 0u64;
        let mut inserted = 0u64;
        let mut already_present = 0u64;

        loop {
            let page = self.source.page_source_memos(cursor, page_size).await?;
            if page.is_empty() {
                break;
            }
            validate_page(&page, cursor, page_size)?;

            for memo in &page {
                match self.migration.stage_memo(memo).await? {
                    HighMemoMigrationResult::InsertedAndVerified => inserted += 1,
                    HighMemoMigrationResult::AlreadyPresentVerified => already_present += 1,
                }
                visited += 1;
            }

            cursor = page.last().map(|memo| memo.id);
        }

        Ok((visited, inserted, already_present))
    }

    async fn run_verification_pass(&self, page_size: usize) -> AppResult<u64> {
        let mut cursor = None;
        let mut visited = 0u64;

        loop {
            let page = self.source.page_source_memos(cursor, page_size).await?;
            if page.is_empty() {
                break;
            }
            validate_page(&page, cursor, page_size)?;

            for memo in &page {
                self.migration.verify_staged_memo(memo).await?;
                visited += 1;
            }

            cursor = page.last().map(|memo| memo.id);
        }

        Ok(visited)
    }
}

fn validate_page_size(page_size: usize) -> AppResult<()> {
    if !(1..=MAX_MIGRATION_PAGE_SIZE).contains(&page_size) {
        return Err(AppError::ValidationError(format!(
            "HIGH migration page size must be between 1 and {MAX_MIGRATION_PAGE_SIZE}"
        )));
    }
    Ok(())
}

fn validate_page(page: &[Memo], after: Option<Uuid>, limit: usize) -> AppResult<()> {
    if page.len() > limit {
        return Err(AppError::DatabaseError(format!(
            "HIGH migration source returned {} memo(s), exceeding requested page size {limit}",
            page.len()
        )));
    }

    let mut previous = after;
    for memo in page {
        if previous.is_some_and(|previous| memo.id <= previous) {
            return Err(AppError::DatabaseError(
                "HIGH migration source page is not strictly ordered by memo ID".into(),
            ));
        }
        previous = Some(memo.id);
    }

    Ok(())
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

    use chrono::{TimeZone, Utc};

    use super::*;
    use crate::{
        application::{
            crypto::{HighEncryptedMemoEnvelope, MEMO_HIGH_SCHEMA_VERSION, MEMO_HIGH_SUITE_ID},
            crypto_migration::{
                EncryptedMemoStageResult, HighEncryptedMemoStagingStore,
                HighMemoStagingCryptography,
            },
        },
        error::AppResult,
    };

    struct FakeSource {
        memos: Mutex<Vec<Memo>>,
        count_calls: AtomicUsize,
    }

    impl FakeSource {
        fn new(mut memos: Vec<Memo>) -> Self {
            memos.sort_by_key(|memo| memo.id);
            Self {
                memos: Mutex::new(memos),
                count_calls: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl PlaintextMemoMigrationSource for FakeSource {
        async fn count_source_memos(&self) -> AppResult<u64> {
            self.count_calls.fetch_add(1, Ordering::Relaxed);
            Ok(self.memos.lock().unwrap().len() as u64)
        }

        async fn page_source_memos(
            &self,
            after: Option<Uuid>,
            limit: usize,
        ) -> AppResult<Vec<Memo>> {
            Ok(self
                .memos
                .lock()
                .unwrap()
                .iter()
                .filter(|memo| after.is_none_or(|after| memo.id > after))
                .take(limit)
                .cloned()
                .collect())
        }
    }

    struct FakeCrypto;

    #[async_trait]
    impl HighMemoStagingCryptography for FakeCrypto {
        async fn encrypt_for_staging(&self, memo: &Memo) -> AppResult<HighEncryptedMemoEnvelope> {
            Ok(HighEncryptedMemoEnvelope {
                memo_id: memo.id,
                owner_partition: memo.user_id,
                ciphertext: serde_json::to_vec(memo).unwrap(),
                nonce: vec![0x10; 12],
                wrapped_dek: vec![0x20; 48],
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
    struct FakeStaging {
        envelopes: Mutex<HashMap<Uuid, HighEncryptedMemoEnvelope>>,
    }

    #[async_trait]
    impl HighEncryptedMemoStagingStore for FakeStaging {
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

    fn memo(id: u128) -> Memo {
        Memo {
            id: Uuid::from_u128(id),
            title: format!("memo-{id}"),
            content: "content".into(),
            tags: vec!["migration".into()],
            user_id: Uuid::from_u128(10_000 + id),
            created_at: Utc.timestamp_millis_opt(1_700_000_000_000).unwrap(),
            updated_at: Utc.timestamp_millis_opt(1_700_000_001_000).unwrap(),
            version: 1,
        }
    }

    fn fake_envelope(memo: &Memo) -> HighEncryptedMemoEnvelope {
        HighEncryptedMemoEnvelope {
            memo_id: memo.id,
            owner_partition: memo.user_id,
            ciphertext: serde_json::to_vec(memo).unwrap(),
            nonce: vec![0x10; 12],
            wrapped_dek: vec![0x20; 48],
            version: memo.version,
            crypto_suite_id: MEMO_HIGH_SUITE_ID.into(),
            key_version: "test-key-v1".into(),
            schema_version: MEMO_HIGH_SCHEMA_VERSION,
        }
    }

    fn service(memos: Vec<Memo>) -> HighMemoBatchMigrationService {
        let source = Arc::new(FakeSource::new(memos));
        let staging = Arc::new(FakeStaging::default());
        let migration = Arc::new(HighMemoMigrationService::new(Arc::new(FakeCrypto), staging));
        HighMemoBatchMigrationService::new(source, migration)
    }

    #[tokio::test]
    async fn migrates_multiple_pages_and_verifies_full_staging_set() {
        let service = service(vec![memo(1), memo(2), memo(3), memo(4), memo(5)]);

        let stats = service.migrate_all(2).await.unwrap();

        assert_eq!(
            stats,
            HighMemoBatchMigrationStats {
                source_count: 5,
                staged_count: 5,
                migrated_visited: 5,
                verified_visited: 5,
                inserted_verified: 5,
                already_present_verified: 0,
            }
        );
    }

    #[tokio::test]
    async fn rerun_is_verified_without_growing_staging_set() {
        let service = service(vec![memo(1), memo(2), memo(3)]);

        service.migrate_all(2).await.unwrap();
        let stats = service.migrate_all(2).await.unwrap();

        assert_eq!(stats.inserted_verified, 0);
        assert_eq!(stats.already_present_verified, 3);
        assert_eq!(stats.staged_count, 3);
    }

    #[tokio::test]
    async fn rejects_stale_target_only_envelopes() {
        let source = Arc::new(FakeSource::new(vec![memo(1), memo(2)]));
        let staging = Arc::new(FakeStaging::default());
        let stale = memo(99);
        staging
            .envelopes
            .lock()
            .unwrap()
            .insert(stale.id, fake_envelope(&stale));
        let migration = Arc::new(HighMemoMigrationService::new(Arc::new(FakeCrypto), staging));
        let service = HighMemoBatchMigrationService::new(source, migration);

        assert!(matches!(
            service.migrate_all(1).await,
            Err(AppError::Conflict(_))
        ));
    }

    struct ChangingCountSource {
        source: FakeSource,
        count_calls: AtomicUsize,
    }

    #[async_trait]
    impl PlaintextMemoMigrationSource for ChangingCountSource {
        async fn count_source_memos(&self) -> AppResult<u64> {
            let call = self.count_calls.fetch_add(1, Ordering::Relaxed);
            Ok(if call == 0 { 2 } else { 3 })
        }

        async fn page_source_memos(
            &self,
            after: Option<Uuid>,
            limit: usize,
        ) -> AppResult<Vec<Memo>> {
            self.source.page_source_memos(after, limit).await
        }
    }

    #[tokio::test]
    async fn rejects_source_cardinality_change_during_staging() {
        let source = Arc::new(ChangingCountSource {
            source: FakeSource::new(vec![memo(1), memo(2)]),
            count_calls: AtomicUsize::new(0),
        });
        let staging = Arc::new(FakeStaging::default());
        let migration = Arc::new(HighMemoMigrationService::new(Arc::new(FakeCrypto), staging));
        let service = HighMemoBatchMigrationService::new(source, migration);

        assert!(matches!(
            service.migrate_all(1).await,
            Err(AppError::Conflict(_))
        ));
    }

    #[tokio::test]
    async fn rejects_invalid_page_sizes() {
        let service = service(vec![]);

        assert!(matches!(
            service.migrate_all(0).await,
            Err(AppError::ValidationError(_))
        ));
        assert!(matches!(
            service.migrate_all(MAX_MIGRATION_PAGE_SIZE + 1).await,
            Err(AppError::ValidationError(_))
        ));
    }

    #[test]
    fn rejects_out_of_order_or_oversized_pages() {
        assert!(validate_page(&[memo(2), memo(1)], None, 2).is_err());
        assert!(validate_page(&[memo(1), memo(2)], Some(Uuid::from_u128(1)), 2).is_err());
        assert!(validate_page(&[memo(1), memo(2), memo(3)], None, 2).is_err());
    }
}
