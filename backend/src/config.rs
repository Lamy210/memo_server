use std::collections::{BTreeMap, HashMap, HashSet};
use std::env;

use reqwest::Url;
use serde::Deserialize;
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
const MAX_KMS_KEY_ARN_BYTES: usize = 2048;
const MAX_HIGH_MEMO_KEY_VERSION_ID_CHARS: usize = 128;
const MAX_HIGH_MEMO_KMS_KEY_VERSIONS: usize = 32;
const MAX_HIGH_MEMO_KMS_KEYS_JSON_BYTES: usize = 64 * 1024;
const MAX_HIGH_SEARCH_SHADOW_CONCURRENCY: usize = 256;
const MAX_HIGH_SEARCH_SHADOW_TIMEOUT_MS: u64 = 60_000;
const MIN_AUTH_ACCESS_TOKEN_MAX_LIFETIME_SECONDS: u64 = 60;
const MAX_AUTH_ACCESS_TOKEN_MAX_LIFETIME_SECONDS: u64 = 3_600;

// MongoDB database names on Unix/Linux must not contain NUL, space, double quote,
// dollar sign, dot, forward slash, or backslash.
const INVALID_MONGODB_DATABASE_BYTES: [u8; 7] = [0, 32, 34, 36, 46, 47, 92];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JwtSignatureMode {
    /// Existing production contract. This remains the default when the
    /// migration setting is absent so current deployments do not widen their
    /// accepted algorithm set implicitly.
    Rs256,
    /// Explicit migration window in which the independent auth service may
    /// rotate issuers from RSA to P-384 while memo_server accepts both.
    Rs256Es384,
    /// Target AUTH-1 contract. Legacy RSA access tokens are rejected.
    Es384,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JwtAccessTokenTypeMode {
    /// Compatibility mode for the current external issuer contract. Header
    /// `typ` is not used as a validation signal yet.
    LegacyAny,
    /// RFC 9068 access-token profile. Only `at+jwt` (or its full media type)
    /// is accepted so other JWT kinds cannot be substituted as access tokens.
    AtJwt,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthConfig {
    Development,
    Jwt {
        issuer: String,
        audience: String,
        jwks_uri: String,
        signature_mode: JwtSignatureMode,
        access_token_type_mode: JwtAccessTokenTypeMode,
        max_access_token_lifetime_seconds: u64,
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
pub enum HighMemoCryptoConfig {
    Disabled,
    AwsKms {
        region: String,
        active_key_version: String,
        key_versions: BTreeMap<String, String>,
    },
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
        max_document_content_terms: usize,
        max_query_content_terms: usize,
        max_normalized_term_bytes: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HighSearchShadowConfig {
    Disabled,
    Observe {
        max_concurrency: usize,
        timeout_ms: u64,
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
    pub high_memo_crypto: HighMemoCryptoConfig,
    pub high_search: HighSearchConfig,
    pub high_search_shadow: HighSearchShadowConfig,
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
    #[error("HIGH_MEMO_CRYPTO_MODE must be `disabled` or `aws-kms`, got `{0}`")]
    InvalidHighMemoCryptoMode(String),
    #[error("HIGH_MEMO_CRYPTO_MODE=aws-kms requires AUTHORITATIVE_BACKEND=mongodb")]
    HighMemoCryptoRequiresMongoDb,
    #[error(
        "HIGH_MEMO_CRYPTO_MODE=aws-kms requires the binary to be built with feature `aws-kms-memo`"
    )]
    HighMemoCryptoBuildFeatureUnavailable,
    #[error("{0} is required when HIGH_MEMO_CRYPTO_MODE=aws-kms")]
    MissingHighMemoCryptoSetting(&'static str),
    #[error("{0} is invalid for HIGH_MEMO_CRYPTO_MODE=aws-kms: `{1}`")]
    InvalidHighMemoCryptoSetting(&'static str, String),
    #[error("HIGH_SEARCH_MODE must be `disabled` or `aws-kms`, got `{0}`")]
    InvalidHighSearchMode(String),
    #[error("HIGH_SEARCH_SHADOW_MODE must be `disabled` or `observe`, got `{0}`")]
    InvalidHighSearchShadowMode(String),
    #[error("HIGH_SEARCH_SHADOW_MODE=observe requires HIGH_SEARCH_MODE=aws-kms")]
    HighSearchShadowRequiresHighSearch,
    #[error("HIGH_SEARCH_MODE=aws-kms requires AUTHORITATIVE_BACKEND=mongodb")]
    HighSearchRequiresMongoDb,
    #[error("HIGH_SEARCH_MODE=aws-kms requires SEARCH_BACKEND=manticore")]
    HighSearchRequiresManticore,
    #[error(
        "HIGH_SEARCH_MODE=aws-kms requires the binary to be built with feature `aws-kms-search`"
    )]
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
    #[error(
        "AUTH_ISSUER must be an absolute HTTPS URL without userinfo, query, or fragment, got `{0}`"
    )]
    InvalidJwtIssuer(String),
    #[error("AUTH_JWKS_URI must be an absolute HTTPS URL without userinfo or fragment, got `{0}`")]
    InvalidJwtJwksUri(String),
    #[error("AUTH_JWT_SIGNATURE_MODE must be `rs256`, `rs256-es384`, or `es384`, got `{0}`")]
    InvalidJwtSignatureMode(String),
    #[error("AUTH_JWT_TYPE_MODE must be `legacy-any` or `at-jwt`, got `{0}`")]
    InvalidJwtAccessTokenTypeMode(String),
    #[error("AUTH_ACCESS_TOKEN_MAX_LIFETIME_SECONDS must be an integer in 60..=3600, got `{0}`")]
    InvalidJwtAccessTokenMaxLifetime(String),
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

        let high_memo_crypto = parse_high_memo_crypto_config(&vars, authoritative_backend)?;
        let high_search = parse_high_search_config(&vars, authoritative_backend, search_backend)?;
        let high_search_shadow = parse_high_search_shadow_config(&vars, &high_search)?;

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
            "jwt" => {
                let issuer = required_jwt_setting(&vars, "AUTH_ISSUER")?;
                validate_jwt_issuer(&issuer)?;
                let audience = required_jwt_setting(&vars, "AUTH_AUDIENCE")?;
                let jwks_uri = required_jwt_setting(&vars, "AUTH_JWKS_URI")?;
                validate_jwt_jwks_uri(&jwks_uri)?;

                AuthConfig::Jwt {
                    issuer,
                    audience,
                    jwks_uri,
                    signature_mode: parse_jwt_signature_mode(&vars)?,
                    access_token_type_mode: parse_jwt_access_token_type_mode(&vars)?,
                    max_access_token_lifetime_seconds: parse_jwt_access_token_max_lifetime_seconds(
                        &vars,
                    )?,
                }
            }
            _ => return Err(ConfigError::InvalidAuthMode(auth_mode_value)),
        };

        Ok(Self {
            authoritative_backend,
            authoritative_uri,
            mongodb_database,
            redis_uri,
            search_backend,
            search_uri,
            high_memo_crypto,
            high_search,
            high_search_shadow,
            port,
            auth,
        })
    }
}

