use std::sync::Arc;
use actix_web::{web::Data, App, HttpServer, middleware};
use crate::{
    application::memo::service::MemoService,
    infrastructure::{
        persistence::{
            scylla::ScyllaDB,
            redis::RedisCache,
            elasticsearch::ElasticsearchClient,
        },
        repositories::memo::MemoRepositoryImpl,
    },
    interfaces::routes::configure_routes,
};

pub struct Application {
    port: u16,
    server: actix_web::dev::Server,
}

impl Application {
    pub async fn build(
        scylla_uri: String,
        redis_uri: String,
        elasticsearch_uri: String,
        port: u16,
    ) -> std::io::Result<Self> {
        // データベース接続の初期化
        let scylla = Arc::new(ScyllaDB::new(&scylla_uri).await.unwrap());
        let redis = Arc::new(RedisCache::new(&redis_uri).unwrap());
        let elasticsearch = Arc::new(ElasticsearchClient::new(&elasticsearch_uri).await.unwrap());

        // リポジトリの初期化
        let memo_repository = Arc::new(
            MemoRepositoryImpl::new(
                scylla.clone(),
                redis.clone(),
                elasticsearch.clone(),
            )
            .await
            .unwrap(),
        );

        // サービスの初期化
        let memo_service = Data::new(MemoService::new(memo_repository));

        // HTTPサーバーの設定
        let server = HttpServer::new(move || {
            App::new()
                .wrap(middleware::Logger::default())
                .wrap(middleware::Compress::default())
                .app_data(memo_service.clone())
                .configure(configure_routes)
        })
        .bind(("0.0.0.0", port))?
        .run();

        Ok(Self { port, server })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub async fn run_until_stopped(self) -> std::io::Result<()> {
        self.server.await
    }
}