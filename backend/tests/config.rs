use memo_app_backend::config::{
    AppConfig, AuthConfig, AuthoritativeBackend, ConfigError, HighSearchConfig, SearchBackend,
};

fn development_vars() -> Vec<(String, String)> {
    vec![("AUTH_MODE".to_string(), "development".to_string())]
}

#[test]
fn uses_service_defaults_in_explicit_development_mode() {
    let config = AppConfig::from_vars(development_vars())
        .expect("explicit development configuration should be valid");

    assert_eq!(config.authoritative_backend, AuthoritativeBackend::Scylla);
    assert_eq!(config.authoritative_uri, "127.0.0.1:9042");
    assert_eq!(config.mongodb_database, "memo_app");
    assert_eq!(config.redis_uri, "redis://127.0.0.1:6379");
    assert_eq!(config.search_backend, SearchBackend::Elasticsearch);
    assert_eq!(config.search_uri, "http://127.0.0.1:9200");
    assert_eq!(config.high_search, HighSearchConfig::Disabled);
    assert_eq!(config.port, 8080);
    assert_eq!(config.auth, AuthConfig::Development);
}

#[test]
fn scylla_fallback_ignores_empty_mongodb_database() {
    let config = AppConfig::from_vars([
        ("AUTH_MODE".to_string(), "development".to_string()),
        ("MONGODB_DATABASE".to_string(), "   ".to_string()),
    ])
    .expect("unused MongoDB settings must not break Scylla fallback");

    assert_eq!(config.authoritative_backend, AuthoritativeBackend::Scylla);
}

#[test]
fn supports_explicit_mongodb_authoritative_backend() {
    let config = AppConfig::from_vars([
        ("AUTH_MODE".to_string(), "development".to_string()),
        ("AUTHORITATIVE_BACKEND".to_string(), "mongodb".to_string()),
        (
            "MONGODB_URI".to_string(),
            "mongodb://mongo.example.test:27017/?replicaSet=rs0".to_string(),
        ),
        ("MONGODB_DATABASE".to_string(), "memo_test".to_string()),
    ])
    .expect("MongoDB configuration should be valid");

    assert_eq!(config.authoritative_backend, AuthoritativeBackend::MongoDb);
    assert_eq!(
        config.authoritative_uri,
        "mongodb://mongo.example.test:27017/?replicaSet=rs0"
    );
    assert_eq!(config.mongodb_database, "memo_test");
}

#[test]
fn rejects_unknown_authoritative_backend() {
    let error = AppConfig::from_vars([
        ("AUTH_MODE".to_string(), "development".to_string()),
        ("AUTHORITATIVE_BACKEND".to_string(), "unknown".to_string()),
    ])
    .expect_err("unknown authoritative backend must be rejected");

    assert_eq!(
        error,
        ConfigError::InvalidAuthoritativeBackend("unknown".to_string())
    );
}

#[test]
fn rejects_empty_mongodb_database() {
    let error = AppConfig::from_vars([
        ("AUTH_MODE".to_string(), "development".to_string()),
        ("AUTHORITATIVE_BACKEND".to_string(), "mongodb".to_string()),
        ("MONGODB_DATABASE".to_string(), "   ".to_string()),
    ])
    .expect_err("empty MongoDB database must be rejected");

    assert_eq!(error, ConfigError::EmptyMongoDatabase);
}

#[test]
fn rejects_invalid_mongodb_database_name() {
    for invalid in ["memo.app", "memo app", "memo/app", "memo\\app", "memo$app"] {
        let error = AppConfig::from_vars([
            ("AUTH_MODE".to_string(), "development".to_string()),
            ("AUTHORITATIVE_BACKEND".to_string(), "mongodb".to_string()),
            ("MONGODB_DATABASE".to_string(), invalid.to_string()),
        ])
        .expect_err("invalid MongoDB database name must be rejected");

        assert_eq!(
            error,
            ConfigError::InvalidMongoDatabase(invalid.to_string())
        );
    }
}

#[test]
fn rejects_mongodb_database_name_at_64_bytes() {
    let invalid = "a".repeat(64);
    let error = AppConfig::from_vars([
        ("AUTH_MODE".to_string(), "development".to_string()),
        ("AUTHORITATIVE_BACKEND".to_string(), "mongodb".to_string()),
        ("MONGODB_DATABASE".to_string(), invalid.clone()),
    ])
    .expect_err("MongoDB database names must be shorter than 64 bytes");

    assert_eq!(error, ConfigError::InvalidMongoDatabase(invalid));
}

#[test]
fn supports_explicit_manticore_search_backend() {
    let config = AppConfig::from_vars([
        ("AUTH_MODE".to_string(), "development".to_string()),
        ("SEARCH_BACKEND".to_string(), "manticore".to_string()),
        (
            "MANTICORE_URL".to_string(),
            "http://manticore.example.test:9308".to_string(),
        ),
    ])
    .expect("Manticore configuration should be valid");

    assert_eq!(config.search_backend, SearchBackend::Manticore);
    assert_eq!(
        config.search_uri,
        "http://manticore.example.test:9308".to_string()
    );
}