fn raw_url_authority_contains_userinfo(value: &str) -> bool {
    let Some((_, remainder)) = value.split_once("://") else {
        return false;
    };
    let authority_end = remainder
        .char_indices()
        .find_map(|(index, character)| matches!(character, '/' | '?' | '#').then_some(index))
        .unwrap_or(remainder.len());

    remainder[..authority_end].contains('@')
}

fn validate_jwt_issuer(value: &str) -> Result<(), ConfigError> {
    let url = Url::parse(value).map_err(|_| ConfigError::InvalidJwtIssuer(value.to_string()))?;
    let valid = url.scheme() == "https"
        && url.has_host()
        && !raw_url_authority_contains_userinfo(value)
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none();

    if valid {
        Ok(())
    } else {
        Err(ConfigError::InvalidJwtIssuer(value.to_string()))
    }
}

fn validate_jwt_jwks_uri(value: &str) -> Result<(), ConfigError> {
    let url = Url::parse(value).map_err(|_| ConfigError::InvalidJwtJwksUri(value.to_string()))?;
    let valid = url.scheme() == "https"
        && url.has_host()
        && !raw_url_authority_contains_userinfo(value)
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none();

    if valid {
        Ok(())
    } else {
        Err(ConfigError::InvalidJwtJwksUri(value.to_string()))
    }
}

