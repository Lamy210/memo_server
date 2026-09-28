use std::{io, sync::Arc, time::Duration};

use actix_web::{
    middleware,
    web::{self, Data},
    App, HttpServer,
};

use crate::{
    application::{
        health::HealthService, high_memo_routing::HighMemoDataRoute,
        high_search_shadow::HighSearchShadowObserver, memo::service::MemoService,
    },
    config::{AppConfig, HighMemoCryptoConfig, HighSearchShadowConfig},
    infrastructure::{
        auth::AuthService, high_memo_authoritative::HighMemoAuthoritativeAdapter,
        high_memo_aws_runtime::HighMemoStagingRuntimeHandle,
        high_memo_cache::HighMemoCiphertextCacheAdapter,
        high_search_aws_runtime::HighSearchRuntimeHandle, persistence::stack::PersistenceStack,
        reconciliation::ProjectionReconciler, repositories::memo::MemoRepositoryImpl,
    },
    interfaces::routes::configure_routes,
};

const MAX_JSON_PAYLOAD_BYTES: usize = 512 * 1024;

pub struct Application {
    port: u16,
    server: actix_web::dev::Server,
    high_search_runtime: HighSearchRuntimeHandle,
}

impl Application {
    pub async fn build(config: AppConfig) -> io::Result<Self> {
        let high_search_runtime =
            HighSearchRuntimeHandle::build(&config.high_search, &config.search_uri)
                .await
                .map_err(|error| io::Error::other(error.to_string()))?;

        let persistence = PersistenceStack::build(&config)
            .await
            .map_err(|error| io::Error::other(error.to_string()))?;

        // When MEMO-HIGH-1 is explicitly configured, every replica must prove
        // that its KMS/key-ring runtime is usable before it can participate in
        // a future encrypted-route cutover. Building this runtime does not
        // change the persisted memo route, which remains legacy by default.
        let high_memo_runtime = HighMemoStagingRuntimeHandle::build(&config.high_memo_crypto)
            .await
            .map_err(|error| io::Error::other(error.to_string()))?;

        let high_search_query_reader = high_search_runtime.query_reader();
        let high_search_shadow = match config.high_search_shadow {
            HighSearchShadowConfig::Disabled => None,
            HighSearchShadowConfig::Observe {
                max_concurrency,
                timeout_ms,
            } => {
                let reader = high_search_query_reader.clone().ok_or_else(|| {
                    io::Error::other(
                        "HIGH search shadow is enabled but no protected query reader is available",
                    )
                })?;
                Some(Arc::new(
                    HighSearchShadowObserver::new(
                        reader,
                        persistence.high_search_query_guard.clone(),
                        max_concurrency,
                        Duration::from_millis(timeout_ms),
                    )
                    .map_err(|error| io::Error::other(error.to_string()))?,
                ))
            }
        };

        let health_service = Data::new(HealthService::new(
            persistence.authoritative_health.clone(),
            persistence.cache_health.clone(),
            persistence.search_health.clone(),
        ));
        let high_search_projection_sink = high_search_runtime.projection_sink();
        let projection_reconciler = ProjectionReconciler::new(
            persistence.authoritative_store.clone(),
            persistence.cache.clone(),
            persistence.search_projection.clone(),
            high_search_projection_sink.clone(),
            persistence.mutation_guard.clone(),
        );
        let projection_reconciler =
            if let Some(route_reader) = persistence.high_search_route_reader.clone() {
                projection_reconciler.with_search_route(route_reader)
            } else {
                projection_reconciler
            };
        let projection_reconciler = match &config.high_memo_crypto {
            HighMemoCryptoConfig::Disabled => projection_reconciler,
            HighMemoCryptoConfig::AwsKms { .. } => projection_reconciler.with_memo_route(
                persistence.high_memo_access_guard.clone(),
                HighMemoDataRoute::LegacyPlaintext,
            ),
        };
        let projection_reconciler = Arc::new(projection_reconciler);
        let memo_repository = Arc::new(MemoRepositoryImpl::new(
            persistence.authoritative_store.clone(),
            persistence.cache.clone(),
            persistence.search_projection.clone(),
            projection_reconciler.clone(),
        ));
        let _projection_reconciler_task = tokio::spawn(projection_reconciler.run());

        let encrypted_memo_repository = match &config.high_memo_crypto {
            HighMemoCryptoConfig::Disabled => None,
            HighMemoCryptoConfig::AwsKms { .. } => {
                let cryptography = high_memo_runtime.request_cryptography().ok_or_else(|| {
                    io::Error::other(
                        "MEMO-HIGH-1 is enabled but no request cryptography runtime is available",
                    )
                })?;
                let encrypted_store = persistence
                    .high_encrypted_authoritative_store
                    .clone()
                    .ok_or_else(|| {
                        io::Error::other(
                            "MEMO-HIGH-1 encrypted authoritative storage requires MongoDB",
                        )
                    })?;
                let encrypted_authoritative = Arc::new(HighMemoAuthoritativeAdapter::new(
                    encrypted_store,
                    cryptography.clone(),
                ));
                let encrypted_cache = Arc::new(HighMemoCiphertextCacheAdapter::new(
                    persistence.high_encrypted_cache.clone(),
                    cryptography,
                ));

                // The encrypted collection owns a distinct durable outbox.
                // Never feed those intents through the legacy authoritative
                // reconciler, or retries could hydrate the wrong generation.
                let encrypted_reconciler = ProjectionReconciler::new(
                    encrypted_authoritative.clone(),
                    encrypted_cache.clone(),
                    persistence.search_projection.clone(),
                    high_search_projection_sink.clone(),
                    persistence.mutation_guard.clone(),
                );
                let encrypted_reconciler =
                    if let Some(route_reader) = persistence.high_search_route_reader.clone() {
                        encrypted_reconciler.with_search_route(route_reader)
                    } else {
                        encrypted_reconciler
                    };
                let encrypted_reconciler = Arc::new(encrypted_reconciler.with_memo_route(
                    persistence.high_memo_access_guard.clone(),
                    HighMemoDataRoute::Encrypted,
                ));
                let encrypted_repository = Arc::new(MemoRepositoryImpl::new(
                    encrypted_authoritative,
                    encrypted_cache,
                    persistence.search_projection.clone(),
                    encrypted_reconciler.clone(),
                ));
                let _encrypted_projection_reconciler_task =
                    tokio::spawn(encrypted_reconciler.run());

                let repository: Arc<dyn crate::domain::memo::repository::MemoRepository> =
                    encrypted_repository;
                Some(repository)
            }
        };

        let memo_service = Data::new(MemoService::new(
            memo_repository,
            encrypted_memo_repository,
            persistence.mutation_guard,
            persistence.high_memo_access_guard,
            persistence.high_search_query_guard,
            high_search_query_reader,
            high_search_shadow,
        ));
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

        Ok(Self {
            port,
            server,
            high_search_runtime,
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub async fn run_until_stopped(self) -> io::Result<()> {
        let Self {
            server,
            high_search_runtime,
            ..
        } = self;

        let result = server.await;
        drop(high_search_runtime);
        result
    }
}
