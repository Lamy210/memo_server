use std::sync::Arc;

use crate::{
    application::{
        crypto_migration_batch::{validate_page, validate_page_size, PlaintextMemoMigrationSource},
        crypto_search_projection::HighSearchProjectionMigrationAdmin,
        crypto_search_reindex::{
            HighSearchReindexRunner, HighSearchReindexService, HighSearchReindexStats,
        },
        crypto_search_rotation::{
            HighSearchKeyCacheControl, HighSearchOfflineWindowGuard, HighSearchOfflineWindowPermit,
            HighSearchRotationReady, HighSearchRotationService,
        },
        high_memo_routing::HighMemoDataRoute,
        high_search_routing::{
            HighSearchQueryRoute, HighSearchQueryRouteReader, HighSearchQueryRouteSnapshot,
        },
    },
    config::{AppConfig, AuthoritativeBackend, HighSearchConfig, SearchBackend},
    error::{AppError, AppResult},
};

use super::{
    high_search_aws_runtime::HighSearchRuntimeHandle,
    high_search_cutover_approval::HighSearchCutoverApproval,
    high_search_maintenance_mongodb::MongoHighSearchMaintenanceGuard,
    high_search_reindex_operator::ResettingHighSearchReindexRunner,
    persistence::{
        manticore::ManticoreClient, manticore_high::HighManticoreClient,
        mongodb::MongoDbAuthoritativeStore,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HighSearchRouteChangeReport {
    pub previous: HighSearchQueryRouteSnapshot,
    pub current: HighSearchQueryRouteSnapshot,
    pub reindex: Option<HighSearchReindexStats>,
    pub legacy_rebuild: Option<LegacySearchRebuildStats>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LegacySearchRebuildStats {
    pub source_count: u64,
    pub projected_visited: u64,
    pub projection_count: u64,
}

pub fn validate_protected_high_search_cutover(
    config: &AppConfig,
    page_size: usize,
    expected_generation: i64,
    approval: &HighSearchCutoverApproval,
) -> AppResult<()> {
    validate_route_topology(config)?;
    validate_page_size(page_size)?;
    validate_expected_generation(expected_generation)?;
    approval.validate_against_config(&config.high_search)
}

pub fn validate_legacy_high_search_rollback(
    config: &AppConfig,
    page_size: usize,
    expected_generation: i64,
) -> AppResult<()> {
    validate_route_topology(config)?;
    validate_page_size(page_size)?;
    validate_expected_generation(expected_generation)
}

pub async fn inspect_high_search_query_route(
    config: &AppConfig,
) -> AppResult<HighSearchQueryRouteSnapshot> {
    validate_route_topology(config)?;
    let source =
        MongoDbAuthoritativeStore::new(&config.authoritative_uri, &config.mongodb_database).await?;
    let guard = MongoHighSearchMaintenanceGuard::new(source.database_handle()).await?;
    guard.current_query_route().await
}

pub async fn run_protected_high_search_cutover(
    config: &AppConfig,
    page_size: usize,
    expected_generation: i64,
    approval: &HighSearchCutoverApproval,
) -> AppResult<HighSearchRouteChangeReport> {
    validate_protected_high_search_cutover(config, page_size, expected_generation, approval)?;

    let runtime = HighSearchRuntimeHandle::build(&config.high_search, &config.search_uri).await?;
    let stack = runtime.stack().ok_or_else(|| {
        AppError::ServiceUnavailable(
            "HIGH search cutover could not obtain an enabled protected runtime".into(),
        )
    })?;

    let source = Arc::new(
        MongoDbAuthoritativeStore::new(&config.authoritative_uri, &config.mongodb_database).await?,
    );
    let guard: Arc<dyn HighSearchOfflineWindowGuard> =
        Arc::new(MongoHighSearchMaintenanceGuard::new(source.database_handle()).await?);
    let inspector = Arc::new(HighManticoreClient::new(&config.search_uri)?);
    let projection_admin: Arc<dyn HighSearchProjectionMigrationAdmin> = inspector.clone();

    let inner_reindex: Arc<dyn HighSearchReindexRunner> = Arc::new(HighSearchReindexService::new(
        source,
        stack.projection_service(),
        inspector,
    ));
    let reindex = Arc::new(ResettingHighSearchReindexRunner::new(
        projection_admin,
        inner_reindex,
    ));
    let cache: Arc<dyn HighSearchKeyCacheControl> = stack;
    let rotation = HighSearchRotationService::new(guard, cache, reindex);

    let required_snapshot = HighSearchQueryRouteSnapshot {
        route: HighSearchQueryRoute::Legacy,
        generation: expected_generation,
    };
    let ready = rotation
        .rotate_and_reindex_requiring_snapshot(page_size, required_snapshot)
        .await?;
    let previous = ready.current_query_route().await?;
    if previous != required_snapshot {
        return Err(AppError::ServiceUnavailable(
            "HIGH search route changed after the drained cutover preflight; maintenance barrier remains closed"
                .into(),
        ));
    }

    let (_, current, stats) =
        switch_prepared_route_fail_closed(ready, previous, HighSearchQueryRoute::Protected).await?;

    Ok(HighSearchRouteChangeReport {
        previous,
        current,
        reindex: Some(stats),
        legacy_rebuild: None,
    })
}

pub async fn run_legacy_high_search_rollback(
    config: &AppConfig,
    page_size: usize,
    expected_generation: i64,
) -> AppResult<HighSearchRouteChangeReport> {
    validate_legacy_high_search_rollback(config, page_size, expected_generation)?;

    let source = Arc::new(
        MongoDbAuthoritativeStore::new(&config.authoritative_uri, &config.mongodb_database).await?,
    );
    let guard = MongoHighSearchMaintenanceGuard::new(source.database_handle()).await?;
    let legacy_projection = Arc::new(ManticoreClient::new(&config.search_uri)?);
    let permit = guard.acquire_offline_window().await?;
    let previous = permit.current_query_route().await?;

    if previous.generation != expected_generation {
        return release_pre_switch(
            permit,
            AppError::Conflict(format!(
                "HIGH search rollback expected route generation {expected_generation}, observed {}",
                previous.generation
            )),
        )
        .await;
    }

    let memo_route = match permit.current_memo_route().await {
        Ok(route) => route,
        Err(error) => return release_pre_switch(permit, error).await,
    };
    if let Err(error) = validate_legacy_rollback_memo_route(memo_route) {
        return release_pre_switch(permit, error).await;
    }

    let source_for_rebuild: Arc<dyn PlaintextMemoMigrationSource> = source;
    let rebuild = match rebuild_legacy_search_projection(
        source_for_rebuild,
        legacy_projection,
        page_size,
        permit.as_ref(),
    )
    .await
    {
        Ok(stats) => stats,
        Err(error) => return release_pre_switch(permit, error).await,
    };

    let observed_route = match permit.current_query_route().await {
        Ok(route) => route,
        Err(error) => return release_pre_switch(permit, error).await,
    };
    if observed_route != previous {
        return release_pre_switch(
            permit,
            AppError::Conflict(format!(
                "HIGH search route changed during legacy rollback rebuild: expected {} generation {}, observed {} generation {}",
                previous.route, previous.generation, observed_route.route, observed_route.generation
            )),
        )
        .await;
    }
    let observed_memo_route = match permit.current_memo_route().await {
        Ok(route) => route,
        Err(error) => return release_pre_switch(permit, error).await,
    };
    if observed_memo_route != memo_route {
        return release_pre_switch(
            permit,
            AppError::Conflict(
                "MEMO data route changed during HIGH search legacy rollback rebuild".into(),
            ),
        )
        .await;
    }

    let current = if previous.route == HighSearchQueryRoute::Legacy {
        permit.release().await?;
        previous
    } else {
        switch_permit_route_fail_closed(permit, previous, HighSearchQueryRoute::Legacy).await?
    };

    Ok(HighSearchRouteChangeReport {
        previous,
        current,
        reindex: None,
        legacy_rebuild: Some(rebuild),
    })
}

async fn rebuild_legacy_search_projection(
    source: Arc<dyn PlaintextMemoMigrationSource>,
    projection: Arc<ManticoreClient>,
    page_size: usize,
    permit: &dyn HighSearchOfflineWindowPermit,
) -> AppResult<LegacySearchRebuildStats> {
    validate_page_size(page_size)?;
    permit.assert_still_enforced().await?;

    let source_count_before = source.count_source_memos().await?;
    projection.reset_legacy_projection().await?;
    if projection.count_legacy_documents().await? != 0 {
        return Err(AppError::Conflict(
            "legacy plaintext search projection reset did not reach zero documents".into(),
        ));
    }

    let mut cursor = None;
    let mut projected_visited = 0_u64;
    loop {
        let page = source.page_source_memos(cursor, page_size).await?;
        if page.is_empty() {
            break;
        }
        validate_page(&page, cursor, page_size)?;

        for memo in &page {
            projection.index_memo(memo).await?;
            projected_visited += 1;
        }
        cursor = page.last().map(|memo| memo.id);
    }

    let source_count_after = source.count_source_memos().await?;
    if source_count_before != source_count_after {
        return Err(AppError::Conflict(format!(
            "plaintext authoritative memo count changed during legacy search rebuild: before={source_count_before} after={source_count_after}"
        )));
    }
    if projected_visited != source_count_after {
        return Err(AppError::Conflict(format!(
            "legacy search rebuild visited {projected_visited} memo(s) but plaintext authoritative source contains {source_count_after}"
        )));
    }

    let projection_count = projection.count_legacy_documents().await?;
    if projection_count != source_count_after {
        return Err(AppError::Conflict(format!(
            "legacy search rebuild convergence failed: source={source_count_after} projection={projection_count}"
        )));
    }

    permit.assert_still_enforced().await?;
    Ok(LegacySearchRebuildStats {
        source_count: source_count_after,
        projected_visited,
        projection_count,
    })
}

fn validate_route_topology(config: &AppConfig) -> AppResult<()> {
    if !matches!(&config.high_search, HighSearchConfig::AwsKms { .. }) {
        return Err(AppError::ServiceUnavailable(
            "HIGH search route operator requires HIGH_SEARCH_MODE=aws-kms".into(),
        ));
    }
    if config.authoritative_backend != AuthoritativeBackend::MongoDb {
        return Err(AppError::ServiceUnavailable(
            "HIGH search route operator requires MongoDB authoritative storage".into(),
        ));
    }
    if config.search_backend != SearchBackend::Manticore {
        return Err(AppError::ServiceUnavailable(
            "HIGH search route operator requires Manticore Search".into(),
        ));
    }
    Ok(())
}

fn validate_legacy_rollback_memo_route(
    memo_route: crate::application::high_memo_routing::HighMemoDataRouteSnapshot,
) -> AppResult<()> {
    if memo_route.route != HighMemoDataRoute::LegacyPlaintext {
        return Err(AppError::Conflict(format!(
            "HIGH search legacy rollback requires MEMO route legacy_plaintext; observed {} generation {}",
            memo_route.route, memo_route.generation
        )));
    }
    Ok(())
}

fn validate_expected_generation(expected_generation: i64) -> AppResult<()> {
    if expected_generation < 0 || expected_generation == i64::MAX {
        return Err(AppError::ValidationError(
            "HIGH search expected route generation must be non-negative and leave room to advance"
                .into(),
        ));
    }
    Ok(())
}

async fn abort_pre_switch<T>(ready: HighSearchRotationReady, primary: AppError) -> AppResult<T> {
    match ready.abort().await {
        Ok(()) => Err(primary),
        Err(abort) => Err(AppError::ServiceUnavailable(format!(
            "HIGH search cutover preflight failed and abort cleanup also failed; primary={primary}; abort={abort}"
        ))),
    }
}

async fn release_pre_switch<T>(
    permit: Box<dyn HighSearchOfflineWindowPermit>,
    primary: AppError,
) -> AppResult<T> {
    match permit.release().await {
        Ok(()) => Err(primary),
        Err(release) => Err(AppError::ServiceUnavailable(format!(
            "HIGH search route preflight failed and maintenance release also failed; primary={primary}; release={release}"
        ))),
    }
}

async fn switch_prepared_route_fail_closed(
    ready: HighSearchRotationReady,
    expected: HighSearchQueryRouteSnapshot,
    target: HighSearchQueryRoute,
) -> AppResult<(
    HighSearchQueryRouteSnapshot,
    HighSearchQueryRouteSnapshot,
    HighSearchReindexStats,
)> {
    let target_snapshot = expected_target_snapshot(expected, target)?;
    let switch_result = ready.switch_query_route(expected, target).await;
    let observed_result = ready.current_query_route().await;

    match (switch_result, observed_result) {
        (Ok(switched), Ok(observed))
            if switched == target_snapshot && observed == target_snapshot =>
        {
            let stats = ready.finish_after_cutover().await?;
            Ok((expected, observed, stats))
        }
        (Err(_), Ok(observed)) if observed == target_snapshot => {
            let stats = ready.finish_after_cutover().await?;
            Ok((expected, observed, stats))
        }
        (Err(primary), Ok(observed)) if observed == expected => {
            abort_pre_switch(
                ready,
                AppError::ServiceUnavailable(format!(
                    "HIGH search route switch did not commit; primary={primary}"
                )),
            )
            .await
        }
        (switch_result, observed_result) => Err(ambiguous_switch_error(
            switch_result.err(),
            observed_result,
            expected,
            target_snapshot,
        )),
    }
}

async fn switch_permit_route_fail_closed(
    permit: Box<dyn HighSearchOfflineWindowPermit>,
    expected: HighSearchQueryRouteSnapshot,
    target: HighSearchQueryRoute,
) -> AppResult<HighSearchQueryRouteSnapshot> {
    let target_snapshot = expected_target_snapshot(expected, target)?;
    let switch_result = permit.switch_query_route(expected, target).await;
    let observed_result = permit.current_query_route().await;

    match (switch_result, observed_result) {
        (Ok(switched), Ok(observed))
            if switched == target_snapshot && observed == target_snapshot =>
        {
            permit.release().await?;
            Ok(observed)
        }
        (Err(_), Ok(observed)) if observed == target_snapshot => {
            permit.release().await?;
            Ok(observed)
        }
        (switch_result, observed_result) => Err(ambiguous_switch_error(
            switch_result.err(),
            observed_result,
            expected,
            target_snapshot,
        )),
    }
}

fn expected_target_snapshot(
    expected: HighSearchQueryRouteSnapshot,
    target: HighSearchQueryRoute,
) -> AppResult<HighSearchQueryRouteSnapshot> {
    if target == expected.route {
        return Ok(expected);
    }
    let generation = expected
        .generation
        .checked_add(1)
        .ok_or_else(|| AppError::Conflict("HIGH search query route generation overflow".into()))?;
    Ok(HighSearchQueryRouteSnapshot {
        route: target,
        generation,
    })
}

fn ambiguous_switch_error(
    switch_error: Option<AppError>,
    observed_result: AppResult<HighSearchQueryRouteSnapshot>,
    expected: HighSearchQueryRouteSnapshot,
    target: HighSearchQueryRouteSnapshot,
) -> AppError {
    let switch = switch_error
        .map(|error| error.to_string())
        .unwrap_or_else(|| "switch returned an unexpected snapshot".into());
    let observed = observed_result
        .map(|snapshot| format!("{} generation {}", snapshot.route, snapshot.generation))
        .unwrap_or_else(|error| format!("unavailable ({error})"));

    AppError::ServiceUnavailable(format!(
        "HIGH search route switch outcome is ambiguous; maintenance barrier remains closed; expected={} generation {}; target={} generation {}; switch={switch}; observed={observed}",
        expected.route, expected.generation, target.route, target.generation
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AuthConfig, HighMemoCryptoConfig, HighSearchShadowConfig};

    fn disabled_config() -> AppConfig {
        AppConfig {
            authoritative_backend: AuthoritativeBackend::MongoDb,
            authoritative_uri: "not-a-mongodb-uri".into(),
            mongodb_database: "memo_app".into(),
            redis_uri: "redis://unused".into(),
            search_backend: SearchBackend::Manticore,
            search_uri: "not-a-manticore-uri".into(),
            high_memo_crypto: HighMemoCryptoConfig::Disabled,
            high_search: HighSearchConfig::Disabled,
            high_search_shadow: HighSearchShadowConfig::Disabled,
            port: 8080,
            auth: AuthConfig::Development,
        }
    }

    #[test]
    fn route_operator_rejects_disabled_runtime_before_network_access() {
        assert!(matches!(
            validate_legacy_high_search_rollback(&disabled_config(), 100, 0),
            Err(AppError::ServiceUnavailable(_))
        ));
    }

    #[test]
    fn legacy_rollback_requires_plaintext_authoritative_memo_route() {
        assert!(validate_legacy_rollback_memo_route(
            crate::application::high_memo_routing::HighMemoDataRouteSnapshot {
                route: HighMemoDataRoute::LegacyPlaintext,
                generation: 4,
            }
        )
        .is_ok());

        assert!(matches!(
            validate_legacy_rollback_memo_route(
                crate::application::high_memo_routing::HighMemoDataRouteSnapshot {
                    route: HighMemoDataRoute::Encrypted,
                    generation: 5,
                }
            ),
            Err(AppError::Conflict(_))
        ));
    }

    #[test]
    fn legacy_rollback_rejects_invalid_rebuild_page_size_before_network_access() {
        let mut config = disabled_config();
        config.high_search = HighSearchConfig::AwsKms {
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
        };

        assert!(matches!(
            validate_legacy_high_search_rollback(&config, 0, 0),
            Err(AppError::ValidationError(_))
        ));
    }

    #[test]
    fn target_snapshot_advances_once_and_rejects_overflow() {
        let legacy = HighSearchQueryRouteSnapshot {
            route: HighSearchQueryRoute::Legacy,
            generation: 7,
        };
        assert_eq!(
            expected_target_snapshot(legacy, HighSearchQueryRoute::Protected).unwrap(),
            HighSearchQueryRouteSnapshot {
                route: HighSearchQueryRoute::Protected,
                generation: 8,
            }
        );
        assert_eq!(
            expected_target_snapshot(legacy, HighSearchQueryRoute::Legacy).unwrap(),
            legacy
        );
        assert!(expected_target_snapshot(
            HighSearchQueryRouteSnapshot {
                route: HighSearchQueryRoute::Legacy,
                generation: i64::MAX,
            },
            HighSearchQueryRoute::Protected,
        )
        .is_err());
    }
}
