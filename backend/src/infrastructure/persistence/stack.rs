use std::sync::Arc;

use crate::{
    application::health::HealthProbe,
    config::AppConfig,
    error::AppResult,
};

use super::{
    elasticsearch::ElasticsearchClient,
    ports::{MemoAuthoritativeStore, MemoCache, MemoSearchProjection},
    redis::RedisCache,
    scylla::ScyllaDB,
};

pub struct PersistenceStack {
    pub authoritative_store: Arc<dyn MemoAuthoritativeStore>,
    pub cache: Arc<dyn MemoCache>,
    pub search_projection: Arc<dyn MemoSearchProjection>,
    pub authoritative_health: Arc<dyn HealthProbe>,
    pub cache_health: Arc<dyn HealthProbe>,
    pub search_health: Arc<dyn HealthProbe>,
}

impl PersistenceStack {
    pub async fn build(config: &AppConfig) -> AppResult<Self> {
        let scylla = Arc::new(ScyllaDB::new(&config.scylla_uri).await?);
        let redis = Arc::new(RedisCache::new(&config.redis_uri)?);
        let elasticsearch =
            Arc::new(ElasticsearchClient::new(&config.elasticsearch_uri).await?);

        let authoritative_store: Arc<dyn MemoAuthoritativeStore> = scylla.clone();
        let cache: Arc<dyn MemoCache> = redis.clone();
        let search_projection: Arc<dyn MemoSearchProjection> = elasticsearch.clone();

        let authoritative_health: Arc<dyn HealthProbe> = scylla;
        let cache_health: Arc<dyn HealthProbe> = redis;
        let search_health: Arc<dyn HealthProbe> = elasticsearch;

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
