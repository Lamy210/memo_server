use std::collections::HashMap;
use std::env;

use thiserror::Error;

const DEFAULT_SCYLLA_URI: &str = "127.0.0.1:9042";
const DEFAULT_REDIS_URI: &str = "redis://127.0.0.1:6379";
const DEFAULT_ELASTICSEARCH_URI: &str = "http://127.0.0.1:9200";
const DEFAULT_PORT: u16 = 8080;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthMode {
    Development,
    Jwt,
}

#[derive(Debug, Clone)]
pub struct AuthConfig {
    pub mode: AuthMode,
    pub issuer: Option<String>,
    pub audience: Option<String>,
    pub jwks_uri: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub scylla_uri: String,
    pub redis_uri: String,
    pub elasticsearch_uri: String,
    pub port: u16,
    pub auth: AuthConfig,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("PORT must be a valid u16, got `{0}`")]
    InvalidPort(String),
    #[error("AUTH_MODE is required; use `development` or `jwt`")]
    MissingAuthMode,
    #[error("AUTH_MODE must be `development` or `jwt`, got `{0}`")]
    InvalidAuthMode(String),
    #[error("{0} is required when AUTH_MODE=jwt")]
    MissingJwtSetting(&'static str),
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

        let auth_mode_value = vars
            .get("AUTH_MODE")
            .ok_or(ConfigError::MissingAuthMode)?
            .to_ascii_lowercase();

        let auth = match auth_mode_value.as_str() {
            "development" => AuthConfig {
                mode: AuthMode::Development,
                issuer: None,
                audience: None,
                jwks_uri: None,
            },
            "jwt" => AuthConfig {
                mode: AuthMode::Jwt,
                issuer: Some(required_jwt_setting(&vars, "AUTH_ISSUER")?),
                audience: Some(required_jwt_setting(&vars, "AUTH_AUDIENCE")?),
                jwks_uri: Some(required_jwt_setting(&vars, "AUTH_JWKS_URI")?),
            },
            _ => return Err(ConfigError::InvalidAuthMode(auth_mode_value)),
        };

        Ok(Self {
            scylla_uri,
            redis_uri,
            elasticsearch_uri,
            port,
            auth,
        })
    }
}

fn required_jwt_setting(
    vars: &HashMap<String, String>,
    name: &'static str,
) -> Result<String, ConfigError> {
    vars.get(name)
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .ok_or(ConfigError::MissingJwtSetting(name))
}
