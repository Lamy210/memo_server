use std::sync::Arc;

use crate::{
    application::health::HealthProbe,
    config::{AppConfig, SearchBackend},
    error::AppResult,
};

use super::{
    elasticsearch::ElasticsearchClient,
    manticore::ManticoreClient,
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
}

impl PersistenceStack {
    pub(crate) async fn build(config: &AppConfig) -> AppResult<Self> {
        let scylla = Arc::new(ScyllaDB::new(&config.scylla_uri).await?);
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

        let authoritative_store: Arc<dyn MemoAuthoritativeStore> = scylla.clone();
        let cache: Arc<dyn MemoCache> = redis.clone();

        let authoritative_health: Arc<dyn HealthProbe> = scylla;
        let cache_health: Arc<dyn HealthProbe> = redis;

        Ok(Self {
            authoritative_store,
            cache,
            search_projection,
            authoritative_health,
            cache_health,
            search_health,
        })
    }
}
