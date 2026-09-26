use std::sync::Arc;

use crate::{
    application::{
        crypto_migration_batch::validate_page_size,
        crypto_search_reindex::{HighSearchReindexService, HighSearchReindexStats},
        crypto_search_rotation::{
            HighSearchKeyCacheControl, HighSearchOfflineWindowGuard, HighSearchRotationService,
        },
    },
    config::{AppConfig, AuthoritativeBackend, HighSearchConfig, SearchBackend},
    error::{AppError, AppResult},
};

use super::{
    high_search_aws_runtime::HighSearchRuntimeHandle,
    high_search_maintenance_mongodb::MongoHighSearchMaintenanceGuard,
    persistence::{manticore_high::HighManticoreClient, mongodb::MongoDbAuthoritativeStore},
};

/// Run the staged SEARCH-HIGH-1 protected-projection reindex and convergence
/// verification without installing the protected search request path.
///
/// This is an operator-only staging boundary. The caller must ensure every
/// application replica that can mutate memos participates in the shared
/// MongoDB maintenance barrier. The current protected request path must remain
/// inactive for the whole operation; therefore the rotation "cutover" is a
/// deliberate no-op and only releases the verified maintenance permit.
pub fn validate_staged_high_search_reindex(
    config: &AppConfig,
    page_size: usize,
) -> AppResult<()> {
    validate_page_size(page_size)?;

    if !matches!(&config.high_search, HighSearchConfig::AwsKms { .. }) {
        return Err(AppError::ServiceUnavailable(
            "staged HIGH search reindex requires HIGH_SEARCH_MODE=aws-kms".into(),
        ));
    }
    if config.authoritative_backend != AuthoritativeBackend::MongoDb {
        return Err(AppError::ServiceUnavailable(
            "staged HIGH search reindex requires MongoDB authoritative storage".into(),
        ));
    }
    if config.search_backend != SearchBackend::Manticore {
        return Err(AppError::ServiceUnavailable(
            "staged HIGH search reindex requires Manticore Search".into(),
        ));
    }

    Ok(())
}

pub async fn run_staged_high_search_reindex(
    config: &AppConfig,
    page_size: usize,
) -> AppResult<HighSearchReindexStats> {
    // Reject invalid operator input and impossible staged topologies before any
    // provider or database network I/O.
    validate_staged_high_search_reindex(config, page_size)?;

    let runtime = HighSearchRuntimeHandle::build(&config.high_search, &config.search_uri).await?;
    let stack = runtime.stack().ok_or_else(|| {
        AppError::ServiceUnavailable(
            "staged HIGH search reindex could not obtain an enabled runtime".into(),
        )
    })?;

    let source = Arc::new(
        MongoDbAuthoritativeStore::new(&config.authoritative_uri, &config.mongodb_database).await?,
    );
    let guard: Arc<dyn HighSearchOfflineWindowGuard> = Arc::new(
        MongoHighSearchMaintenanceGuard::new(source.database_handle()).await?,
    );
    let inspector = Arc::new(HighManticoreClient::new(&config.search_uri)?);

    let reindex = Arc::new(HighSearchReindexService::new(
        source,
        stack.projection_service(),
        inspector,
    ));
    let cache: Arc<dyn HighSearchKeyCacheControl> = stack;
    let rotation = HighSearchRotationService::new(guard, cache, reindex);

    let ready = rotation.rotate_and_reindex(page_size).await?;

    // SEARCH-HIGH-1 is intentionally not installed in request handling yet.
    // There is no routing generation to switch here; finishing only revalidates
    // and releases the maintenance permit after the verified staging pass.
    ready.finish_after_cutover().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AuthConfig, HighSearchConfig};

    fn disabled_config() -> AppConfig {
        AppConfig {
            authoritative_backend: AuthoritativeBackend::MongoDb,
            authoritative_uri: "not-a-mongodb-uri".into(),
            mongodb_database: "memo_app".into(),
            redis_uri: "redis://unused".into(),
            search_backend: SearchBackend::Manticore,
            search_uri: "not-a-manticore-uri".into(),
            high_search: HighSearchConfig::Disabled,
            port: 8080,
            auth: AuthConfig::Development,
        }
    }

    #[test]
    fn disabled_high_search_fails_static_validation() {
        assert!(matches!(
            validate_staged_high_search_reindex(&disabled_config(), 100),
            Err(AppError::ServiceUnavailable(_))
        ));
    }

    #[test]
    fn invalid_page_size_fails_before_runtime_or_network_access() {
        assert!(matches!(
            validate_staged_high_search_reindex(&disabled_config(), 0),
            Err(AppError::ValidationError(_))
        ));
    }
}
