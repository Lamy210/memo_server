use env_logger::Env;
use memo_app_backend::startup::Application;

mod application;
mod domain;
mod error;
mod infrastructure;
mod interfaces;
mod startup;

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    // ロギングの初期化
    env_logger::init_from_env(Env::default().default_filter_or("info"));

    // 環境変数から設定を読み込む
    let scylla_uri = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "scylla://localhost:9042/memo_app".to_string());
    let redis_uri = std::env::var("REDIS_URL")
        .unwrap_or_else(|_| "redis://localhost:6379".to_string());
    let elasticsearch_uri = std::env::var("ELASTICSEARCH_URL")
        .unwrap_or_else(|_| "http://localhost:9200".to_string());
    let port = std::env::var("PORT")
        .unwrap_or_else(|_| "8080".to_string())
        .parse::<u16>()
        .expect("Failed to parse PORT");

    // アプリケーションの構築と起動
    let application = Application::build(
        scylla_uri,
        redis_uri,
        elasticsearch_uri,
        port,
    )
    .await?;

    log::info!("Starting server at port {}", application.port());

    application.run_until_stopped().await?;

    Ok(())
}