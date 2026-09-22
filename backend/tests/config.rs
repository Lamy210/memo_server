use memo_app_backend::config::{
    AppConfig, AuthConfig, AuthoritativeBackend, ConfigError, SearchBackend,
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
        (
            "AUTHORITATIVE_BACKEND".to_string(),
            "mongodb".to_string(),
        ),
        ("MONGODB_DATABASE".to_string(), "   ".to_string()),
    ])
    .expect_err("empty MongoDB database must be rejected");

    assert_eq!(error, ConfigError::EmptyMongoDatabase);
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
