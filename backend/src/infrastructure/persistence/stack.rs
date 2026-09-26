use std::sync::Arc;

use crate::{
    application::{
        health::HealthProbe,
        maintenance::{MemoMutationGuard, UnrestrictedMemoMutationGuard},
    },
    config::{AppConfig, AuthoritativeBackend, HighSearchConfig, SearchBackend},
    error::{AppError, AppResult},
};

use super::{
    elasticsearch::ElasticsearchClient,
    high_search_maintenance_mongodb::MongoHighSearchMaintenanceGuard,
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

        let mutation_guard: Arc<dyn MemoMutationGuard> = match config.high_search {
            HighSearchConfig::Disabled => Arc::new(UnrestrictedMemoMutationGuard),
            HighSearchConfig::AwsKms { .. } => {
                let database = mongodb_database.ok_or_else(|| {
                    AppError::ServiceUnavailable(
                        "HIGH search maintenance guard requires MongoDB authoritative storage"
                            .into(),
                    )
                })?;
                Arc::new(MongoHighSearchMaintenanceGuard::new(database).await?)
            }
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
        })
    }
}
