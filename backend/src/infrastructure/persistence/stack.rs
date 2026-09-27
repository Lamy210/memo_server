use std::sync::Arc;

use crate::{
    application::{
        health::HealthProbe,
        maintenance::{
            HighSearchQueryGuard, MemoMutationGuard, UnrestrictedHighSearchQueryGuard,
            UnrestrictedMemoMutationGuard,
        },
    },
    config::{
        AppConfig, AuthoritativeBackend, HighMemoCryptoConfig, HighSearchConfig, SearchBackend,
    },
    error::{AppError, AppResult},
    infrastructure::high_search_maintenance_mongodb::MongoHighSearchMaintenanceGuard,
};

use super::{
    elasticsearch::ElasticsearchClient,
    manticore::ManticoreClient,
    mongodb::MongoDbAuthoritativeStore,
    ports::{MemoAuthoritativeStore, MemoCache, MemoSearchProjection},
    redis::RedisCache,
    scylla::ScyllaDB,
};

pub(crate) struct PersistenceStack {
    pub(crate) authoritative_store: Arc<dyn MemoAuthoritativeStore>,
    pub(crate) cache: Arc<dyn MemoCache>,
    pub(crate) search_projection: Arc<dyn MemoSearchProjection>,
    pub(crate) authoritative_health: Arc<dyn HealthProbe>,
    pub(crate) cache_health: Arc<dyn HealthProbe>,
    pub(crate) search_health: Arc<dyn HealthProbe>,
    pub(crate) mutation_guard: Arc<dyn MemoMutationGuard>,
    pub(crate) high_search_query_guard: Arc<dyn HighSearchQueryGuard>,
}

fn maintenance_participation(
    high_memo_crypto: &HighMemoCryptoConfig,
    high_search: &HighSearchConfig,
) -> (bool, bool) {
    let high_memo_enabled = matches!(high_memo_crypto, HighMemoCryptoConfig::AwsKms { .. });
    let high_search_enabled = matches!(high_search, HighSearchConfig::AwsKms { .. });

    // MEMO-HIGH-1 migration only needs to freeze memo mutations. Protected
    // search reads need query leases only when SEARCH-HIGH-1 itself is enabled.
    (
        high_memo_enabled || high_search_enabled,
        high_search_enabled,
    )
}

impl PersistenceStack {
    pub(crate) async fn build(config: &AppConfig) -> AppResult<Self> {
        let mut mongodb_database = None;
        let (authoritative_store, authoritative_health): (
            Arc<dyn MemoAuthoritativeStore>,
            Arc<dyn HealthProbe>,
        ) = match config.authoritative_backend {
            AuthoritativeBackend::Scylla => {
                let store = Arc::new(ScyllaDB::new(&config.authoritative_uri).await?);
                (store.clone(), store)
            }
            AuthoritativeBackend::MongoDb => {
                let store = Arc::new(
                    MongoDbAuthoritativeStore::new(
                        &config.authoritative_uri,
                        &config.mongodb_database,
                    )
                    .await?,
                );
                mongodb_database = Some(store.database_handle());
                (store.clone(), store)
            }
        };

        let (guard_memo_mutations, guard_high_search_queries) =
            maintenance_participation(&config.high_memo_crypto, &config.high_search);
        let shared_maintenance_guard = if guard_memo_mutations {
            let database = mongodb_database.as_ref().ok_or_else(|| {
                AppError::ServiceUnavailable(
                    "HIGH maintenance guard requires MongoDB authoritative storage".into(),
                )
            })?;
            Some(Arc::new(
                MongoHighSearchMaintenanceGuard::new(database.clone()).await?,
            ))
        } else {
            None
        };

        let mutation_guard: Arc<dyn MemoMutationGuard> =
            if let Some(guard) = shared_maintenance_guard.as_ref() {
                guard.clone()
            } else {
                Arc::new(UnrestrictedMemoMutationGuard)
            };

        let high_search_query_guard: Arc<dyn HighSearchQueryGuard> = if guard_high_search_queries {
            let guard = shared_maintenance_guard.as_ref().ok_or_else(|| {
                AppError::ServiceUnavailable(
                    "HIGH search query maintenance requires the shared MongoDB guard".into(),
                )
            })?;
            guard.clone()
        } else {
            Arc::new(UnrestrictedHighSearchQueryGuard)
        };

        let redis = Arc::new(RedisCache::new(&config.redis_uri)?);

        let (search_projection, search_health): (
            Arc<dyn MemoSearchProjection>,
            Arc<dyn HealthProbe>,
        ) = match config.search_backend {
            SearchBackend::Elasticsearch => {
                let client = Arc::new(ElasticsearchClient::new(&config.search_uri).await?);
                (client.clone(), client)
            }
            SearchBackend::Manticore => {
                let client = Arc::new(ManticoreClient::new(&config.search_uri)?);
                (client.clone(), client)
            }
        };

        let cache: Arc<dyn MemoCache> = redis.clone();
        let cache_health: Arc<dyn HealthProbe> = redis;

        Ok(Self {
            authoritative_store,
            cache,
            search_projection,
            authoritative_health,
            cache_health,
            search_health,
            mutation_guard,
            high_search_query_guard,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn high_memo_enabled() -> HighMemoCryptoConfig {
        HighMemoCryptoConfig::AwsKms {
            region: "ap-northeast-1".into(),
            active_key_version: "memo-key-v1".into(),
            key_versions: BTreeMap::from([(
                "memo-key-v1".into(),
                "arn:aws:kms:ap-northeast-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab"
                    .into(),
            )]),
        }
    }

    fn high_search_enabled() -> HighSearchConfig {
        HighSearchConfig::AwsKms {
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
        }
    }

    #[test]
    fn memo_crypto_alone_guards_mutations_but_not_legacy_queries() {
        assert_eq!(
            maintenance_participation(&high_memo_enabled(), &HighSearchConfig::Disabled),
            (true, false)
        );
    }

    #[test]
    fn high_search_guards_both_mutations_and_routed_queries() {
        assert_eq!(
            maintenance_participation(&HighMemoCryptoConfig::Disabled, &high_search_enabled()),
            (true, true)
        );
    }

    #[test]
    fn disabled_high_modes_need_no_shared_guard() {
        assert_eq!(
            maintenance_participation(&HighMemoCryptoConfig::Disabled, &HighSearchConfig::Disabled,),
            (false, false)
        );
    }
}
