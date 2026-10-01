use memo_app_backend::config::{
    AppConfig, AuthConfig, AuthoritativeBackend, ConfigError, HighMemoCryptoConfig,
    HighSearchConfig, HighSearchShadowConfig, JwtAccessTokenTypeMode, JwtSignatureMode,
    SearchBackend,
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
    assert_eq!(config.high_memo_crypto, HighMemoCryptoConfig::Disabled);
    assert_eq!(config.high_search, HighSearchConfig::Disabled);
    assert_eq!(config.high_search_shadow, HighSearchShadowConfig::Disabled);
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

fn high_memo_aws_vars() -> Vec<(String, String)> {
    vec![
        ("AUTH_MODE".to_string(), "development".to_string()),
        ("AUTHORITATIVE_BACKEND".to_string(), "mongodb".to_string()),
        ("HIGH_MEMO_CRYPTO_MODE".to_string(), "aws-kms".to_string()),
        (
            "HIGH_MEMO_AWS_REGION".to_string(),
            "ap-northeast-1".to_string(),
        ),
        (
            "HIGH_MEMO_ACTIVE_KEY_VERSION".to_string(),
            "memo-key-v2".to_string(),
        ),
        (
            "HIGH_MEMO_AWS_KMS_KEYS_JSON".to_string(),
            r#"[
                {"key_version":"memo-key-v1","key_arn":"arn:aws:kms:ap-northeast-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab"},
                {"key_version":"memo-key-v2","key_arn":"arn:aws:kms:ap-northeast-1:111122223333:key/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"}
            ]"#
            .to_string(),
        ),
    ]
}

#[test]
fn high_memo_crypto_is_disabled_by_default_and_ignores_staged_settings() {
    let config = AppConfig::from_vars([
        ("AUTH_MODE".to_string(), "development".to_string()),
        (
            "HIGH_MEMO_AWS_KMS_KEYS_JSON".to_string(),
            "not-json-and-not-active".to_string(),
        ),
    ])
    .expect("staged HIGH memo settings must not activate without an explicit mode");

    assert_eq!(config.high_memo_crypto, HighMemoCryptoConfig::Disabled);
}

#[cfg(feature = "aws-kms-memo")]
#[test]
fn accepts_complete_high_memo_aws_kms_configuration() {
    let config = AppConfig::from_vars(high_memo_aws_vars())
        .expect("complete staged AWS KMS HIGH memo configuration should be valid");

    let HighMemoCryptoConfig::AwsKms {
        region,
        active_key_version,
        key_versions,
    } = config.high_memo_crypto
    else {
        panic!("HIGH memo crypto should be AWS KMS");
    };

    assert_eq!(region, "ap-northeast-1");
    assert_eq!(active_key_version, "memo-key-v2");
    assert_eq!(key_versions.len(), 2);
    assert_eq!(
        key_versions.get("memo-key-v1").map(String::as_str),
        Some("arn:aws:kms:ap-northeast-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab")
    );
}

#[cfg(not(feature = "aws-kms-memo"))]
#[test]
fn high_memo_aws_kms_rejects_binary_without_provider_feature() {
    let error = AppConfig::from_vars(high_memo_aws_vars())
        .expect_err("AWS KMS HIGH memo config must fail closed without aws-kms-memo");

    assert_eq!(error, ConfigError::HighMemoCryptoBuildFeatureUnavailable);
}

#[test]
fn high_memo_aws_kms_requires_mongodb_authoritative_storage() {
    let mut vars = high_memo_aws_vars();
    vars.retain(|(name, _)| name != "AUTHORITATIVE_BACKEND");

    let error = AppConfig::from_vars(vars)
        .expect_err("HIGH memo staging must not use the Scylla authoritative fallback");

    assert_eq!(error, ConfigError::HighMemoCryptoRequiresMongoDb);
}

