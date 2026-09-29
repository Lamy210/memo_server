use std::sync::Arc;

use crate::{
    application::{
        crypto_migration::{
            HighEncryptedMemoStagingAdmin, HighEncryptedMemoStagingStore,
            HighMemoStagingCryptography,
        },
        crypto_migration_batch::{
            validate_page_size, HighMemoBatchMigrationStats, PlaintextMemoMigrationSource,
        },
        crypto_search_rotation::{HighSearchOfflineWindowGuard, HighSearchOfflineWindowPermit},
        high_memo_routing::{HighMemoDataRoute, HighMemoDataRouteSnapshot},
        high_search_routing::{HighSearchQueryRoute, HighSearchQueryRouteSnapshot},
    },
    config::{
        AppConfig, AuthoritativeBackend, HighMemoCryptoConfig, HighSearchConfig, SearchBackend,
    },
    error::{AppError, AppResult},
};

use super::{
    high_memo_aws_runtime::HighMemoStagingRuntimeHandle,
    high_memo_migration_operator::run_under_permit as run_migration_under_permit,
    high_search_maintenance_mongodb::MongoHighSearchMaintenanceGuard,
    persistence::{
        manticore::ManticoreClient,
        mongodb::MongoDbAuthoritativeStore,
        ports::{
            HighMemoCacheSweepStats, HighMemoCiphertextCacheMaintenance, LegacyMemoCacheSweepStats,
            LegacyMemoPlaintextCacheMaintenance,
        },
        redis::RedisCache,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HighMemoRouteCutoverApproval {
    pub all_replicas_encrypted_ready: bool,
    pub plaintext_backup_verified: bool,
    pub no_automatic_rollback_accepted: bool,
}

impl HighMemoRouteCutoverApproval {
    fn validate(self) -> AppResult<()> {
        if !self.all_replicas_encrypted_ready {
            return Err(AppError::ValidationError(
                "MEMO-HIGH-1 cutover requires all replicas to have the encrypted standby repository and maintenance participation"
                    .into(),
            ));
        }
        if !self.plaintext_backup_verified {
            return Err(AppError::ValidationError(
                "MEMO-HIGH-1 cutover requires a verified plaintext authoritative backup".into(),
            ));
        }
        if !self.no_automatic_rollback_accepted {
            return Err(AppError::ValidationError(
                "MEMO-HIGH-1 cutover requires explicit acceptance that encrypted writes have no automatic plaintext rollback"
                    .into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HighMemoRouteCutoverReport {
    pub previous_memo_route: HighMemoDataRouteSnapshot,
    pub current_memo_route: HighMemoDataRouteSnapshot,
    pub search_route: HighSearchQueryRouteSnapshot,
    pub migration: Option<HighMemoBatchMigrationStats>,
    pub legacy_cache_purge: Option<LegacyMemoCacheSweepStats>,
    pub encrypted_cache_purge: Option<HighMemoCacheSweepStats>,
    pub legacy_search_purged: bool,
}

pub fn validate_high_memo_cutover_config(
    config: &AppConfig,
    page_size: usize,
    cache_scan_count: usize,
    expected_memo_generation: i64,
    expected_search_generation: i64,
    approval: HighMemoRouteCutoverApproval,
) -> AppResult<()> {
    approval.validate()?;
    validate_page_size(page_size)?;
    validate_cache_scan_count(cache_scan_count)?;
    validate_generation("memo", expected_memo_generation)?;
    validate_generation("search", expected_search_generation)?;

    if config.authoritative_backend != AuthoritativeBackend::MongoDb {
        return Err(AppError::ServiceUnavailable(
            "MEMO-HIGH-1 cutover requires MongoDB authoritative storage".into(),
        ));
    }
    if config.search_backend != SearchBackend::Manticore {
        return Err(AppError::ServiceUnavailable(
            "MEMO-HIGH-1 cutover requires Manticore Search".into(),
        ));
    }
    if !matches!(config.high_memo_crypto, HighMemoCryptoConfig::AwsKms { .. }) {
        return Err(AppError::ServiceUnavailable(
            "MEMO-HIGH-1 cutover requires HIGH_MEMO_CRYPTO_MODE=aws-kms".into(),
        ));
    }
    if !matches!(config.high_search, HighSearchConfig::AwsKms { .. }) {
        return Err(AppError::ServiceUnavailable(
            "MEMO-HIGH-1 cutover requires protected HIGH search runtime".into(),
        ));
    }

    Ok(())
}

pub async fn inspect_high_memo_cutover_routes(
    config: &AppConfig,
) -> AppResult<(HighMemoDataRouteSnapshot, HighSearchQueryRouteSnapshot)> {
    validate_topology_only(config)?;

    let source =
        MongoDbAuthoritativeStore::new(&config.authoritative_uri, &config.mongodb_database).await?;
    let guard = MongoHighSearchMaintenanceGuard::new(source.database_handle()).await?;

    use crate::application::{
        high_memo_routing::HighMemoDataRouteReader, high_search_routing::HighSearchQueryRouteReader,
    };

    let memo = guard.current_memo_data_route().await?;
    let search = guard.current_query_route().await?;
    Ok((memo, search))
}

pub async fn run_encrypted_high_memo_cutover(
    config: &AppConfig,
    page_size: usize,
    cache_scan_count: usize,
    expected_memo_generation: i64,
    expected_search_generation: i64,
    approval: HighMemoRouteCutoverApproval,
) -> AppResult<HighMemoRouteCutoverReport> {
    validate_high_memo_cutover_config(
        config,
        page_size,
        cache_scan_count,
        expected_memo_generation,
        expected_search_generation,
        approval,
    )?;

    // Complete all KMS/key-ring preflight before traffic is frozen.
    let runtime = HighMemoStagingRuntimeHandle::build(&config.high_memo_crypto).await?;
    let cryptography = runtime.cryptography().ok_or_else(|| {
        AppError::ServiceUnavailable(
            "MEMO-HIGH-1 cutover could not obtain the configured cryptography runtime".into(),
        )
    })?;

    let source = Arc::new(
        MongoDbAuthoritativeStore::new(&config.authoritative_uri, &config.mongodb_database).await?,
    );
    let guard = MongoHighSearchMaintenanceGuard::new(source.database_handle()).await?;
    let legacy_cache = Arc::new(RedisCache::new(&config.redis_uri)?);
    let legacy_search = Arc::new(ManticoreClient::new(&config.search_uri)?);
    let permit = guard.acquire_offline_window().await?;

    let expected_search = HighSearchQueryRouteSnapshot {
        route: HighSearchQueryRoute::Protected,
        generation: expected_search_generation,
    };
    let observed_search = match permit.current_query_route().await {
        Ok(route) if route == expected_search => route,
        Ok(route) => {
            return release_pre_switch(
                permit,
                AppError::Conflict(format!(
                    "MEMO-HIGH-1 cutover requires protected search route generation {expected_search_generation}; observed {} generation {}",
                    route.route, route.generation
                )),
            )
            .await;
        }
        Err(error) => return release_pre_switch(permit, error).await,
    };

    let previous_memo_route = match permit.current_memo_route().await {
        Ok(route) if route.generation == expected_memo_generation => route,
        Ok(route) => {
            return release_pre_switch(
                permit,
                AppError::Conflict(format!(
                    "MEMO-HIGH-1 cutover expected memo route generation {expected_memo_generation}; observed {} generation {}",
                    route.route, route.generation
                )),
            )
            .await;
        }
        Err(error) => return release_pre_switch(permit, error).await,
    };

    if previous_memo_route.route == HighMemoDataRoute::Encrypted {
        let (legacy_cache_purge, encrypted_cache_purge) =
            match purge_cutover_cache_and_legacy_search(
                legacy_cache.as_ref(),
                legacy_search.as_ref(),
                cache_scan_count,
            )
            .await
            {
                Ok(stats) => stats,
                Err(error) => return release_pre_switch(permit, error).await,
            };

        let revalidated_search = permit.current_query_route().await.map_err(|error| {
            fail_closed_after_destructive_pre_switch("search route revalidation failed", error)
        })?;
        let revalidated_memo = permit.current_memo_route().await.map_err(|error| {
            fail_closed_after_destructive_pre_switch("memo route revalidation failed", error)
        })?;
        if revalidated_search != expected_search || revalidated_memo != previous_memo_route {
            return Err(AppError::ServiceUnavailable(
                "MEMO-HIGH-1 idempotent cleanup observed a route change; maintenance barrier remains closed"
                    .into(),
            ));
        }

        permit.release().await?;
        return Ok(HighMemoRouteCutoverReport {
            previous_memo_route,
            current_memo_route: previous_memo_route,
            search_route: observed_search,
            migration: None,
            legacy_cache_purge: Some(legacy_cache_purge),
            encrypted_cache_purge: Some(encrypted_cache_purge),
            legacy_search_purged: true,
        });
    }

    if previous_memo_route.route != HighMemoDataRoute::LegacyPlaintext {
        return release_pre_switch(
            permit,
            AppError::Conflict(format!(
                "MEMO-HIGH-1 cutover requires memo route legacy_plaintext; observed {}",
                previous_memo_route.route
            )),
        )
        .await;
    }

    if let Err(error) = ensure_no_pending_projection_intents(source.as_ref()).await {
        return release_pre_switch(permit, error).await;
    }

    let source_for_migration: Arc<dyn PlaintextMemoMigrationSource> = source.clone();
    let staging: Arc<dyn HighEncryptedMemoStagingStore> = source.clone();
    let staging_admin: Arc<dyn HighEncryptedMemoStagingAdmin> = source.clone();
    let cryptography: Arc<dyn HighMemoStagingCryptography> = cryptography;
    let migration = match run_migration_under_permit(
        source_for_migration,
        staging,
        staging_admin,
        cryptography,
        page_size,
        permit.as_ref(),
    )
    .await
    {
        Ok(stats) => stats,
        Err(error) => return release_pre_switch(permit, error).await,
    };

    if let Err(error) = ensure_no_pending_projection_intents(source.as_ref()).await {
        return release_pre_switch(permit, error).await;
    }

    let (legacy_cache_purge, encrypted_cache_purge) = match purge_cutover_cache_and_legacy_search(
        legacy_cache.as_ref(),
        legacy_search.as_ref(),
        cache_scan_count,
    )
    .await
    {
        Ok(stats) => stats,
        Err(error) => return release_pre_switch(permit, error).await,
    };

    if let Err(error) = permit.assert_still_enforced().await {
        return Err(fail_closed_after_destructive_pre_switch(
            "maintenance permit validation failed",
            error,
        ));
    }

    let revalidated_search = permit.current_query_route().await.map_err(|error| {
        fail_closed_after_destructive_pre_switch("search route revalidation failed", error)
    })?;
    if revalidated_search != expected_search {
        return Err(AppError::ServiceUnavailable(format!(
            "search route changed after plaintext retirement preparation; maintenance barrier remains closed; expected {} generation {}, observed {} generation {}",
            expected_search.route,
            expected_search.generation,
            revalidated_search.route,
            revalidated_search.generation
        )));
    }

    let revalidated_memo = permit.current_memo_route().await.map_err(|error| {
        fail_closed_after_destructive_pre_switch("memo route revalidation failed", error)
    })?;
    if revalidated_memo != previous_memo_route {
        return Err(AppError::ServiceUnavailable(format!(
            "memo route changed after plaintext retirement preparation; maintenance barrier remains closed; expected {} generation {}, observed {} generation {}",
            previous_memo_route.route,
            previous_memo_route.generation,
            revalidated_memo.route,
            revalidated_memo.generation
        )));
    }

    let current_memo_route = switch_memo_route_fail_closed(
        permit,
        previous_memo_route,
        expected_search,
        HighMemoDataRoute::Encrypted,
    )
    .await?;

    Ok(HighMemoRouteCutoverReport {
        previous_memo_route,
        current_memo_route,
        search_route: expected_search,
        migration: Some(migration),
        legacy_cache_purge: Some(legacy_cache_purge),
        encrypted_cache_purge: Some(encrypted_cache_purge),
        legacy_search_purged: true,
    })
}

fn validate_topology_only(config: &AppConfig) -> AppResult<()> {
    if config.authoritative_backend != AuthoritativeBackend::MongoDb {
        return Err(AppError::ServiceUnavailable(
            "MEMO-HIGH-1 cutover inspection requires MongoDB authoritative storage".into(),
        ));
    }
    Ok(())
}

fn validate_cache_scan_count(scan_count: usize) -> AppResult<()> {
    if !(1..=10_000).contains(&scan_count) {
        return Err(AppError::ValidationError(
            "legacy memo cache scan_count must be in 1..=10000".into(),
        ));
    }
    Ok(())
}

fn validate_generation(label: &str, generation: i64) -> AppResult<()> {
    if generation < 0 || generation == i64::MAX {
        return Err(AppError::ValidationError(format!(
            "HIGH {label} route generation must be non-negative and leave room to advance"
        )));
    }
    Ok(())
}

async fn ensure_no_pending_projection_intents(source: &MongoDbAuthoritativeStore) -> AppResult<()> {
    let pending = source.count_projection_intents_for_cutover().await?;
    if pending == 0 {
        Ok(())
    } else {
        Err(AppError::Conflict(format!(
            "MEMO-HIGH-1 cutover requires projection outbox convergence; {pending} intent(s) remain pending"
        )))
    }
}

async fn purge_cutover_cache_and_legacy_search(
    cache: &RedisCache,
    legacy_search: &ManticoreClient,
    cache_scan_count: usize,
) -> AppResult<(LegacyMemoCacheSweepStats, HighMemoCacheSweepStats)> {
    // Purge the encrypted namespace too. Although it contains ciphertext only,
    // a stale standby/test envelope could otherwise become a stale fast-path
    // value immediately after the encrypted route activates.
    let encrypted_purge = cache
        .purge_high_encrypted_memo_cache(cache_scan_count)
        .await?;
    let encrypted_after = cache
        .inspect_high_encrypted_memo_cache(cache_scan_count)
        .await?;
    if encrypted_after.encrypted_keys != 0 {
        return Err(AppError::Conflict(format!(
            "HIGH encrypted memo cache purge left {} memo key(s)",
            encrypted_after.encrypted_keys
        )));
    }

    let legacy_purge = cache
        .purge_legacy_plaintext_memo_cache(cache_scan_count)
        .await?;
    let legacy_after = cache
        .inspect_legacy_plaintext_memo_cache(cache_scan_count)
        .await?;
    if legacy_after.legacy_keys != 0 {
        return Err(AppError::Conflict(format!(
            "legacy plaintext memo cache purge left {} memo key(s)",
            legacy_after.legacy_keys
        )));
    }

    legacy_search.reset_legacy_projection().await?;
    let remaining = legacy_search.count_legacy_documents().await?;
    if remaining != 0 {
        return Err(AppError::Conflict(format!(
            "legacy plaintext search purge left {remaining} document(s)"
        )));
    }

    Ok((legacy_purge, encrypted_purge))
}

fn expected_memo_target(
    expected: HighMemoDataRouteSnapshot,
    target: HighMemoDataRoute,
) -> AppResult<HighMemoDataRouteSnapshot> {
    if target == expected.route {
        return Ok(expected);
    }

    let generation = expected
        .generation
        .checked_add(1)
        .ok_or_else(|| AppError::Conflict("HIGH memo route generation overflow".into()))?;
    Ok(HighMemoDataRouteSnapshot {
        route: target,
        generation,
    })
}

async fn switch_memo_route_fail_closed(
    permit: Box<dyn HighSearchOfflineWindowPermit>,
    expected: HighMemoDataRouteSnapshot,
    required_search: HighSearchQueryRouteSnapshot,
    target: HighMemoDataRoute,
) -> AppResult<HighMemoDataRouteSnapshot> {
    let target_snapshot = expected_memo_target(expected, target)?;
    let switch_result = permit.switch_memo_route(expected, target).await;
    let memo_observed = permit.current_memo_route().await;
    let search_observed = permit.current_query_route().await;

    match (switch_result, memo_observed, search_observed) {
        (Ok(switched), Ok(memo), Ok(search))
            if switched == target_snapshot
                && memo == target_snapshot
                && search == required_search =>
        {
            permit.release().await?;
            Ok(memo)
        }
        (Err(_), Ok(memo), Ok(search))
            if memo == target_snapshot && search == required_search =>
        {
            permit.release().await?;
            Ok(memo)
        }
        (Err(primary), Ok(memo), Ok(search))
            if memo == expected && search == required_search =>
        {
            match permit.release().await {
                Ok(()) => Err(AppError::ServiceUnavailable(format!(
                    "MEMO-HIGH-1 route switch did not commit; primary={primary}"
                ))),
                Err(release) => Err(AppError::ServiceUnavailable(format!(
                    "MEMO-HIGH-1 route switch did not commit and maintenance release failed; primary={primary}; release={release}"
                ))),
            }
        }
        (switch_result, memo_observed, search_observed) => Err(AppError::ServiceUnavailable(
            format!(
                "MEMO-HIGH-1 route switch outcome is ambiguous; maintenance barrier remains closed; expected memo={} generation {}; target memo={} generation {}; switch={}; observed_memo={}; observed_search={}",
                expected.route,
                expected.generation,
                target_snapshot.route,
                target_snapshot.generation,
                switch_result
                    .err()
                    .map(|error| error.to_string())
                    .unwrap_or_else(|| "unexpected successful snapshot".into()),
                format_memo_observation(memo_observed),
                format_search_observation(search_observed),
            ),
        )),
    }
}

async fn release_pre_switch<T>(
    permit: Box<dyn HighSearchOfflineWindowPermit>,
    primary: AppError,
) -> AppResult<T> {
    match permit.release().await {
        Ok(()) => Err(primary),
        Err(release) => Err(AppError::ServiceUnavailable(format!(
            "MEMO-HIGH-1 cutover preflight failed and maintenance release also failed; primary={primary}; release={release}"
        ))),
    }
}

fn fail_closed_after_destructive_pre_switch(context: &str, primary: AppError) -> AppError {
    AppError::ServiceUnavailable(format!(
        "MEMO-HIGH-1 {context} after plaintext cache/search purge; maintenance barrier remains closed; primary={primary}"
    ))
}

fn format_memo_observation(result: AppResult<HighMemoDataRouteSnapshot>) -> String {
    result
        .map(|snapshot| format!("{} generation {}", snapshot.route, snapshot.generation))
        .unwrap_or_else(|error| format!("unavailable ({error})"))
}

fn format_search_observation(result: AppResult<HighSearchQueryRouteSnapshot>) -> String {
    result
        .map(|snapshot| format!("{} generation {}", snapshot.route, snapshot.generation))
        .unwrap_or_else(|error| format!("unavailable ({error})"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AuthConfig, HighSearchShadowConfig};
    use async_trait::async_trait;
    use std::{
        collections::BTreeMap,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
    };

    #[derive(Debug, Clone, Copy)]
    enum FakeSwitchOutcome {
        Ok(HighMemoDataRouteSnapshot),
        Err,
    }

    struct FakeOfflinePermit {
        memo_observed: HighMemoDataRouteSnapshot,
        search_observed: HighSearchQueryRouteSnapshot,
        switch_outcome: FakeSwitchOutcome,
        released: Arc<AtomicBool>,
    }

    #[async_trait]
    impl HighSearchOfflineWindowPermit for FakeOfflinePermit {
        async fn assert_still_enforced(&self) -> AppResult<()> {
            Ok(())
        }

        async fn current_query_route(&self) -> AppResult<HighSearchQueryRouteSnapshot> {
            Ok(self.search_observed)
        }

        async fn switch_query_route(
            &self,
            _expected: HighSearchQueryRouteSnapshot,
            _target: HighSearchQueryRoute,
        ) -> AppResult<HighSearchQueryRouteSnapshot> {
            Err(AppError::InternalServerError(
                "unexpected query-route switch in memo cutover test".into(),
            ))
        }

        async fn current_memo_route(&self) -> AppResult<HighMemoDataRouteSnapshot> {
            Ok(self.memo_observed)
        }

        async fn switch_memo_route(
            &self,
            _expected: HighMemoDataRouteSnapshot,
            _target: HighMemoDataRoute,
        ) -> AppResult<HighMemoDataRouteSnapshot> {
            match self.switch_outcome {
                FakeSwitchOutcome::Ok(snapshot) => Ok(snapshot),
                FakeSwitchOutcome::Err => Err(AppError::ServiceUnavailable(
                    "simulated memo route switch failure".into(),
                )),
            }
        }

        async fn current_plaintext_retirement_state(
            &self,
        ) -> AppResult<crate::application::high_memo_routing::HighMemoPlaintextRetirementState> {
            Ok(
                crate::application::high_memo_routing::HighMemoPlaintextRetirementState::Available,
            )
        }

        async fn begin_plaintext_retirement(
            &self,
        ) -> AppResult<crate::application::high_memo_routing::HighMemoPlaintextRetirementState> {
            Ok(
                crate::application::high_memo_routing::HighMemoPlaintextRetirementState::InProgress,
            )
        }

        async fn finish_plaintext_retirement(
            &self,
        ) -> AppResult<crate::application::high_memo_routing::HighMemoPlaintextRetirementState> {
            Ok(crate::application::high_memo_routing::HighMemoPlaintextRetirementState::Retired)
        }

        async fn release(self: Box<Self>) -> AppResult<()> {
            self.released.store(true, Ordering::Relaxed);
            Ok(())
        }
    }

    fn approved() -> HighMemoRouteCutoverApproval {
        HighMemoRouteCutoverApproval {
            all_replicas_encrypted_ready: true,
            plaintext_backup_verified: true,
            no_automatic_rollback_accepted: true,
        }
    }

    fn configured() -> AppConfig {
        AppConfig {
            authoritative_backend: AuthoritativeBackend::MongoDb,
            authoritative_uri: "not-a-mongodb-uri".into(),
            mongodb_database: "memo_app".into(),
            redis_uri: "redis://unused".into(),
            search_backend: SearchBackend::Manticore,
            search_uri: "not-a-manticore-uri".into(),
            high_memo_crypto: HighMemoCryptoConfig::AwsKms {
                region: "ap-northeast-1".into(),
                active_key_version: "memo-key-v1".into(),
                key_versions: BTreeMap::from([(
                    "memo-key-v1".into(),
                    "arn:aws:kms:ap-northeast-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab"
                        .into(),
                )]),
            },
            high_search: HighSearchConfig::AwsKms {
                key_arn:
                    "arn:aws:kms:ap-northeast-1:111122223333:key/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
                        .into(),
                region: "ap-northeast-1".into(),
                provider_seed_version: "seed-v1".into(),
                cache_ttl_seconds: 60,
                cache_max_entries: 64,
                cache_sweep_seconds: 30,
                max_document_content_terms: 128,
                max_query_content_terms: 32,
                max_normalized_term_bytes: 128,
            },
            high_search_shadow: HighSearchShadowConfig::Disabled,
            port: 8080,
            auth: AuthConfig::Development,
        }
    }

    fn route_snapshots() -> (
        HighMemoDataRouteSnapshot,
        HighMemoDataRouteSnapshot,
        HighSearchQueryRouteSnapshot,
    ) {
        (
            HighMemoDataRouteSnapshot {
                route: HighMemoDataRoute::LegacyPlaintext,
                generation: 4,
            },
            HighMemoDataRouteSnapshot {
                route: HighMemoDataRoute::Encrypted,
                generation: 5,
            },
            HighSearchQueryRouteSnapshot {
                route: HighSearchQueryRoute::Protected,
                generation: 8,
            },
        )
    }

    #[tokio::test]
    async fn memo_route_switch_releases_only_after_exact_target_is_observed() {
        let (expected, target, search) = route_snapshots();
        let released = Arc::new(AtomicBool::new(false));
        let permit: Box<dyn HighSearchOfflineWindowPermit> = Box::new(FakeOfflinePermit {
            memo_observed: target,
            search_observed: search,
            switch_outcome: FakeSwitchOutcome::Ok(target),
            released: released.clone(),
        });

        assert_eq!(
            switch_memo_route_fail_closed(permit, expected, search, HighMemoDataRoute::Encrypted,)
                .await
                .unwrap(),
            target
        );
        assert!(released.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn switch_error_is_accepted_only_when_exact_target_is_persisted() {
        let (expected, target, search) = route_snapshots();
        let released = Arc::new(AtomicBool::new(false));
        let permit: Box<dyn HighSearchOfflineWindowPermit> = Box::new(FakeOfflinePermit {
            memo_observed: target,
            search_observed: search,
            switch_outcome: FakeSwitchOutcome::Err,
            released: released.clone(),
        });

        assert_eq!(
            switch_memo_route_fail_closed(permit, expected, search, HighMemoDataRoute::Encrypted,)
                .await
                .unwrap(),
            target
        );
        assert!(released.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn proven_uncommitted_switch_releases_maintenance_and_returns_failure() {
        let (expected, _target, search) = route_snapshots();
        let released = Arc::new(AtomicBool::new(false));
        let permit: Box<dyn HighSearchOfflineWindowPermit> = Box::new(FakeOfflinePermit {
            memo_observed: expected,
            search_observed: search,
            switch_outcome: FakeSwitchOutcome::Err,
            released: released.clone(),
        });

        assert!(switch_memo_route_fail_closed(
            permit,
            expected,
            search,
            HighMemoDataRoute::Encrypted,
        )
        .await
        .is_err());
        assert!(released.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn ambiguous_switch_outcome_keeps_maintenance_fail_closed() {
        let (expected, target, search) = route_snapshots();
        let released = Arc::new(AtomicBool::new(false));
        let changed_search = HighSearchQueryRouteSnapshot {
            route: HighSearchQueryRoute::Protected,
            generation: search.generation + 1,
        };
        let permit: Box<dyn HighSearchOfflineWindowPermit> = Box::new(FakeOfflinePermit {
            memo_observed: target,
            search_observed: changed_search,
            switch_outcome: FakeSwitchOutcome::Ok(target),
            released: released.clone(),
        });

        assert!(switch_memo_route_fail_closed(
            permit,
            expected,
            search,
            HighMemoDataRoute::Encrypted,
        )
        .await
        .is_err());
        assert!(!released.load(Ordering::Relaxed));
    }

    #[test]
    fn cutover_rejects_invalid_budgets_before_network_access() {
        assert!(
            validate_high_memo_cutover_config(&configured(), 0, 100, 0, 0, approved()).is_err()
        );
        assert!(
            validate_high_memo_cutover_config(&configured(), 100, 0, 0, 0, approved()).is_err()
        );
        assert!(
            validate_high_memo_cutover_config(&configured(), 100, 100, -1, 0, approved()).is_err()
        );
        assert!(
            validate_high_memo_cutover_config(&configured(), 100, 100, 0, -1, approved()).is_err()
        );
    }

    #[test]
    fn cutover_requires_every_operator_attestation() {
        let mut approval = approved();
        approval.all_replicas_encrypted_ready = false;
        assert!(
            validate_high_memo_cutover_config(&configured(), 100, 100, 0, 0, approval).is_err()
        );

        let mut approval = approved();
        approval.plaintext_backup_verified = false;
        assert!(
            validate_high_memo_cutover_config(&configured(), 100, 100, 0, 0, approval).is_err()
        );

        let mut approval = approved();
        approval.no_automatic_rollback_accepted = false;
        assert!(
            validate_high_memo_cutover_config(&configured(), 100, 100, 0, 0, approval).is_err()
        );
    }

    #[test]
    fn memo_target_generation_advances_once() {
        let legacy = HighMemoDataRouteSnapshot {
            route: HighMemoDataRoute::LegacyPlaintext,
            generation: 4,
        };
        assert_eq!(
            expected_memo_target(legacy, HighMemoDataRoute::Encrypted).unwrap(),
            HighMemoDataRouteSnapshot {
                route: HighMemoDataRoute::Encrypted,
                generation: 5,
            }
        );
        assert_eq!(
            expected_memo_target(legacy, HighMemoDataRoute::LegacyPlaintext).unwrap(),
            legacy
        );
    }
}
