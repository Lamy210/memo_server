use std::collections::HashMap;
use std::env;

use thiserror::Error;

const DEFAULT_SCYLLA_URI: &str = "127.0.0.1:9042";
const DEFAULT_MONGODB_URI: &str = "mongodb://127.0.0.1:27017/?replicaSet=rs0&directConnection=true";
const DEFAULT_MONGODB_DATABASE: &str = "memo_app";
const DEFAULT_REDIS_URI: &str = "redis://127.0.0.1:6379";
const DEFAULT_ELASTICSEARCH_URI: &str = "http://127.0.0.1:9200";
const DEFAULT_MANTICORE_URI: &str = "http://127.0.0.1:9308";
const DEFAULT_PORT: u16 = 8080;
const MAX_SEARCH_VERSION_ID_CHARS: usize = 128;
const HIGH_SEARCH_PRF_PREFIX: &str = "prf384-v1:";
const HIGH_SEARCH_HKDF_PREFIX: &str = "hkdf384-v1:";

// MongoDB database names on Unix/Linux must not contain NUL, space, double quote,
// dollar sign, dot, forward slash, or backslash.
const INVALID_MONGODB_DATABASE_BYTES: [u8; 7] = [0, 32, 34, 36, 46, 47, 92];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthConfig {
    Development,
    Jwt {
        issuer: String,
        audience: String,
        jwks_uri: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthoritativeBackend {
    Scylla,
    MongoDb,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchBackend {
    Elasticsearch,
    Manticore,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HighSearchConfig {
    Disabled,
    AwsKms {
        key_arn: String,
        region: String,
        provider_seed_version: String,
        cache_ttl_seconds: u64,
        cache_max_entries: usize,
        cache_sweep_seconds: u64,
    },
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub authoritative_backend: AuthoritativeBackend,
    pub authoritative_uri: String,
    pub mongodb_database: String,
    pub redis_uri: String,
    pub search_backend: SearchBackend,
    pub search_uri: String,
    pub high_search: HighSearchConfig,
    pub port: u16,
    pub auth: AuthConfig,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("PORT must be a valid u16, got `{0}`")]
    InvalidPort(String),
    #[error("AUTHORITATIVE_BACKEND must be `scylla` or `mongodb`, got `{0}`")]
    InvalidAuthoritativeBackend(String),
    #[error("MONGODB_DATABASE must not be empty")]
    EmptyMongoDatabase,
    #[error("MONGODB_DATABASE is not a valid MongoDB database name: `{0}`")]
    InvalidMongoDatabase(String),
    #[error("SEARCH_BACKEND must be `elasticsearch` or `manticore`, got `{0}`")]
    InvalidSearchBackend(String),
    #[error("HIGH_SEARCH_MODE must be `disabled` or `aws-kms`, got `{0}`")]
    InvalidHighSearchMode(String),
    #[error("HIGH_SEARCH_MODE=aws-kms requires SEARCH_BACKEND=manticore")]
    HighSearchRequiresManticore,
    #[error("HIGH_SEARCH_MODE=aws-kms requires the binary to be built with feature `aws-kms-search`")]
    HighSearchBuildFeatureUnavailable,
    #[error("{0} is required when HIGH_SEARCH_MODE=aws-kms")]
    MissingHighSearchSetting(&'static str),
    #[error("{0} is invalid for HIGH_SEARCH_MODE=aws-kms: `{1}`")]
    InvalidHighSearchSetting(&'static str, String),
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

        let authoritative_backend = match vars
            .get("AUTHORITATIVE_BACKEND")
            .map(|value| value.to_ascii_lowercase())
            .as_deref()
        {
            None | Some("scylla") => AuthoritativeBackend::Scylla,
            Some("mongodb") => AuthoritativeBackend::MongoDb,
            Some(value) => return Err(ConfigError::InvalidAuthoritativeBackend(value.to_string())),
        };

        let authoritative_uri = match authoritative_backend {
            AuthoritativeBackend::Scylla => vars
                .get("SCYLLA_URI")
                .or_else(|| vars.get("DATABASE_URL"))
                .cloned()
                .unwrap_or_else(|| DEFAULT_SCYLLA_URI.to_string()),
            AuthoritativeBackend::MongoDb => vars
                .get("MONGODB_URI")
                .cloned()
                .unwrap_or_else(|| DEFAULT_MONGODB_URI.to_string()),
        };

        let mongodb_database = vars
            .get("MONGODB_DATABASE")
            .cloned()
            .unwrap_or_else(|| DEFAULT_MONGODB_DATABASE.to_string());
        if authoritative_backend == AuthoritativeBackend::MongoDb {
            validate_mongodb_database_name(&mongodb_database)?;
        }

        let redis_uri = vars
            .get("REDIS_URL")
            .cloned()
            .unwrap_or_else(|| DEFAULT_REDIS_URI.to_string());

        let search_backend = match vars
            .get("SEARCH_BACKEND")
            .map(|value| value.to_ascii_lowercase())
            .as_deref()
        {
            None | Some("elasticsearch") => SearchBackend::Elasticsearch,
            Some("manticore") => SearchBackend::Manticore,
            Some(value) => return Err(ConfigError::InvalidSearchBackend(value.to_string())),
        };

        let search_uri = match search_backend {
            SearchBackend::Elasticsearch => vars
                .get("ELASTICSEARCH_URL")
                .cloned()
                .unwrap_or_else(|| DEFAULT_ELASTICSEARCH_URI.to_string()),
            SearchBackend::Manticore => vars
                .get("MANTICORE_URL")
                .cloned()
                .unwrap_or_else(|| DEFAULT_MANTICORE_URI.to_string()),
        };

        let high_search = parse_high_search_config(&vars, search_backend)?;

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
            "development" => AuthConfig::Development,
            "jwt" => AuthConfig::Jwt {
                issuer: required_jwt_setting(&vars, "AUTH_ISSUER")?,
                audience: required_jwt_setting(&vars, "AUTH_AUDIENCE")?,
                jwks_uri: required_jwt_setting(&vars, "AUTH_JWKS_URI")?,
            },
            _ => return Err(ConfigError::InvalidAuthMode(auth_mode_value)),
        };

        Ok(Self {
            authoritative_backend,
            authoritative_uri,
            mongodb_database,
            redis_uri,
            search_backend,
            search_uri,
            high_search,
            port,
            auth,
        })
    }
}

fn parse_high_search_config(
    vars: &HashMap<String, String>,
    search_backend: SearchBackend,
) -> Result<HighSearchConfig, ConfigError> {
    let mode = vars
        .get("HIGH_SEARCH_MODE")
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_else(|| "disabled".to_string());

    match mode.as_str() {
        "disabled" => Ok(HighSearchConfig::Disabled),
        "aws-kms" => {
            if search_backend != SearchBackend::Manticore {
                return Err(ConfigError::HighSearchRequiresManticore);
            }

            let key_arn = required_high_search_setting(vars, "HIGH_SEARCH_AWS_KMS_KEY_ARN")?;
            let region = required_high_search_setting(vars, "HIGH_SEARCH_AWS_REGION")?;
            validate_aws_region(&region)?;
            validate_high_search_kms_key_arn(&key_arn, &region)?;

            let provider_seed_version =
                required_high_search_setting(vars, "HIGH_SEARCH_SEED_VERSION")?;
            validate_provider_seed_version(&provider_seed_version)?;

            let cache_ttl_seconds = parse_positive_high_search_setting::<u64>(
                vars,
                "HIGH_SEARCH_KEY_CACHE_TTL_SECONDS",
            )?;
            let cache_max_entries = parse_positive_high_search_setting::<usize>(
                vars,
                "HIGH_SEARCH_KEY_CACHE_MAX_ENTRIES",
            )?;
            let cache_sweep_seconds = parse_positive_high_search_setting::<u64>(
                vars,
                "HIGH_SEARCH_KEY_CACHE_SWEEP_SECONDS",
            )?;

            let config = HighSearchConfig::AwsKms {
                key_arn,
                region,
                provider_seed_version,
                cache_ttl_seconds,
                cache_max_entries,
                cache_sweep_seconds,
            };

            if !cfg!(feature = "aws-kms-search") {
                return Err(ConfigError::HighSearchBuildFeatureUnavailable);
            }

            Ok(config)
        }
        _ => Err(ConfigError::InvalidHighSearchMode(mode)),
    }
}

fn required_high_search_setting(
    vars: &HashMap<String, String>,
    name: &'static str,
) -> Result<String, ConfigError> {
    vars.get(name)
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .ok_or(ConfigError::MissingHighSearchSetting(name))
}

fn parse_positive_high_search_setting<T>(
    vars: &HashMap<String, String>,
    name: &'static str,
) -> Result<T, ConfigError>
where
    T: std::str::FromStr + PartialEq + Default,
{
    let raw = required_high_search_setting(vars, name)?;
    let parsed = raw
        .parse::<T>()
        .map_err(|_| ConfigError::InvalidHighSearchSetting(name, raw.clone()))?;
    if parsed == T::default() {
        return Err(ConfigError::InvalidHighSearchSetting(name, raw));
    }
    Ok(parsed)
}

fn validate_provider_seed_version(value: &str) -> Result<(), ConfigError> {
    let final_len = HIGH_SEARCH_HKDF_PREFIX.len() + HIGH_SEARCH_PRF_PREFIX.len() + value.len();
    let valid_chars = value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'));

    if value.is_empty() || !valid_chars || final_len > MAX_SEARCH_VERSION_ID_CHARS {
        return Err(ConfigError::InvalidHighSearchSetting(
            "HIGH_SEARCH_SEED_VERSION",
            value.to_string(),
        ));
    }

    Ok(())
}

fn validate_aws_region(region: &str) -> Result<(), ConfigError> {
    let valid = !region.is_empty()
        && region.trim() == region
        && region
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !region.starts_with('-')
        && !region.ends_with('-');

    if !valid {
        return Err(ConfigError::InvalidHighSearchSetting(
            "HIGH_SEARCH_AWS_REGION",
            region.to_string(),
        ));
    }

    Ok(())
}

fn validate_high_search_kms_key_arn(
    key_arn: &str,
    expected_region: &str,
) -> Result<(), ConfigError> {
    let parts: Vec<&str> = key_arn.splitn(6, ':').collect();
    let valid = parts.len() == 6
        && parts[0] == "arn"
        && !parts[1].is_empty()
        && parts[2] == "kms"
        && parts[3] == expected_region
        && !parts[4].is_empty()
        && parts[5].starts_with("key/")
        && !parts[5].starts_with("alias/")
        && parts[5].strip_prefix("key/").is_some_and(|resource| {
            !resource.is_empty()
                && !resource.contains('/')
                && !resource.chars().any(char::is_whitespace)
        });

    if !valid {
        return Err(ConfigError::InvalidHighSearchSetting(
            "HIGH_SEARCH_AWS_KMS_KEY_ARN",
            key_arn.to_string(),
        ));
    }

    Ok(())
}

fn validate_mongodb_database_name(name: &str) -> Result<(), ConfigError> {
    if name.trim().is_empty() {
        return Err(ConfigError::EmptyMongoDatabase);
    }

    let has_invalid_byte = name
        .bytes()
        .any(|byte| INVALID_MONGODB_DATABASE_BYTES.contains(&byte));
    if name.len() >= 64 || has_invalid_byte {
        return Err(ConfigError::InvalidMongoDatabase(name.to_string()));
    }

    Ok(())
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
