use std::sync::Arc;

use crate::{
    application::{
        crypto_migration::{
            HighEncryptedMemoStagingAdmin, HighEncryptedMemoStagingStore, HighMemoMigrationService,
        },
        crypto_migration_batch::{
            validate_page_size, HighMemoBatchMigrationService, HighMemoBatchMigrationStats,
            PlaintextMemoMigrationSource,
        },
        crypto_search_rotation::{HighSearchOfflineWindowGuard, HighSearchOfflineWindowPermit},
        high_memo_routing::{HighMemoAuthoritativeRoute, HighMemoAuthoritativeRouteSnapshot},
    },
    config::{AppConfig, AuthoritativeBackend, HighMemoCryptoConfig},
    error::{AppError, AppResult},
};

use super::{
    high_memo_aws_runtime::HighMemoStagingRuntimeHandle,
    high_search_maintenance_mongodb::MongoHighSearchMaintenanceGuard,
    persistence::mongodb::MongoDbAuthoritativeStore,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HighMemoMigrationPlan {
    pub source_count: u64,
    pub staged_count: u64,
}

pub fn validate_high_memo_migration_config(config: &AppConfig, page_size: usize) -> AppResult<()> {
    validate_page_size(page_size)?;

    if config.authoritative_backend != AuthoritativeBackend::MongoDb {
        return Err(AppError::ServiceUnavailable(
            "MEMO-HIGH-1 migration requires MongoDB authoritative storage".into(),
        ));
    }
    if !matches!(config.high_memo_crypto, HighMemoCryptoConfig::AwsKms { .. }) {
        return Err(AppError::ServiceUnavailable(
            "MEMO-HIGH-1 migration requires HIGH_MEMO_CRYPTO_MODE=aws-kms".into(),
        ));
    }

    Ok(())
}

pub async fn plan_high_memo_migration(config: &AppConfig) -> AppResult<HighMemoMigrationPlan> {
    validate_topology_only(config)?;

    let source =
        MongoDbAuthoritativeStore::new(&config.authoritative_uri, &config.mongodb_database).await?;
    Ok(HighMemoMigrationPlan {
        source_count: source.count_source_memos().await?,
        staged_count: source.count_staged().await?,
    })
}

pub async fn run_high_memo_migration(
    config: &AppConfig,
    page_size: usize,
) -> AppResult<HighMemoBatchMigrationStats> {
    validate_high_memo_migration_config(config, page_size)?;

    // KMS/AWS preflight is intentionally completed before user traffic is
    // frozen. No migration data is mutated while this runtime is constructed.
    let runtime = HighMemoStagingRuntimeHandle::build(&config.high_memo_crypto).await?;
    let cryptography = runtime.cryptography().ok_or_else(|| {
        AppError::ServiceUnavailable(
            "MEMO-HIGH-1 staging runtime unexpectedly has no cryptography".into(),
        )
    })?;

    let source = Arc::new(
        MongoDbAuthoritativeStore::new(&config.authoritative_uri, &config.mongodb_database).await?,
    );
    let guard = MongoHighSearchMaintenanceGuard::new(source.database_handle()).await?;
    let permit = guard.acquire_offline_window().await?;

    let result = run_under_permit(
        source.clone(),
        source.clone(),
        source.clone(),
        cryptography,
        page_size,
        permit.as_ref(),
    )
    .await;

    finish_guarded_migration(result, permit).await
}

async fn run_under_permit(
    source: Arc<dyn PlaintextMemoMigrationSource>,
    staging: Arc<dyn HighEncryptedMemoStagingStore>,
    staging_admin: Arc<dyn HighEncryptedMemoStagingAdmin>,
    cryptography: Arc<dyn crate::application::crypto_migration::HighMemoStagingCryptography>,
    page_size: usize,
    permit: &dyn HighSearchOfflineWindowPermit,
) -> AppResult<HighMemoBatchMigrationStats> {
    permit.assert_still_enforced().await?;
    let memo_route = permit.current_memo_route().await?;
    if memo_route.route != HighMemoAuthoritativeRoute::Plaintext {
        return Err(AppError::Conflict(format!(
            "MEMO-HIGH-1 staging reset requires plaintext authoritative routing; observed {} generation {}",
            memo_route.route, memo_route.generation
        )));
    }

    // The target collection is reset only while plaintext storage remains
    // authoritative. Once the shared route is encrypted this destructive
    // staging operation is permanently blocked by the same maintenance state
    // used for future cutover.
    staging_admin.reset_staging().await?;

    permit.assert_still_enforced().await?;
    let migration = Arc::new(HighMemoMigrationService::new(cryptography, staging));
    let batch = HighMemoBatchMigrationService::new(source, migration);
    let stats = batch.migrate_all(page_size).await?;

    permit.assert_still_enforced().await?;
    Ok(stats)
}

async fn finish_guarded_migration(
    result: AppResult<HighMemoBatchMigrationStats>,
    permit: Box<dyn HighSearchOfflineWindowPermit>,
) -> AppResult<HighMemoBatchMigrationStats> {
    match (result, permit.release().await) {
        (Ok(stats), Ok(())) => Ok(stats),
        (Err(primary), Ok(())) => Err(primary),
        (Ok(_), Err(release)) => Err(release),
        (Err(primary), Err(release)) => Err(AppError::ServiceUnavailable(format!(
            "MEMO-HIGH-1 migration failed and maintenance release also failed; primary={primary}; release={release}"
        ))),
    }
}

fn validate_topology_only(config: &AppConfig) -> AppResult<()> {
    if config.authoritative_backend != AuthoritativeBackend::MongoDb {
        return Err(AppError::ServiceUnavailable(
            "MEMO-HIGH-1 migration requires MongoDB authoritative storage".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    };

    use async_trait::async_trait;
    use chrono::{TimeZone, Utc};
    use uuid::Uuid;

    use super::*;
    use crate::{
        application::{
            crypto::{HighEncryptedMemoEnvelope, MEMO_HIGH_SCHEMA_VERSION, MEMO_HIGH_SUITE_ID},
            crypto_migration::{EncryptedMemoStageResult, HighMemoStagingCryptography},
            high_search_routing::{HighSearchQueryRoute, HighSearchQueryRouteSnapshot},
        },
        config::{AuthConfig, HighSearchConfig, HighSearchShadowConfig, SearchBackend},
        domain::memo::entity::Memo,
    };

    struct FakePermit {
        checks: AtomicUsize,
        releases: Arc<AtomicUsize>,
        fail_release: bool,
    }

    #[async_trait]
    impl HighSearchOfflineWindowPermit for FakePermit {
        async fn assert_still_enforced(&self) -> AppResult<()> {
            self.checks.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }

        async fn current_query_route(&self) -> AppResult<HighSearchQueryRouteSnapshot> {
            Ok(HighSearchQueryRouteSnapshot {
                route: HighSearchQueryRoute::Legacy,
                generation: 0,
            })
        }

        async fn switch_query_route(
            &self,
            _expected: HighSearchQueryRouteSnapshot,
            _target: HighSearchQueryRoute,
        ) -> AppResult<HighSearchQueryRouteSnapshot> {
            Err(AppError::Conflict("not used by memo migration".into()))
        }

        async fn current_memo_route(&self) -> AppResult<HighMemoAuthoritativeRouteSnapshot> {
            Ok(HighMemoAuthoritativeRouteSnapshot {
                route: HighMemoAuthoritativeRoute::Plaintext,
                generation: 0,
            })
        }

        async fn switch_memo_route(
            &self,
            _expected: HighMemoAuthoritativeRouteSnapshot,
            _target: HighMemoAuthoritativeRoute,
        ) -> AppResult<HighMemoAuthoritativeRouteSnapshot> {
            Err(AppError::Conflict("not used by memo migration".into()))
        }

        async fn release(self: Box<Self>) -> AppResult<()> {
            self.releases.fetch_add(1, Ordering::Relaxed);
            if self.fail_release {
                Err(AppError::ServiceUnavailable("release failed".into()))
            } else {
                Ok(())
            }
        }
    }

    #[derive(Default)]
    struct FakeStore {
        source: Mutex<Vec<Memo>>,
        staged: Mutex<Vec<HighEncryptedMemoEnvelope>>,
        resets: AtomicUsize,
    }

    #[async_trait]
    impl PlaintextMemoMigrationSource for FakeStore {
        async fn count_source_memos(&self) -> AppResult<u64> {
            Ok(self.source.lock().unwrap().len() as u64)
        }

        async fn page_source_memos(
            &self,
            after: Option<Uuid>,
            limit: usize,
        ) -> AppResult<Vec<Memo>> {
            let mut memos = self.source.lock().unwrap().clone();
            memos.sort_by_key(|memo| memo.id);
            Ok(memos
                .into_iter()
                .filter(|memo| after.is_none_or(|after| memo.id > after))
                .take(limit)
                .collect())
        }
    }

    #[async_trait]
    impl HighEncryptedMemoStagingAdmin for FakeStore {
        async fn reset_staging(&self) -> AppResult<()> {
            self.resets.fetch_add(1, Ordering::Relaxed);
            self.staged.lock().unwrap().clear();
            Ok(())
        }
    }

    #[async_trait]
    impl HighEncryptedMemoStagingStore for FakeStore {
        async fn find_staged(
            &self,
            owner_partition: Uuid,
            memo_id: Uuid,
        ) -> AppResult<Option<HighEncryptedMemoEnvelope>> {
            Ok(self
                .staged
                .lock()
                .unwrap()
                .iter()
                .find(|envelope| {
                    envelope.owner_partition == owner_partition && envelope.memo_id == memo_id
                })
                .cloned())
        }

        async fn stage_if_absent(
            &self,
            envelope: &HighEncryptedMemoEnvelope,
        ) -> AppResult<EncryptedMemoStageResult> {
            let mut staged = self.staged.lock().unwrap();
            if staged.iter().any(|value| value.memo_id == envelope.memo_id) {
                return Ok(EncryptedMemoStageResult::AlreadyPresent);
            }
            staged.push(envelope.clone());
            Ok(EncryptedMemoStageResult::Inserted)
        }

        async fn count_staged(&self) -> AppResult<u64> {
            Ok(self.staged.lock().unwrap().len() as u64)
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
                nonce: vec![0x22; 12],
                wrapped_dek: vec![0x33; 48],
                version: memo.version,
                crypto_suite_id: MEMO_HIGH_SUITE_ID.into(),
                key_version: "memo-key-v1".into(),
                schema_version: MEMO_HIGH_SCHEMA_VERSION,
            })
        }

        async fn decrypt_staged(&self, envelope: &HighEncryptedMemoEnvelope) -> AppResult<Memo> {
            serde_json::from_slice(&envelope.ciphertext)
                .map_err(|error| AppError::DatabaseError(format!("fake decrypt failed: {error}")))
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

    #[tokio::test]
    async fn frozen_full_rebuild_removes_stale_staging_and_verifies_all_source_rows() {
        let store = Arc::new(FakeStore {
            source: Mutex::new(vec![memo(1), memo(2)]),
            staged: Mutex::new(vec![HighEncryptedMemoEnvelope {
                memo_id: Uuid::from_u128(99),
                owner_partition: Uuid::from_u128(10_099),
                ciphertext: serde_json::to_vec(&memo(99)).unwrap(),
                nonce: vec![0x44; 12],
                wrapped_dek: vec![0x55; 48],
                version: 1,
                crypto_suite_id: MEMO_HIGH_SUITE_ID.into(),
                key_version: "memo-key-v0".into(),
                schema_version: MEMO_HIGH_SCHEMA_VERSION,
            }]),
            resets: AtomicUsize::new(0),
        });
        let releases = Arc::new(AtomicUsize::new(0));
        let permit = FakePermit {
            checks: AtomicUsize::new(0),
            releases,
            fail_release: false,
        };

        let stats = run_under_permit(
            store.clone(),
            store.clone(),
            store.clone(),
            Arc::new(FakeCrypto),
            1,
            &permit,
        )
        .await
        .unwrap();

        assert_eq!(store.resets.load(Ordering::Relaxed), 1);
        assert_eq!(stats.source_count, 2);
        assert_eq!(stats.staged_count, 2);
        assert_eq!(stats.inserted_verified, 2);
        assert!(store
            .staged
            .lock()
            .unwrap()
            .iter()
            .all(|envelope| envelope.memo_id != Uuid::from_u128(99)));
        assert_eq!(permit.checks.load(Ordering::Relaxed), 3);
    }

    struct EncryptedRoutePermit {
        checks: AtomicUsize,
    }

    #[async_trait]
    impl HighSearchOfflineWindowPermit for EncryptedRoutePermit {
        async fn assert_still_enforced(&self) -> AppResult<()> {
            self.checks.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }

        async fn current_query_route(&self) -> AppResult<HighSearchQueryRouteSnapshot> {
            Ok(HighSearchQueryRouteSnapshot {
                route: HighSearchQueryRoute::Legacy,
                generation: 0,
            })
        }

        async fn switch_query_route(
            &self,
            _expected: HighSearchQueryRouteSnapshot,
            _target: HighSearchQueryRoute,
        ) -> AppResult<HighSearchQueryRouteSnapshot> {
            Err(AppError::Conflict("not used".into()))
        }

        async fn current_memo_route(&self) -> AppResult<HighMemoAuthoritativeRouteSnapshot> {
            Ok(HighMemoAuthoritativeRouteSnapshot {
                route: HighMemoAuthoritativeRoute::Encrypted,
                generation: 3,
            })
        }

        async fn switch_memo_route(
            &self,
            _expected: HighMemoAuthoritativeRouteSnapshot,
            _target: HighMemoAuthoritativeRoute,
        ) -> AppResult<HighMemoAuthoritativeRouteSnapshot> {
            Err(AppError::Conflict("not used".into()))
        }

        async fn release(self: Box<Self>) -> AppResult<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn encrypted_authoritative_route_blocks_destructive_staging_reset() {
        let store = Arc::new(FakeStore {
            source: Mutex::new(vec![memo(1)]),
            staged: Mutex::new(Vec::new()),
            resets: AtomicUsize::new(0),
        });
        let permit = EncryptedRoutePermit {
            checks: AtomicUsize::new(0),
        };

        assert!(matches!(
            run_under_permit(
                store.clone(),
                store.clone(),
                store.clone(),
                Arc::new(FakeCrypto),
                100,
                &permit,
            )
            .await,
            Err(AppError::Conflict(_))
        ));
        assert_eq!(store.resets.load(Ordering::Relaxed), 0);
        assert_eq!(permit.checks.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn migration_error_still_releases_maintenance_permit() {
        let releases = Arc::new(AtomicUsize::new(0));
        let permit: Box<dyn HighSearchOfflineWindowPermit> = Box::new(FakePermit {
            checks: AtomicUsize::new(0),
            releases: releases.clone(),
            fail_release: false,
        });

        let error = AppError::Conflict("migration failed".into());
        assert!(matches!(
            finish_guarded_migration(Err(error), permit).await,
            Err(AppError::Conflict(_))
        ));
        assert_eq!(releases.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn disabled_or_non_mongodb_config_is_rejected_before_network_access() {
        let config = AppConfig {
            authoritative_backend: AuthoritativeBackend::Scylla,
            authoritative_uri: "unused".into(),
            mongodb_database: "memo".into(),
            redis_uri: "unused".into(),
            search_backend: SearchBackend::Elasticsearch,
            search_uri: "unused".into(),
            high_memo_crypto: HighMemoCryptoConfig::Disabled,
            high_search: HighSearchConfig::Disabled,
            high_search_shadow: HighSearchShadowConfig::Disabled,
            port: 8080,
            auth: AuthConfig::Development,
        };

        assert!(validate_high_memo_migration_config(&config, 100).is_err());
    }
}