#[test]
fn rejects_unknown_search_backend() {
    let error = AppConfig::from_vars([
        ("AUTH_MODE".to_string(), "development".to_string()),
        ("SEARCH_BACKEND".to_string(), "unknown".to_string()),
    ])
    .expect_err("unknown search backend must be rejected");

    assert_eq!(
        error,
        ConfigError::InvalidSearchBackend("unknown".to_string())
    );
}

fn high_search_aws_vars() -> Vec<(String, String)> {
    vec![
        ("AUTH_MODE".to_string(), "development".to_string()),
        ("SEARCH_BACKEND".to_string(), "manticore".to_string()),
        ("HIGH_SEARCH_MODE".to_string(), "aws-kms".to_string()),
        (
            "HIGH_SEARCH_AWS_KMS_KEY_ARN".to_string(),
            "arn:aws:kms:ap-northeast-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab"
                .to_string(),
        ),
        (
            "HIGH_SEARCH_AWS_REGION".to_string(),
            "ap-northeast-1".to_string(),
        ),
        (
            "HIGH_SEARCH_SEED_VERSION".to_string(),
            "search-seed-v1".to_string(),
        ),
        (
            "HIGH_SEARCH_KEY_CACHE_TTL_SECONDS".to_string(),
            "60".to_string(),
        ),
        (
            "HIGH_SEARCH_KEY_CACHE_MAX_ENTRIES".to_string(),
            "512".to_string(),
        ),
        (
            "HIGH_SEARCH_KEY_CACHE_SWEEP_SECONDS".to_string(),
            "30".to_string(),
        ),
    ]
}

#[test]
fn high_search_is_disabled_by_default_and_ignores_staged_settings() {
    let config = AppConfig::from_vars([
        ("AUTH_MODE".to_string(), "development".to_string()),
        (
            "HIGH_SEARCH_AWS_KMS_KEY_ARN".to_string(),
            "alias/not-used".to_string(),
        ),
    ])
    .expect("staged HIGH search settings must not activate without an explicit mode");

    assert_eq!(config.high_search, HighSearchConfig::Disabled);
}

#[cfg(feature = "aws-kms-search")]
#[test]
fn accepts_complete_high_search_aws_kms_configuration() {
    let config = AppConfig::from_vars(high_search_aws_vars())
        .expect("complete staged AWS KMS HIGH search configuration should be valid");

    assert_eq!(
        config.high_search,
        HighSearchConfig::AwsKms {
            key_arn:
                "arn:aws:kms:ap-northeast-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab"
                    .to_string(),
            region: "ap-northeast-1".to_string(),
            provider_seed_version: "search-seed-v1".to_string(),
            cache_ttl_seconds: 60,
            cache_max_entries: 512,
            cache_sweep_seconds: 30,
        }
    );
}

#[cfg(not(feature = "aws-kms-search"))]
#[test]
fn high_search_aws_kms_rejects_binary_without_aws_kms_feature() {
    let error = AppConfig::from_vars(high_search_aws_vars())
        .expect_err("AWS KMS HIGH search must fail closed when the binary lacks its provider feature");

    assert_eq!(error, ConfigError::HighSearchBuildFeatureUnavailable);
}

#[test]
fn high_search_aws_kms_requires_manticore() {
    let mut vars = high_search_aws_vars();
    vars.retain(|(name, _)| name != "SEARCH_BACKEND");

    let error = AppConfig::from_vars(vars)
        .expect_err("HIGH protected search must not run against Elasticsearch");

    assert_eq!(error, ConfigError::HighSearchRequiresManticore);
}

#[test]
fn rejects_unknown_high_search_mode() {
    let error = AppConfig::from_vars([
        ("AUTH_MODE".to_string(), "development".to_string()),
        ("HIGH_SEARCH_MODE".to_string(), "magic".to_string()),
    ])
    .expect_err("unknown HIGH search modes must fail closed");

    assert_eq!(
        error,
        ConfigError::InvalidHighSearchMode("magic".to_string())
    );
}

#[test]
fn high_search_aws_kms_requires_every_security_setting() {
    for missing in [
        "HIGH_SEARCH_AWS_KMS_KEY_ARN",
        "HIGH_SEARCH_AWS_REGION",
        "HIGH_SEARCH_SEED_VERSION",
        "HIGH_SEARCH_KEY_CACHE_TTL_SECONDS",
        "HIGH_SEARCH_KEY_CACHE_MAX_ENTRIES",
        "HIGH_SEARCH_KEY_CACHE_SWEEP_SECONDS",
    ] {
        let mut vars = high_search_aws_vars();
        vars.retain(|(name, _)| name != missing);

        let error = AppConfig::from_vars(vars)
            .expect_err("enabled HIGH search must reject incomplete security settings");

        assert_eq!(error, ConfigError::MissingHighSearchSetting(missing));
    }
}