#[test]
fn rejects_unknown_high_memo_crypto_mode() {
    let error = AppConfig::from_vars([
        ("AUTH_MODE".to_string(), "development".to_string()),
        ("HIGH_MEMO_CRYPTO_MODE".to_string(), "magic".to_string()),
    ])
    .expect_err("unknown HIGH memo crypto modes must fail closed");

    assert_eq!(
        error,
        ConfigError::InvalidHighMemoCryptoMode("magic".to_string())
    );
}

#[test]
fn high_memo_aws_kms_requires_every_setting() {
    for missing in [
        "HIGH_MEMO_AWS_REGION",
        "HIGH_MEMO_ACTIVE_KEY_VERSION",
        "HIGH_MEMO_AWS_KMS_KEYS_JSON",
    ] {
        let mut vars = high_memo_aws_vars();
        vars.retain(|(name, _)| name != missing);

        let error = AppConfig::from_vars(vars)
            .expect_err("enabled HIGH memo crypto must reject incomplete configuration");

        assert_eq!(error, ConfigError::MissingHighMemoCryptoSetting(missing));
    }
}

#[test]
fn high_memo_aws_kms_rejects_invalid_region_aliases_and_arns() {
    let mut invalid_region = high_memo_aws_vars();
    invalid_region
        .iter_mut()
        .find(|(name, _)| name == "HIGH_MEMO_AWS_REGION")
        .unwrap()
        .1 = "AP Northeast 1".to_string();
    assert!(matches!(
        AppConfig::from_vars(invalid_region),
        Err(ConfigError::InvalidHighMemoCryptoSetting(
            "HIGH_MEMO_AWS_REGION",
            _
        ))
    ));

    let mut invalid_active = high_memo_aws_vars();
    invalid_active
        .iter_mut()
        .find(|(name, _)| name == "HIGH_MEMO_ACTIVE_KEY_VERSION")
        .unwrap()
        .1 = "provider/key/arn".to_string();
    assert!(matches!(
        AppConfig::from_vars(invalid_active),
        Err(ConfigError::InvalidHighMemoCryptoSetting(
            "HIGH_MEMO_ACTIVE_KEY_VERSION",
            _
        ))
    ));

    let mut region_mismatch = high_memo_aws_vars();
    region_mismatch
        .iter_mut()
        .find(|(name, _)| name == "HIGH_MEMO_AWS_KMS_KEYS_JSON")
        .unwrap()
        .1 = r#"[{"key_version":"memo-key-v2","key_arn":"arn:aws:kms:us-east-1:111122223333:key/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"}]"#.to_string();
    assert!(matches!(
        AppConfig::from_vars(region_mismatch),
        Err(ConfigError::InvalidHighMemoCryptoSetting(
            "HIGH_MEMO_AWS_KMS_KEYS_JSON",
            _
        ))
    ));
}

#[test]
fn high_memo_aws_kms_rejects_malformed_duplicate_or_incomplete_key_rings() {
    for raw in [
        "not-json",
        "[]",
        r#"[{"key_version":"memo-key-v2","key_arn":"arn:aws:kms:ap-northeast-1:111122223333:key/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee","unexpected":true}]"#,
        r#"[
            {"key_version":"memo-key-v2","key_arn":"arn:aws:kms:ap-northeast-1:111122223333:key/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"},
            {"key_version":"memo-key-v2","key_arn":"arn:aws:kms:ap-northeast-1:111122223333:key/bbbbbbbb-cccc-dddd-eeee-ffffffffffff"}
        ]"#,
        r#"[
            {"key_version":"memo-key-v1","key_arn":"arn:aws:kms:ap-northeast-1:111122223333:key/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"},
            {"key_version":"memo-key-v2","key_arn":"arn:aws:kms:ap-northeast-1:111122223333:key/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"}
        ]"#,
    ] {
        let mut vars = high_memo_aws_vars();
        vars.iter_mut()
            .find(|(name, _)| name == "HIGH_MEMO_AWS_KMS_KEYS_JSON")
            .unwrap()
            .1 = raw.to_string();

        assert!(matches!(
            AppConfig::from_vars(vars),
            Err(ConfigError::InvalidHighMemoCryptoSetting(
                "HIGH_MEMO_AWS_KMS_KEYS_JSON",
                _
            ))
        ));
    }

    let mut missing_active = high_memo_aws_vars();
    missing_active
        .iter_mut()
        .find(|(name, _)| name == "HIGH_MEMO_AWS_KMS_KEYS_JSON")
        .unwrap()
        .1 = r#"[{"key_version":"memo-key-v1","key_arn":"arn:aws:kms:ap-northeast-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab"}]"#.to_string();

    assert!(matches!(
        AppConfig::from_vars(missing_active),
        Err(ConfigError::InvalidHighMemoCryptoSetting(
            "HIGH_MEMO_ACTIVE_KEY_VERSION",
            _
        ))
    ));
}

