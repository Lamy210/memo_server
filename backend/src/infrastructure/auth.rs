use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use jsonwebtoken::{
    decode, decode_header,
    errors::{Error as JwtError, ErrorKind},
    jwk::JwkSet,
    Algorithm, DecodingKey, Validation,
};
use reqwest::Client;
use serde::Deserialize;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::{
    config::{AuthConfig, AuthMode},
    error::{AppError, AppResult},
};

const JWKS_CACHE_TTL: Duration = Duration::from_secs(300);
const JWKS_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct AuthenticatedIdentity {
    pub user_id: Uuid,
}

pub struct AuthService {
    backend: AuthBackend,
}

enum AuthBackend {
    Development,
    Jwt(JwtVerifier),
}

impl AuthService {
    pub fn new(config: AuthConfig) -> Self {
        let backend = match config.mode {
            AuthMode::Development => AuthBackend::Development,
            AuthMode::Jwt => AuthBackend::Jwt(JwtVerifier::new(
                config
                    .issuer
                    .expect("validated JWT configuration must contain issuer"),
                config
                    .audience
                    .expect("validated JWT configuration must contain audience"),
                config
                    .jwks_uri
                    .expect("validated JWT configuration must contain JWKS URI"),
            )),
        };

        Self { backend }
    }

    pub async fn authenticate(
        &self,
        bearer_token: Option<&str>,
        development_user_id: Option<&str>,
    ) -> AppResult<AuthenticatedIdentity> {
        match &self.backend {
            AuthBackend::Development => authenticate_development_user(development_user_id),
            AuthBackend::Jwt(verifier) => {
                let token = bearer_token.ok_or_else(|| {
                    AppError::Unauthorized("Bearer access token is required".into())
                })?;
                verifier.verify(token).await
            }
        }
    }
}

fn authenticate_development_user(value: Option<&str>) -> AppResult<AuthenticatedIdentity> {
    let value = value.ok_or_else(|| {
        AppError::Unauthorized("X-Development-User-Id is required in development auth mode".into())
    })?;
    let user_id = Uuid::parse_str(value).map_err(|_| {
        AppError::Unauthorized("X-Development-User-Id must contain a valid UUID".into())
    })?;

    Ok(AuthenticatedIdentity { user_id })
}

struct CachedJwks {
    set: Arc<JwkSet>,
    fetched_at: Instant,
}

struct JwtVerifier {
    client: Client,
    issuer: String,
    audience: String,
    jwks_uri: String,
    jwks: RwLock<Option<CachedJwks>>,
}

impl JwtVerifier {
    fn new(issuer: String, audience: String, jwks_uri: String) -> Self {
        let client = Client::builder()
            .timeout(JWKS_REQUEST_TIMEOUT)
            .build()
            .expect("static authentication HTTP client configuration must be valid");

        Self {
            client,
            issuer,
            audience,
            jwks_uri,
            jwks: RwLock::new(None),
        }
    }

    async fn verify(&self, token: &str) -> AppResult<AuthenticatedIdentity> {
        let header = decode_header(token)
            .map_err(|_| AppError::Unauthorized("Access token header is invalid".into()))?;

        if header.alg != Algorithm::RS256 {
            return Err(AppError::Unauthorized("Access token must use RS256".into()));
        }

        let kid = header
            .kid
            .as_deref()
            .ok_or_else(|| AppError::Unauthorized("Access token is missing kid".into()))?;

        let key = self.decoding_key(kid, false).await?;
        match self.decode_claims(token, &key) {
            Ok(identity) => Ok(identity),
            Err(ClaimsVerificationError::Jwt(error))
                if matches!(error.kind(), ErrorKind::InvalidSignature) =>
            {
                let refreshed_key = self.decoding_key(kid, true).await?;
                self.decode_claims(token, &refreshed_key)
                    .map_err(|_| AppError::Unauthorized("Access token is invalid".into()))
            }
            Err(_) => Err(AppError::Unauthorized("Access token is invalid".into())),
        }
    }

    fn decode_claims(
        &self,
        token: &str,
        key: &DecodingKey,
    ) -> Result<AuthenticatedIdentity, ClaimsVerificationError> {
        let mut validation = Validation::new(Algorithm::RS256);
        validation.leeway = 30;
        validation.validate_nbf = true;
        validation.set_audience(&[self.audience.as_str()]);
        validation.set_issuer(&[self.issuer.as_str()]);
        validation.set_required_spec_claims(&["exp", "iat", "iss", "aud", "sub"]);

        let claims = decode::<AccessTokenClaims>(token, key, &validation)?.claims;
        claims.try_into()
    }

