use std::{io, sync::Arc};

use actix_web::{
    middleware,
    web::{self, Data},
    App, HttpServer,
};

use crate::{
    application::{health::HealthService, memo::service::MemoService},
    config::AppConfig,
    infrastructure::{
        auth::AuthService,
        persistence::{elasticsearch::ElasticsearchClient, redis::RedisCache, scylla::ScyllaDB},
        reconciliation::ProjectionReconciler,
        repositories::memo::MemoRepositoryImpl,
    },
    interfaces::routes::configure_routes,
};

const MAX_JSON_PAYLOAD_BYTES: usize = 512 * 1024;

pub struct Application {
    port: u16,
    server: actix_web::dev::Server,
}

impl Application {
    pub async fn build(config: AppConfig) -> io::Result<Self> {
        let scylla = Arc::new(
            ScyllaDB::new(&config.scylla_uri)
                .await
                .map_err(|error| io::Error::other(error.to_string()))?,
        );
        let redis = Arc::new(
            RedisCache::new(&config.redis_uri)
                .map_err(|error| io::Error::other(error.to_string()))?,
        );
        let elasticsearch = Arc::new(
            ElasticsearchClient::new(&config.elasticsearch_uri)
                .await
                .map_err(|error| io::Error::other(error.to_string()))?,
        );

        let health_service = Data::new(HealthService::new(
            scylla.clone(),
            redis.clone(),
            elasticsearch.clone(),
        ));
        let projection_reconciler = Arc::new(ProjectionReconciler::new(
            scylla.clone(),
            redis.clone(),
            elasticsearch.clone(),
        ));
        let memo_repository = Arc::new(MemoRepositoryImpl::new(
            scylla,
            redis,
            elasticsearch,
            projection_reconciler.clone(),
        ));
        let _projection_reconciler_task = tokio::spawn(projection_reconciler.run());
        let memo_service = Data::new(MemoService::new(memo_repository));
        let auth_service = Data::new(AuthService::new(config.auth));
        let port = config.port;

        let server = HttpServer::new(move || {
            App::new()
                .wrap(middleware::Logger::default())
                .wrap(middleware::Compress::default())
                .app_data(memo_service.clone())
                .app_data(health_service.clone())
                .app_data(auth_service.clone())
                .app_data(web::JsonConfig::default().limit(MAX_JSON_PAYLOAD_BYTES))
                .configure(configure_routes)
        })
        .bind(("0.0.0.0", port))?
        .run();

        Ok(Self { port, server })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub async fn run_until_stopped(self) -> io::Result<()> {
        self.server.await
    }
}
