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
        Self {
            client: Client::new(),
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
            .timeout(JWKS_REQUEST_TIMEOUT)
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
    use chrono::Utc;
    use jsonwebtoken::{encode, EncodingKey, Header};
    use serde::Serialize;

    const TEST_ISSUER: &str = "https://auth.memo.test";
    const TEST_AUDIENCE: &str = "memo-api";

    const TEST_PRIVATE_KEY: &str = r#"-----BEGIN PRIVATE KEY-----
MIIEvwIBADANBgkqhkiG9w0BAQEFAASCBKkwggSlAgEAAoIBAQDwsmGP/sob7cun
H8wOG4P08vigQUmDtawJQPjaE5WCn5a4iSpoBsUNHJ5v6eC6kDZVQK+Rb98KIlgc
Zc+oARPdJMQvqx5Yd05yUHPkSnynMPu+XMTwemERwdaKBdWBibm5pUKLJJJkfIV3
cuXWUUksyoHwloiW58tGC0TbA1JtLLFnz3MdKyowRbXzcAmKcFkUFSNsmJljL0e+
TRiiGFgtQBpeTPuoJwfPBqoCaQKhGfChBGnHzp8eUY4OZ/xdmPig+wL8qKHqH8iy
TVyqqvPMIIEBi1dWzap21B0bV2NCde/3VWJ9ijp8nLGbCqt3IZHevigUP0XQfpuL
jda8MfPPAgMBAAECggEAJTxLSIXviUuLvlJ2dFZAXzP5T31aHJSNxS62cLIn5nm+
zNR3aXlmoYUkY4bIW8Q0i5LCtlqapAw1GkuLyN9FzefCq+cqfiAS1C9rBk2ZpBm5
UDU0yEj+XEti355QbcY7I6OTvEfPl9kForl1IecYTWQUnv4Cqmm4ciELKWCFr1q2
VuOn2+VG8TaYzX2gGTcXVxguDKQvglJscdd1uDMhJQZqEaFUCd/W4i45uZTH11QR
DkD0UIdy4eUKjrNkKuxVnF5ontYWbBcD8epuvOt4ptYHLEiWW5MAX/2+9P3WoXL9
JX54DRjmIKtrUj/8ARadNB4Mbp12VoWt7jLHEmeR4QKBgQD6TXdHU5lIvOQvMGqC
HSqtKIafC8sUl5uRt1pZypTeChLvc7z6+evxkB9dymFbinUlnbBg4gllbPWlUdbG
28H/NwtGaH8HYRfZNd/xL6eT6mF1KwLLaqQose7CXLe+s18+K/ob/VeGihIo4Sue
FX7VjZKjlLwX94PFAa4hYq08oQKBgQD2LPD9+2kUjrndN3CZv/y/Uf8hNlAyoW+b
6WHu/Xf0S1xbqSq4McztD99CBctYJz+EXgOvxu7j/BOX8TPmYKK0kFQ7qEGpIB4d
8sQWaoHAGlcabMDdH5NE3ZmrBZt5MmrGibPwzLUqW13UJPZws5hZsj0Dx9gyrvNG
2tvgS7pqbwKBgQCwuIbxng2IdIzq4FUinnMmJIm/uzTbyhq1a+3nnYczqYsq8t1H
mbLDL81li+DnH7+MGmSQUqbtrFtXKIvqhPfYOEXGpTqivCN5YXdGMy4u2fmLHx3u
/tD+RnpbUdkNVFl3bNc+ccUdIVim8iu4hlaxci5JPlb62O945bHKsn+7YQKBgQCh
Z1vmmmUOFnokYYoRNIBpjEBjrTGt0IzVw5HzWPrCEHsQmfypYfWDZMmzhwsI1Erf
5agzIpJUplzOXVXy8V8cVhj0OGA8nBNC/X21WMWTh3GeoLlfAanUGBr9t6J1Nyos
2/I/qmgJynfddRKjWA1GmgdJKElHCc/1n99T0zL5PwKBgQChhuC8J5326x0w/Ukv
SV5GWLQPq9PBrH51fsCdTQrlLti+gUAFBzG071GLSnmCZCM/rXQVpSRBhVdJ5f2l
6rAhPH7c/Aj9bVpEUy0pm+BK6BKUC0d/+mCIB9DpIHneHD5dmXC2vf8TFg0lr2kU
v/dOTGuiCKJBodwU65TTAMoWkA==
-----END PRIVATE KEY-----"#;

    const TEST_PUBLIC_KEY: &str = r#"-----BEGIN PUBLIC KEY-----
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEA8LJhj/7KG+3Lpx/MDhuD
9PL4oEFJg7WsCUD42hOVgp+WuIkqaAbFDRyeb+ngupA2VUCvkW/fCiJYHGXPqAET
3STEL6seWHdOclBz5Ep8pzD7vlzE8HphEcHWigXVgYm5uaVCiySSZHyFd3Ll1lFJ
LMqB8JaIlufLRgtE2wNSbSyxZ89zHSsqMEW183AJinBZFBUjbJiZYy9Hvk0YohhY
LUAaXkz7qCcHzwaqAmkCoRnwoQRpx86fHlGODmf8XZj4oPsC/Kih6h/Isk1cqqrz
zCCBAYtXVs2qdtQdG1djQnXv91VifYo6fJyxmwqrdyGR3r4oFD9F0H6bi43WvDHz
zwIDAQAB
-----END PUBLIC KEY-----"#;

    #[derive(Serialize)]
    struct TestClaims {
        sub: String,
        iss: String,
        aud: String,
        iat: i64,
        exp: i64,
        #[serde(skip_serializing_if = "Option::is_none")]
        nbf: Option<i64>,
    }

    fn development_service() -> AuthService {
        AuthService::new(AuthConfig {
            mode: AuthMode::Development,
            issuer: None,
            audience: None,
            jwks_uri: None,
        })
    }

    fn verifier() -> JwtVerifier {
        JwtVerifier::new(
            TEST_ISSUER.to_string(),
            TEST_AUDIENCE.to_string(),
            "https://unused.test/.well-known/jwks.json".to_string(),
        )
    }

    fn key() -> DecodingKey {
        DecodingKey::from_rsa_pem(TEST_PUBLIC_KEY.as_bytes()).expect("test public key must parse")
    }

    fn token(claims: &TestClaims) -> String {
        encode(
            &Header::new(Algorithm::RS256),
            claims,
            &EncodingKey::from_rsa_pem(TEST_PRIVATE_KEY.as_bytes())
                .expect("test private key must parse"),
        )
        .expect("test token must encode")
    }

    fn valid_claims() -> TestClaims {
        let now = Utc::now().timestamp();
        TestClaims {
            sub: Uuid::new_v4().to_string(),
            iss: TEST_ISSUER.to_string(),
            aud: TEST_AUDIENCE.to_string(),
            iat: now,
            exp: now + 300,
            nbf: Some(now - 1),
        }
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
    fn valid_signed_access_token_is_accepted() {
        let claims = valid_claims();
        let expected_user_id = Uuid::parse_str(&claims.sub).expect("test UUID must parse");

        let identity = verifier()
            .decode_claims(&token(&claims), &key())
            .expect("valid token must verify");

        assert_eq!(identity.user_id, expected_user_id);
    }

    #[test]
    fn wrong_audience_is_rejected() {
        let mut claims = valid_claims();
        claims.aud = "other-api".to_string();

        assert!(matches!(
            verifier().decode_claims(&token(&claims), &key()),
            Err(ClaimsVerificationError::Jwt(_))
        ));
    }

    #[test]
    fn expired_token_is_rejected() {
        let mut claims = valid_claims();
        let now = Utc::now().timestamp();
        claims.iat = now - 600;
        claims.exp = now - 60;

        assert!(matches!(
            verifier().decode_claims(&token(&claims), &key()),
            Err(ClaimsVerificationError::Jwt(_))
        ));
    }

    #[test]
    fn future_not_before_is_rejected() {
        let mut claims = valid_claims();
        claims.nbf = Some(Utc::now().timestamp() + 120);

        assert!(matches!(
            verifier().decode_claims(&token(&claims), &key()),
            Err(ClaimsVerificationError::Jwt(_))
        ));
    }

    #[test]
    fn access_token_subject_must_be_uuid() {
        let mut claims = valid_claims();
        claims.sub = "not-a-uuid".to_string();

        assert!(matches!(
            verifier().decode_claims(&token(&claims), &key()),
            Err(ClaimsVerificationError::InvalidIdentity)
        ));
    }
}