fn high_search_aws_vars() -> Vec<(String, String)> {
    vec![
        ("AUTH_MODE".to_string(), "development".to_string()),
        ("AUTHORITATIVE_BACKEND".to_string(), "mongodb".to_string()),
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
        (
            "HIGH_SEARCH_MAX_DOCUMENT_CONTENT_TERMS".to_string(),
            "2048".to_string(),
        ),
        (
            "HIGH_SEARCH_MAX_QUERY_CONTENT_TERMS".to_string(),
            "64".to_string(),
        ),
        (
            "HIGH_SEARCH_MAX_NORMALIZED_TERM_BYTES".to_string(),
            "256".to_string(),
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

#[test]
fn high_search_shadow_is_disabled_by_default() {
    let config = AppConfig::from_vars(development_vars())
        .expect("HIGH search shadow must remain disabled unless explicitly enabled");

    assert_eq!(config.high_search_shadow, HighSearchShadowConfig::Disabled);
}

#[test]
fn high_search_shadow_rejects_unknown_mode() {
    let error = AppConfig::from_vars([
        ("AUTH_MODE".to_string(), "development".to_string()),
        ("HIGH_SEARCH_SHADOW_MODE".to_string(), "magic".to_string()),
    ])
    .expect_err("unknown HIGH search shadow modes must fail closed");

    assert_eq!(
        error,
        ConfigError::InvalidHighSearchShadowMode("magic".to_string())
    );
}

#[test]
fn high_search_shadow_requires_enabled_high_search() {
    let error = AppConfig::from_vars([
        ("AUTH_MODE".to_string(), "development".to_string()),
        ("HIGH_SEARCH_SHADOW_MODE".to_string(), "observe".to_string()),
    ])
    .expect_err("shadow observation must not run without the protected HIGH runtime");

    assert_eq!(error, ConfigError::HighSearchShadowRequiresHighSearch);
}

#[cfg(feature = "aws-kms-search")]
#[test]
fn high_search_shadow_requires_explicit_positive_bounds() {
    let mut vars = high_search_aws_vars();
    vars.push(("HIGH_SEARCH_SHADOW_MODE".to_string(), "observe".to_string()));

    let error = AppConfig::from_vars(vars.clone())
        .expect_err("shadow observation requires explicit bounded concurrency");
    assert_eq!(
        error,
        ConfigError::MissingHighSearchSetting("HIGH_SEARCH_SHADOW_MAX_CONCURRENCY")
    );

    vars.push((
        "HIGH_SEARCH_SHADOW_MAX_CONCURRENCY".to_string(),
        "4".to_string(),
    ));
    let error = AppConfig::from_vars(vars.clone())
        .expect_err("shadow observation requires an explicit timeout");
    assert_eq!(
        error,
        ConfigError::MissingHighSearchSetting("HIGH_SEARCH_SHADOW_TIMEOUT_MS")
    );

    vars.push((
        "HIGH_SEARCH_SHADOW_TIMEOUT_MS".to_string(),
        "250".to_string(),
    ));
    let config =
        AppConfig::from_vars(vars).expect("complete HIGH search shadow bounds should be accepted");

    assert_eq!(
        config.high_search_shadow,
        HighSearchShadowConfig::Observe {
            max_concurrency: 4,
            timeout_ms: 250,
        }
    );
}

#[cfg(feature = "aws-kms-search")]
#[test]
fn high_search_shadow_rejects_excessive_resource_bounds() {
    for (name, value) in [
        ("HIGH_SEARCH_SHADOW_MAX_CONCURRENCY", "257"),
        ("HIGH_SEARCH_SHADOW_TIMEOUT_MS", "60001"),
    ] {
        let mut vars = high_search_aws_vars();
        vars.extend([
            ("HIGH_SEARCH_SHADOW_MODE".to_string(), "observe".to_string()),
            (
                "HIGH_SEARCH_SHADOW_MAX_CONCURRENCY".to_string(),
                "4".to_string(),
            ),
            (
                "HIGH_SEARCH_SHADOW_TIMEOUT_MS".to_string(),
                "250".to_string(),
            ),
        ]);
        vars.iter_mut().find(|(key, _)| key == name).unwrap().1 = value.to_string();

        let error = AppConfig::from_vars(vars)
            .expect_err("HIGH search shadow resource bounds must be capped");

        assert!(matches!(
            error,
            ConfigError::InvalidHighSearchSetting(setting, _) if setting == name
        ));
    }
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
            max_document_content_terms: 2048,
            max_query_content_terms: 64,
            max_normalized_term_bytes: 256,
        }
    );
}

#[cfg(not(feature = "aws-kms-search"))]
#[test]
fn high_search_aws_kms_rejects_binary_without_aws_kms_feature() {
    let error = AppConfig::from_vars(high_search_aws_vars()).expect_err(
        "AWS KMS HIGH search must fail closed when the binary lacks its provider feature",
    );

    assert_eq!(error, ConfigError::HighSearchBuildFeatureUnavailable);
}

#[test]
fn high_search_aws_kms_requires_mongodb_authoritative_storage() {
    let mut vars = high_search_aws_vars();
    vars.retain(|(name, _)| name != "AUTHORITATIVE_BACKEND");

    let error = AppConfig::from_vars(vars)
        .expect_err("HIGH protected search must not run on the Scylla migration fallback");

    assert_eq!(error, ConfigError::HighSearchRequiresMongoDb);
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
        "HIGH_SEARCH_MAX_DOCUMENT_CONTENT_TERMS",
        "HIGH_SEARCH_MAX_QUERY_CONTENT_TERMS",
        "HIGH_SEARCH_MAX_NORMALIZED_TERM_BYTES",
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
        "arn:AWS:kms:ap-northeast-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab",
        "arn:aws:kms:ap-northeast-1:not-an-account:key/1234abcd-12ab-34cd-56ef-1234567890ab",
        "arn:aws:kms:ap-northeast-1:111122223333:key/key/extra",
        " arn:aws:kms:ap-northeast-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab",
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
        ("HIGH_SEARCH_MAX_DOCUMENT_CONTENT_TERMS", "0"),
        ("HIGH_SEARCH_MAX_QUERY_CONTENT_TERMS", "0"),
        ("HIGH_SEARCH_MAX_NORMALIZED_TERM_BYTES", "0"),
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
        (
            "AUTH_ACCESS_TOKEN_MAX_LIFETIME_SECONDS".to_string(),
            "900".to_string(),
        ),
    ])
    .expect("complete JWT resource server configuration should be valid");

    assert_eq!(
        config.auth,
        AuthConfig::Jwt {
            issuer: "https://auth.memo.example.com".to_string(),
            audience: "memo-api".to_string(),
            jwks_uri: "https://auth.memo.example.com/.well-known/jwks.json".to_string(),
            signature_mode: JwtSignatureMode::Rs256,
            access_token_type_mode: JwtAccessTokenTypeMode::LegacyAny,
            max_access_token_lifetime_seconds: 900,
        }
    );
}

