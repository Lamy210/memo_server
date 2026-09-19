use memo_app_backend::config::{AppConfig, AuthConfig, ConfigError};

fn development_vars() -> Vec<(String, String)> {
    vec![("AUTH_MODE".to_string(), "development".to_string())]
}

#[test]
fn uses_service_defaults_in_explicit_development_mode() {
    let config = AppConfig::from_vars(development_vars())
        .expect("explicit development configuration should be valid");

    assert_eq!(config.scylla_uri, "127.0.0.1:9042");
    assert_eq!(config.redis_uri, "redis://127.0.0.1:6379");
    assert_eq!(config.elasticsearch_uri, "http://127.0.0.1:9200");
    assert_eq!(config.port, 8080);
    assert_eq!(config.auth, AuthConfig::Development);
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