fn parse_jwt_signature_mode(
    vars: &HashMap<String, String>,
) -> Result<JwtSignatureMode, ConfigError> {
    let mode = vars
        .get("AUTH_JWT_SIGNATURE_MODE")
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_else(|| "rs256".to_string());

    match mode.as_str() {
        "rs256" => Ok(JwtSignatureMode::Rs256),
        "rs256-es384" => Ok(JwtSignatureMode::Rs256Es384),
        "es384" => Ok(JwtSignatureMode::Es384),
        _ => Err(ConfigError::InvalidJwtSignatureMode(mode)),
    }
}

fn parse_jwt_access_token_type_mode(
    vars: &HashMap<String, String>,
) -> Result<JwtAccessTokenTypeMode, ConfigError> {
    let mode = vars
        .get("AUTH_JWT_TYPE_MODE")
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_else(|| "legacy-any".to_string());

    match mode.as_str() {
        "legacy-any" => Ok(JwtAccessTokenTypeMode::LegacyAny),
        "at-jwt" => Ok(JwtAccessTokenTypeMode::AtJwt),
        _ => Err(ConfigError::InvalidJwtAccessTokenTypeMode(mode)),
    }
}

fn parse_jwt_access_token_max_lifetime_seconds(
    vars: &HashMap<String, String>,
) -> Result<u64, ConfigError> {
    let value = required_jwt_setting(vars, "AUTH_ACCESS_TOKEN_MAX_LIFETIME_SECONDS")?;
    let parsed = value
        .parse::<u64>()
        .map_err(|_| ConfigError::InvalidJwtAccessTokenMaxLifetime(value.clone()))?;

    if !(MIN_AUTH_ACCESS_TOKEN_MAX_LIFETIME_SECONDS..=MAX_AUTH_ACCESS_TOKEN_MAX_LIFETIME_SECONDS)
        .contains(&parsed)
    {
        return Err(ConfigError::InvalidJwtAccessTokenMaxLifetime(value));
    }

    Ok(parsed)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HighMemoKmsKeyEntry {
    key_version: String,
    key_arn: String,
}

fn parse_high_memo_crypto_config(
    vars: &HashMap<String, String>,
    authoritative_backend: AuthoritativeBackend,
) -> Result<HighMemoCryptoConfig, ConfigError> {
    let mode = vars
        .get("HIGH_MEMO_CRYPTO_MODE")
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_else(|| "disabled".to_string());

    match mode.as_str() {
        "disabled" => Ok(HighMemoCryptoConfig::Disabled),
        "aws-kms" => {
            if authoritative_backend != AuthoritativeBackend::MongoDb {
                return Err(ConfigError::HighMemoCryptoRequiresMongoDb);
            }

            let region = required_high_memo_setting(vars, "HIGH_MEMO_AWS_REGION")?;
            validate_high_memo_aws_region(&region)?;

            let active_key_version =
                required_high_memo_setting(vars, "HIGH_MEMO_ACTIVE_KEY_VERSION")?;
            validate_high_memo_key_version(&active_key_version, "HIGH_MEMO_ACTIVE_KEY_VERSION")?;

            let raw_keys = required_high_memo_setting(vars, "HIGH_MEMO_AWS_KMS_KEYS_JSON")?;
            if raw_keys.len() > MAX_HIGH_MEMO_KMS_KEYS_JSON_BYTES {
                return Err(ConfigError::InvalidHighMemoCryptoSetting(
                    "HIGH_MEMO_AWS_KMS_KEYS_JSON",
                    "configuration exceeds size limit".into(),
                ));
            }

            let entries: Vec<HighMemoKmsKeyEntry> =
                serde_json::from_str(&raw_keys).map_err(|_| {
                    ConfigError::InvalidHighMemoCryptoSetting(
                        "HIGH_MEMO_AWS_KMS_KEYS_JSON",
                        "must be a JSON array of key_version/key_arn objects".into(),
                    )
                })?;
            if entries.is_empty() || entries.len() > MAX_HIGH_MEMO_KMS_KEY_VERSIONS {
                return Err(ConfigError::InvalidHighMemoCryptoSetting(
                    "HIGH_MEMO_AWS_KMS_KEYS_JSON",
                    format!("must contain 1..={MAX_HIGH_MEMO_KMS_KEY_VERSIONS} key versions"),
                ));
            }

            let mut key_versions = BTreeMap::new();
            let mut unique_arns = HashSet::with_capacity(entries.len());
            for entry in entries {
                validate_high_memo_key_version(&entry.key_version, "HIGH_MEMO_AWS_KMS_KEYS_JSON")?;
                validate_high_memo_kms_key_arn(&entry.key_arn, &region)?;
                if key_versions
                    .insert(entry.key_version.clone(), entry.key_arn.clone())
                    .is_some()
                {
                    return Err(ConfigError::InvalidHighMemoCryptoSetting(
                        "HIGH_MEMO_AWS_KMS_KEYS_JSON",
                        format!("duplicate key_version {}", entry.key_version),
                    ));
                }
                if !unique_arns.insert(entry.key_arn) {
                    return Err(ConfigError::InvalidHighMemoCryptoSetting(
                        "HIGH_MEMO_AWS_KMS_KEYS_JSON",
                        "multiple key versions must not point at the same KMS key ARN".into(),
                    ));
                }
            }

            if !key_versions.contains_key(&active_key_version) {
                return Err(ConfigError::InvalidHighMemoCryptoSetting(
                    "HIGH_MEMO_ACTIVE_KEY_VERSION",
                    "active version is not present in HIGH_MEMO_AWS_KMS_KEYS_JSON".into(),
                ));
            }

            if !cfg!(feature = "aws-kms-memo") {
                return Err(ConfigError::HighMemoCryptoBuildFeatureUnavailable);
            }

            Ok(HighMemoCryptoConfig::AwsKms {
                region,
                active_key_version,
                key_versions,
            })
        }
        _ => Err(ConfigError::InvalidHighMemoCryptoMode(mode)),
    }
}

fn required_high_memo_setting(
    vars: &HashMap<String, String>,
    name: &'static str,
) -> Result<String, ConfigError> {
    vars.get(name)
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .ok_or(ConfigError::MissingHighMemoCryptoSetting(name))
}

fn validate_high_memo_key_version(value: &str, setting: &'static str) -> Result<(), ConfigError> {
    let valid = !value.is_empty()
        && value.chars().count() <= MAX_HIGH_MEMO_KEY_VERSION_ID_CHARS
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'));

    if !valid {
        return Err(ConfigError::InvalidHighMemoCryptoSetting(
            setting,
            value.to_string(),
        ));
    }
    Ok(())
}

fn validate_high_memo_aws_region(region: &str) -> Result<(), ConfigError> {
    let valid = !region.is_empty()
        && region.trim() == region
        && region
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !region.starts_with('-')
        && !region.ends_with('-');

    if !valid {
        return Err(ConfigError::InvalidHighMemoCryptoSetting(
            "HIGH_MEMO_AWS_REGION",
            region.to_string(),
        ));
    }
    Ok(())
}

fn validate_high_memo_kms_key_arn(key_arn: &str, expected_region: &str) -> Result<(), ConfigError> {
    let parts: Vec<&str> = key_arn.splitn(6, ':').collect();
    let partition_valid = parts.get(1).is_some_and(|partition| {
        !partition.is_empty()
            && partition
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            && !partition.starts_with('-')
            && !partition.ends_with('-')
    });
    let account_valid = parts.get(4).is_some_and(|account| {
        account.len() == 12 && account.bytes().all(|byte| byte.is_ascii_digit())
    });
    let resource_valid = parts.get(5).is_some_and(|resource| {
        resource.strip_prefix("key/").is_some_and(|key_id| {
            !key_id.is_empty()
                && !key_id.contains('/')
                && key_id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
    });

    let valid = key_arn.len() <= MAX_KMS_KEY_ARN_BYTES
        && key_arn.trim() == key_arn
        && parts.len() == 6
        && parts[0] == "arn"
        && partition_valid
        && parts[2] == "kms"
        && parts[3] == expected_region
        && account_valid
        && resource_valid;

    if !valid {
        return Err(ConfigError::InvalidHighMemoCryptoSetting(
            "HIGH_MEMO_AWS_KMS_KEYS_JSON",
            "contains an invalid or Region-mismatched pinned KMS key ARN".into(),
        ));
    }
    Ok(())
}

fn parse_high_search_config(
    vars: &HashMap<String, String>,
    authoritative_backend: AuthoritativeBackend,
    search_backend: SearchBackend,
) -> Result<HighSearchConfig, ConfigError> {
    let mode = vars
        .get("HIGH_SEARCH_MODE")
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_else(|| "disabled".to_string());

    match mode.as_str() {
        "disabled" => Ok(HighSearchConfig::Disabled),
        "aws-kms" => {
            if authoritative_backend != AuthoritativeBackend::MongoDb {
                return Err(ConfigError::HighSearchRequiresMongoDb);
            }
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
            let max_document_content_terms = parse_positive_high_search_setting::<usize>(
                vars,
                "HIGH_SEARCH_MAX_DOCUMENT_CONTENT_TERMS",
            )?;
            let max_query_content_terms = parse_positive_high_search_setting::<usize>(
                vars,
                "HIGH_SEARCH_MAX_QUERY_CONTENT_TERMS",
            )?;
            let max_normalized_term_bytes = parse_positive_high_search_setting::<usize>(
                vars,
                "HIGH_SEARCH_MAX_NORMALIZED_TERM_BYTES",
            )?;

            let config = HighSearchConfig::AwsKms {
                key_arn,
                region,
                provider_seed_version,
                cache_ttl_seconds,
                cache_max_entries,
                cache_sweep_seconds,
                max_document_content_terms,
                max_query_content_terms,
                max_normalized_term_bytes,
            };

            if !cfg!(feature = "aws-kms-search") {
                return Err(ConfigError::HighSearchBuildFeatureUnavailable);
            }

            Ok(config)
        }
        _ => Err(ConfigError::InvalidHighSearchMode(mode)),
    }
}

fn parse_high_search_shadow_config(
    vars: &HashMap<String, String>,
    high_search: &HighSearchConfig,
) -> Result<HighSearchShadowConfig, ConfigError> {
    let mode = vars
        .get("HIGH_SEARCH_SHADOW_MODE")
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_else(|| "disabled".to_string());

    match mode.as_str() {
        "disabled" => Ok(HighSearchShadowConfig::Disabled),
        "observe" => {
            if !matches!(high_search, HighSearchConfig::AwsKms { .. }) {
                return Err(ConfigError::HighSearchShadowRequiresHighSearch);
            }

            let max_concurrency = parse_positive_high_search_setting::<usize>(
                vars,
                "HIGH_SEARCH_SHADOW_MAX_CONCURRENCY",
            )?;
            let timeout_ms =
                parse_positive_high_search_setting::<u64>(vars, "HIGH_SEARCH_SHADOW_TIMEOUT_MS")?;

            if max_concurrency > MAX_HIGH_SEARCH_SHADOW_CONCURRENCY {
                return Err(ConfigError::InvalidHighSearchSetting(
                    "HIGH_SEARCH_SHADOW_MAX_CONCURRENCY",
                    max_concurrency.to_string(),
                ));
            }
            if timeout_ms > MAX_HIGH_SEARCH_SHADOW_TIMEOUT_MS {
                return Err(ConfigError::InvalidHighSearchSetting(
                    "HIGH_SEARCH_SHADOW_TIMEOUT_MS",
                    timeout_ms.to_string(),
                ));
            }

            Ok(HighSearchShadowConfig::Observe {
                max_concurrency,
                timeout_ms,
            })
        }
        _ => Err(ConfigError::InvalidHighSearchShadowMode(mode)),
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
    let partition_valid = parts.get(1).is_some_and(|partition| {
        !partition.is_empty()
            && partition
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            && !partition.starts_with('-')
            && !partition.ends_with('-')
    });
    let account_valid = parts.get(4).is_some_and(|account| {
        account.len() == 12 && account.bytes().all(|byte| byte.is_ascii_digit())
    });
    let resource_valid = parts.get(5).is_some_and(|resource| {
        resource.strip_prefix("key/").is_some_and(|key_id| {
            !key_id.is_empty()
                && !key_id.contains('/')
                && key_id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
    });

    let valid = key_arn.len() <= MAX_KMS_KEY_ARN_BYTES
        && key_arn.trim() == key_arn
        && parts.len() == 6
        && parts[0] == "arn"
        && partition_valid
        && parts[2] == "kms"
        && parts[3] == expected_region
        && account_valid
        && resource_valid;

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
