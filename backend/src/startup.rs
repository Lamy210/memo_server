use std::{io, sync::Arc};

use actix_web::{middleware, web::Data, App, HttpServer};

use crate::{
    application::memo::service::MemoService,
    config::AppConfig,
    infrastructure::{
        persistence::{elasticsearch::ElasticsearchClient, redis::RedisCache, scylla::ScyllaDB},
        repositories::memo::MemoRepositoryImpl,
    },
    interfaces::routes::configure_routes,
};

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

        let memo_repository = Arc::new(MemoRepositoryImpl::new(
            scylla,
            redis,
            elasticsearch,
        ));
        let memo_service = Data::new(MemoService::new(memo_repository));
        let development_user_id = Data::new(config.development_user_id);
        let port = config.port;

        let server = HttpServer::new(move || {
            App::new()
                .wrap(middleware::Logger::default())
                .wrap(middleware::Compress::default())
                .app_data(memo_service.clone())
                .app_data(development_user_id.clone())
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