    async fn decoding_key(&self, kid: &str, force_refresh: bool) -> AppResult<DecodingKey> {
        let mut set = self.jwks(force_refresh).await?;

        if set.find(kid).is_none() && !force_refresh {
            set = self.jwks(true).await?;
        }

        let jwk = set.find(kid).ok_or_else(|| {
            AppError::Unauthorized("Access token signing key is not recognized".into())
        })?;

        DecodingKey::from_jwk(jwk).map_err(|error| {
            log::error!("Failed to build decoding key from JWKS: {error}");
            AppError::ServiceUnavailable("Authentication key set is invalid".into())
        })
    }

    async fn jwks(&self, force_refresh: bool) -> AppResult<Arc<JwkSet>> {
        let cached = {
            let guard = self.jwks.read().await;
            guard
                .as_ref()
                .map(|cached| (cached.set.clone(), cached.fetched_at))
        };

        if !force_refresh {
            if let Some((set, fetched_at)) = &cached {
                if fetched_at.elapsed() < JWKS_CACHE_TTL {
                    return Ok(set.clone());
                }
            }
        }

        match self.fetch_jwks().await {
            Ok(set) => {
                let set = Arc::new(set);
                *self.jwks.write().await = Some(CachedJwks {
                    set: set.clone(),
                    fetched_at: Instant::now(),
                });
                Ok(set)
            }
            Err(error) if !force_refresh => {
                if let Some((set, _)) = cached {
                    log::warn!(
                        "JWKS refresh failed; using cached keys until a forced refresh is needed: {error}"
                    );
                    return Ok(set);
                }
                Err(error)
            }
            Err(error) => Err(error),
        }
    }

    async fn fetch_jwks(&self) -> AppResult<JwkSet> {
        let response = self
            .client
            .get(&self.jwks_uri)
            .send()
            .await
            .map_err(|error| {
                log::warn!("Failed to fetch authentication JWKS: {error}");
                AppError::ServiceUnavailable("Authentication key service is unavailable".into())
            })?
            .error_for_status()
            .map_err(|error| {
                log::warn!("Authentication JWKS endpoint returned an error: {error}");
                AppError::ServiceUnavailable("Authentication key service is unavailable".into())
            })?;

        response.json::<JwkSet>().await.map_err(|error| {
            log::error!("Authentication JWKS response could not be decoded: {error}");
            AppError::ServiceUnavailable("Authentication key service returned invalid data".into())
        })
    }
}

#[derive(Debug, Deserialize)]
struct AccessTokenClaims {
    sub: String,
}

impl TryFrom<AccessTokenClaims> for AuthenticatedIdentity {
    type Error = ClaimsVerificationError;

    fn try_from(claims: AccessTokenClaims) -> Result<Self, Self::Error> {
        let user_id =
            Uuid::parse_str(&claims.sub).map_err(|_| ClaimsVerificationError::InvalidIdentity)?;

        Ok(Self { user_id })
    }
}

enum ClaimsVerificationError {
    Jwt(JwtError),
    InvalidIdentity,
}

impl From<JwtError> for ClaimsVerificationError {
    fn from(error: JwtError) -> Self {
        Self::Jwt(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn development_service() -> AuthService {
        AuthService::new(AuthConfig {
            mode: AuthMode::Development,
            issuer: None,
            audience: None,
            jwks_uri: None,
        })
    }

    #[tokio::test]
    async fn development_auth_requires_explicit_user_header() {
        let error = development_service()
            .authenticate(None, None)
            .await
            .expect_err("missing development identity must be rejected");

        assert!(matches!(error, AppError::Unauthorized(_)));
    }

    #[tokio::test]
    async fn development_auth_parses_user_uuid() {
        let user_id = Uuid::new_v4();

        let identity = development_service()
            .authenticate(None, Some(&user_id.to_string()))
            .await
            .expect("valid development identity should authenticate");

        assert_eq!(identity.user_id, user_id);
    }

    #[test]
    fn access_token_subject_must_be_uuid() {
        let claims = AccessTokenClaims {
            sub: "not-a-uuid".to_string(),
        };

        assert!(matches!(
            AuthenticatedIdentity::try_from(claims),
            Err(ClaimsVerificationError::InvalidIdentity)
        ));
    }
}
