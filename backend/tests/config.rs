use memo_app_backend::config::{AppConfig, ConfigError};
use uuid::Uuid;

const DEVELOPMENT_USER_ID: &str = "12345678-1234-1234-1234-123456789012";

#[test]
fn uses_development_defaults_when_variables_are_missing() {
    let config = AppConfig::from_vars(std::iter::empty::<(String, String)>())
        .expect("default development configuration should be valid");

    assert_eq!(config.scylla_uri, "127.0.0.1:9042");
    assert_eq!(config.redis_uri, "redis://127.0.0.1:6379");
    assert_eq!(config.elasticsearch_uri, "http://127.0.0.1:9200");
    assert_eq!(config.port, 8080);
    assert_eq!(
        config.development_user_id,
        Uuid::parse_str(DEVELOPMENT_USER_ID).expect("test UUID must be valid")
    );
}

#[test]
fn rejects_non_numeric_port() {
    let error = AppConfig::from_vars([("PORT".to_string(), "not-a-port".to_string())])
        .expect_err("invalid port must be rejected");

    assert!(matches!(error, ConfigError::InvalidPort(value) if value == "not-a-port"));
}

#[test]
fn rejects_invalid_development_user_id() {
    let error = AppConfig::from_vars([(
        "DEVELOPMENT_USER_ID".to_string(),
        "not-a-uuid".to_string(),
    )])
    .expect_err("invalid development user UUID must be rejected");

    assert!(matches!(
        error,
        ConfigError::InvalidDevelopmentUserId(value) if value == "not-a-uuid"
    ));
}
