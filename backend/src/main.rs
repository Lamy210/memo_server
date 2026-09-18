use std::io;

use env_logger::Env;
use memo_app_backend::{config::AppConfig, startup::Application};

#[actix_web::main]
async fn main() -> io::Result<()> {
    env_logger::init_from_env(Env::default().default_filter_or("info"));

    let config = AppConfig::from_env().map_err(io::Error::other)?;
    let application = Application::build(config).await?;

    log::info!("Starting server at port {}", application.port());
    application.run_until_stopped().await
}