#[test]
fn jwt_mode_requires_explicit_access_token_lifetime_policy() {
    let error = AppConfig::from_vars([
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
    .expect_err("JWT mode must require an explicit maximum access-token lifetime");

    assert_eq!(
        error,
        ConfigError::MissingJwtSetting("AUTH_ACCESS_TOKEN_MAX_LIFETIME_SECONDS")
    );
}

#[test]
fn jwt_access_token_lifetime_policy_is_bounded() {
    for invalid in ["0", "59", "3601", "not-a-number"] {
        let error = AppConfig::from_vars([
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
            (
                "AUTH_ACCESS_TOKEN_MAX_LIFETIME_SECONDS".to_string(),
                invalid.to_string(),
            ),
        ])
        .expect_err("JWT access-token lifetime must stay within the resource-server policy");

        assert_eq!(
            error,
            ConfigError::InvalidJwtAccessTokenMaxLifetime(invalid.to_string())
        );
    }

    for valid in ["60", "900", "3600"] {
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
            (
                "AUTH_ACCESS_TOKEN_MAX_LIFETIME_SECONDS".to_string(),
                valid.to_string(),
            ),
        ])
        .expect("bounded JWT access-token lifetime should be accepted");

        let AuthConfig::Jwt {
            max_access_token_lifetime_seconds,
            ..
        } = config.auth
        else {
            panic!("JWT auth configuration expected");
        };
        assert_eq!(
            max_access_token_lifetime_seconds,
            valid.parse::<u64>().unwrap()
        );
    }
}