#[test]
fn high_search_aws_kms_rejects_aliases_bare_ids_and_region_mismatch() {
    for invalid in [
        "alias/memo-search",
        "1234abcd-12ab-34cd-56ef-1234567890ab",
        "arn:aws:kms:ap-northeast-1:111122223333:alias/memo-search",
        "arn:aws:kms:us-east-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab",
    ] {
        let mut vars = high_search_aws_vars();
        vars.iter_mut()
            .find(|(name, _)| name == "HIGH_SEARCH_AWS_KMS_KEY_ARN")
            .unwrap()
            .1 = invalid.to_string();

        let error = AppConfig::from_vars(vars)
            .expect_err("HIGH search KMS key identity must be pinned and Region-consistent");

        assert!(matches!(
            error,
            ConfigError::InvalidHighSearchSetting("HIGH_SEARCH_AWS_KMS_KEY_ARN", _)
        ));
    }
}

#[test]
fn high_search_aws_kms_rejects_invalid_region() {
    let mut vars = high_search_aws_vars();
    vars.iter_mut()
        .find(|(name, _)| name == "HIGH_SEARCH_AWS_REGION")
        .unwrap()
        .1 = "AP Northeast 1".to_string();

    let error = AppConfig::from_vars(vars).expect_err("invalid AWS Regions must be rejected");

    assert!(matches!(
        error,
        ConfigError::InvalidHighSearchSetting("HIGH_SEARCH_AWS_REGION", _)
    ));
}

#[test]
fn high_search_aws_kms_rejects_seed_versions_that_cannot_fit_final_generation() {
    let mut vars = high_search_aws_vars();
    vars.iter_mut()
        .find(|(name, _)| name == "HIGH_SEARCH_SEED_VERSION")
        .unwrap()
        .1 = "x".repeat(108);

    let error = AppConfig::from_vars(vars)
        .expect_err("provider seed versions must leave room for PRF and HKDF generation prefixes");

    assert!(matches!(
        error,
        ConfigError::InvalidHighSearchSetting("HIGH_SEARCH_SEED_VERSION", _)
    ));
}

#[test]
fn high_search_aws_kms_requires_positive_cache_bounds() {
    for (name, value) in [
        ("HIGH_SEARCH_KEY_CACHE_TTL_SECONDS", "0"),
        ("HIGH_SEARCH_KEY_CACHE_MAX_ENTRIES", "0"),
        ("HIGH_SEARCH_KEY_CACHE_SWEEP_SECONDS", "0"),
        ("HIGH_SEARCH_KEY_CACHE_TTL_SECONDS", "not-a-number"),
    ] {
        let mut vars = high_search_aws_vars();
        vars.iter_mut().find(|(key, _)| key == name).unwrap().1 = value.to_string();

        let error = AppConfig::from_vars(vars)
            .expect_err("HIGH search cache settings must be explicit positive integers");

        assert!(matches!(
            error,
            ConfigError::InvalidHighSearchSetting(setting, _) if setting == name
        ));
    }
}

#[test]
fn requires_explicit_auth_mode() {
    let error = AppConfig::from_vars(std::iter::empty::<(String, String)>())
        .expect_err("auth mode must never silently default");

    assert_eq!(error, ConfigError::MissingAuthMode);
}

#[test]
fn rejects_non_numeric_port() {
    let error = AppConfig::from_vars([
        ("AUTH_MODE".to_string(), "development".to_string()),
        ("PORT".to_string(), "not-a-port".to_string()),
    ])
    .expect_err("invalid port must be rejected");

    assert!(matches!(error, ConfigError::InvalidPort(value) if value == "not-a-port"));
}

#[test]
fn rejects_unknown_auth_mode() {
    let error = AppConfig::from_vars([("AUTH_MODE".to_string(), "trust-me".to_string())])
        .expect_err("unknown auth modes must be rejected");

    assert!(matches!(error, ConfigError::InvalidAuthMode(value) if value == "trust-me"));
}

#[test]
fn jwt_mode_requires_all_resource_server_settings() {
    let error = AppConfig::from_vars([("AUTH_MODE".to_string(), "jwt".to_string())])
        .expect_err("JWT issuer must be configured");

    assert_eq!(error, ConfigError::MissingJwtSetting("AUTH_ISSUER"));
}

#[test]
fn accepts_complete_jwt_resource_server_configuration() {
    let config = AppConfig::from_vars([
        ("AUTH_MODE".to_string(), "jwt".to_string()),
        (
            "AUTH_ISSUER".to_string(),
            "https://auth.memo.example.com".to_string(),
        ),
        ("AUTH_AUDIENCE".to_string(), "memo-api".to_string()),
        (
            "AUTH_JWKS_URI".to_string(),
            "https://auth.memo.example.com/.well-known/jwks.json".to_string(),
        ),
    ])
    .expect("complete JWT resource server configuration should be valid");

    assert_eq!(
        config.auth,
        AuthConfig::Jwt {
            issuer: "https://auth.memo.example.com".to_string(),
            audience: "memo-api".to_string(),
            jwks_uri: "https://auth.memo.example.com/.well-known/jwks.json".to_string(),
        }
    );
}
