use std::collections::HashMap;
use std::env;

use thiserror::Error;
use uuid::Uuid;

const DEFAULT_SCYLLA_URI: &str = "127.0.0.1:9042";
const DEFAULT_REDIS_URI: &str = "redis://127.0.0.1:6379";
const DEFAULT_ELASTICSEARCH_URI: &str = "http://127.0.0.1:9200";
const DEFAULT_PORT: u16 = 8080;
const DEFAULT_DEVELOPMENT_USER_ID: &str = "12345678-1234-1234-1234-123456789012";

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub scylla_uri: String,
    pub redis_uri: String,
    pub elasticsearch_uri: String,
    pub port: u16,
    pub development_user_id: Uuid,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("PORT must be a valid u16, got `{0}`")]
    InvalidPort(String),
    #[error("DEVELOPMENT_USER_ID must be a valid UUID, got `{0}`")]
    InvalidDevelopmentUserId(String),
}

impl AppConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_vars(env::vars())
    }

    pub fn from_vars<I>(vars: I) -> Result<Self, ConfigError>
    where
        I: IntoIterator<Item = (String, String)>,
    {
        let vars: HashMap<String, String> = vars.into_iter().collect();

        let scylla_uri = vars
            .get("SCYLLA_URI")
            .or_else(|| vars.get("DATABASE_URL"))
            .cloned()
            .unwrap_or_else(|| DEFAULT_SCYLLA_URI.to_string());
        let redis_uri = vars
            .get("REDIS_URL")
            .cloned()
            .unwrap_or_else(|| DEFAULT_REDIS_URI.to_string());
        let elasticsearch_uri = vars
            .get("ELASTICSEARCH_URL")
            .cloned()
            .unwrap_or_else(|| DEFAULT_ELASTICSEARCH_URI.to_string());

        let port = match vars.get("PORT") {
            Some(value) => value
                .parse::<u16>()
                .map_err(|_| ConfigError::InvalidPort(value.clone()))?,
            None => DEFAULT_PORT,
        };

        let development_user_id_value = vars
            .get("DEVELOPMENT_USER_ID")
            .map(String::as_str)
            .unwrap_or(DEFAULT_DEVELOPMENT_USER_ID);
        let development_user_id = Uuid::parse_str(development_user_id_value).map_err(|_| {
            ConfigError::InvalidDevelopmentUserId(development_user_id_value.to_string())
        })?;

        Ok(Self {
            scylla_uri,
            redis_uri,
            elasticsearch_uri,
            port,
            development_user_id,
        })
    }
}