#[test]
fn jwt_issuer_requires_https_without_userinfo_query_or_fragment() {
    for invalid in [
        "http://auth.memo.example.com",
        "https://user:secret@auth.memo.example.com",
        "https://@auth.memo.example.com",
        "https://auth.memo.example.com?tenant=a",
        "https://auth.memo.example.com#issuer",
        "auth.memo.example.com",
    ] {
        let error = AppConfig::from_vars([
            ("AUTH_MODE".to_string(), "jwt".to_string()),
            ("AUTH_ISSUER".to_string(), invalid.to_string()),
            ("AUTH_AUDIENCE".to_string(), "memo-api".to_string()),
            (
                "AUTH_JWKS_URI".to_string(),
                "https://auth.memo.example.com/.well-known/jwks.json".to_string(),
            ),
            (
                "AUTH_ACCESS_TOKEN_MAX_LIFETIME_SECONDS".to_string(),
                "900".to_string(),
            ),
        ])
        .expect_err("unsafe OAuth issuer identifiers must fail closed");

        assert_eq!(error, ConfigError::InvalidJwtIssuer(invalid.to_string()));
    }

    let config = AppConfig::from_vars([
        ("AUTH_MODE".to_string(), "jwt".to_string()),
        (
            "AUTH_ISSUER".to_string(),
            "https://auth.memo.example.com/tenant-a".to_string(),
        ),
        ("AUTH_AUDIENCE".to_string(), "memo-api".to_string()),
        (
            "AUTH_JWKS_URI".to_string(),
            "https://auth.memo.example.com/.well-known/jwks.json".to_string(),
        ),
        (
            "AUTH_ACCESS_TOKEN_MAX_LIFETIME_SECONDS".to_string(),
            "900".to_string(),
        ),
    ])
    .expect("HTTPS issuer paths must remain valid for multi-tenant authorization servers");

    let AuthConfig::Jwt { issuer, .. } = config.auth else {
        panic!("JWT auth configuration expected");
    };
    assert_eq!(issuer, "https://auth.memo.example.com/tenant-a");
}

#[test]
fn jwt_jwks_uri_requires_https_without_userinfo_or_fragment() {
    for invalid in [
        "http://auth.memo.example.com/.well-known/jwks.json",
        "https://user:secret@auth.memo.example.com/.well-known/jwks.json",
        "https://@auth.memo.example.com/.well-known/jwks.json",
        "https://auth.memo.example.com/.well-known/jwks.json#keys",
        "/.well-known/jwks.json",
    ] {
        let error = AppConfig::from_vars([
            ("AUTH_MODE".to_string(), "jwt".to_string()),
            (
                "AUTH_ISSUER".to_string(),
                "https://auth.memo.example.com".to_string(),
            ),
            ("AUTH_AUDIENCE".to_string(), "memo-api".to_string()),
            ("AUTH_JWKS_URI".to_string(), invalid.to_string()),
        ])
        .expect_err("unsafe JWKS endpoints must fail closed");

        assert_eq!(error, ConfigError::InvalidJwtJwksUri(invalid.to_string()));
    }
}

#[test]
fn jwt_access_token_type_mode_is_legacy_compatible_by_default_and_supports_rfc9068() {
    let base = [
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
        (
            "AUTH_ACCESS_TOKEN_MAX_LIFETIME_SECONDS".to_string(),
            "900".to_string(),
        ),
    ];

    let config = AppConfig::from_vars(base.clone())
        .expect("omitted token type policy must preserve legacy compatibility");
    let AuthConfig::Jwt {
        access_token_type_mode,
        ..
    } = config.auth
    else {
        panic!("JWT auth configuration expected");
    };
    assert_eq!(access_token_type_mode, JwtAccessTokenTypeMode::LegacyAny);

    let config = AppConfig::from_vars(
        base.into_iter()
            .chain([("AUTH_JWT_TYPE_MODE".to_string(), "AT-JWT".to_string())]),
    )
    .expect("RFC 9068 token type mode should parse case-insensitively");
    let AuthConfig::Jwt {
        access_token_type_mode,
        ..
    } = config.auth
    else {
        panic!("JWT auth configuration expected");
    };
    assert_eq!(access_token_type_mode, JwtAccessTokenTypeMode::AtJwt);
}

#[test]
fn jwt_access_token_type_mode_rejects_unknown_profiles() {
    let error = AppConfig::from_vars([
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
        (
            "AUTH_ACCESS_TOKEN_MAX_LIFETIME_SECONDS".to_string(),
            "900".to_string(),
        ),
        ("AUTH_JWT_TYPE_MODE".to_string(), "id-jwt".to_string()),
    ])
    .expect_err("unknown JWT type profiles must fail closed");

    assert_eq!(
        error,
        ConfigError::InvalidJwtAccessTokenTypeMode("id-jwt".to_string())
    );
}

#[test]
fn jwt_signature_mode_supports_explicit_migration_and_es384_target() {
    for (value, expected) in [
        ("rs256-es384", JwtSignatureMode::Rs256Es384),
        ("es384", JwtSignatureMode::Es384),
        ("RS256-ES384", JwtSignatureMode::Rs256Es384),
    ] {
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
            (
                "AUTH_ACCESS_TOKEN_MAX_LIFETIME_SECONDS".to_string(),
                "900".to_string(),
            ),
            ("AUTH_JWT_SIGNATURE_MODE".to_string(), value.to_string()),
        ])
        .expect("supported JWT signature migration modes should parse");

        let AuthConfig::Jwt { signature_mode, .. } = config.auth else {
            panic!("JWT auth configuration expected");
        };
        assert_eq!(signature_mode, expected);
    }
}

#[test]
fn jwt_signature_mode_rejects_algorithm_sets_outside_the_migration_contract() {
    let error = AppConfig::from_vars([
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
        ("AUTH_JWT_SIGNATURE_MODE".to_string(), "any".to_string()),
    ])
    .expect_err("unsupported JWT algorithm policies must fail closed");

    assert_eq!(
        error,
        ConfigError::InvalidJwtSignatureMode("any".to_string())
    );
}
