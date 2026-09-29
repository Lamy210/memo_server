use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use chrono::Utc;
use jsonwebtoken::{
    decode, decode_header,
    errors::{Error as JwtError, ErrorKind},
    jwk::{AlgorithmParameters, EllipticCurve, Jwk, JwkSet, KeyOperations, PublicKeyUse},
    Algorithm, DecodingKey, Validation,
};
use reqwest::Client;
use serde::Deserialize;
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;

use crate::{
    config::{AuthConfig, JwtSignatureMode},
    error::{AppError, AppResult},
};

const JWT_CLOCK_SKEW_SECONDS: i64 = 30;
const JWKS_CACHE_TTL: Duration = Duration::from_secs(300);
const JWKS_STALE_IF_ERROR_TTL: Duration = Duration::from_secs(3600);
const JWKS_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const JWKS_FORCED_REFRESH_COOLDOWN: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct AuthenticatedIdentity {
    pub user_id: Uuid,
}

pub struct AuthService {
    backend: AuthBackend,
}

enum AuthBackend {
    Development,
    Jwt(Box<JwtVerifier>),
}

impl AuthService {
    pub fn new(config: AuthConfig) -> Self {
        let backend = match config {
            AuthConfig::Development => AuthBackend::Development,
            AuthConfig::Jwt {
                issuer,
                audience,
                jwks_uri,
                signature_mode,
            } => AuthBackend::Jwt(Box::new(JwtVerifier::new(
                issuer,
                audience,
                jwks_uri,
                signature_mode,
            ))),
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

#[derive(Default)]
struct RefreshState {
    last_forced_attempt: Option<Instant>,
}

struct JwtVerifier {
    client: Client,
    issuer: String,
    audience: String,
    jwks_uri: String,
    signature_mode: JwtSignatureMode,
    jwks: RwLock<Option<CachedJwks>>,
    refresh_state: Mutex<RefreshState>,
}

impl JwtVerifier {
    fn new(
        issuer: String,
        audience: String,
        jwks_uri: String,
        signature_mode: JwtSignatureMode,
    ) -> Self {
        Self {
            client: Client::new(),
            issuer,
            audience,
            jwks_uri,
            signature_mode,
            jwks: RwLock::new(None),
            refresh_state: Mutex::new(RefreshState::default()),
        }
    }

    async fn verify(&self, token: &str) -> AppResult<AuthenticatedIdentity> {
        let header = decode_header(token)
            .map_err(|_| AppError::Unauthorized("Access token header is invalid".into()))?;

        let algorithm = header.alg;
        if !jwt_algorithm_allowed(self.signature_mode, algorithm) {
            return Err(AppError::Unauthorized(
                "Access token signature algorithm is not allowed".into(),
            ));
        }

        let kid = header
            .kid
            .as_deref()
            .ok_or_else(|| AppError::Unauthorized("Access token is missing kid".into()))?;

        let key = self.decoding_key(kid, algorithm, false).await?;
        match self.decode_claims(token, &key, algorithm) {
            Ok(identity) => Ok(identity),
            Err(ClaimsVerificationError::Jwt(error))
                if matches!(error.kind(), ErrorKind::InvalidSignature) =>
            {
                let refreshed_key = self.decoding_key(kid, algorithm, true).await?;
                self.decode_claims(token, &refreshed_key, algorithm)
                    .map_err(|_| AppError::Unauthorized("Access token is invalid".into()))
            }
            Err(_) => Err(AppError::Unauthorized("Access token is invalid".into())),
        }
    }

    fn decode_claims(
        &self,
        token: &str,
        key: &DecodingKey,
        algorithm: Algorithm,
    ) -> Result<AuthenticatedIdentity, ClaimsVerificationError> {
        let mut validation = Validation::new(algorithm);
        validation.leeway = JWT_CLOCK_SKEW_SECONDS as u64;
        validation.validate_nbf = true;
        validation.set_audience(&[self.audience.as_str()]);
        validation.set_issuer(&[self.issuer.as_str()]);
        validation.set_required_spec_claims(&["exp", "iat", "iss", "aud", "sub"]);

        let claims = decode::<AccessTokenClaims>(token, key, &validation)?.claims;
        claims.try_into()
    }

    async fn decoding_key(
        &self,
        kid: &str,
        algorithm: Algorithm,
        force_refresh: bool,
    ) -> AppResult<DecodingKey> {
        let mut set = self.jwks(force_refresh).await?;

        if set.find(kid).is_none() && !force_refresh {
            set = self.jwks(true).await?;
        }

        let jwk = set.find(kid).ok_or_else(|| {
            AppError::Unauthorized("Access token signing key is not recognized".into())
        })?;
        validate_jwk_for_algorithm(jwk, algorithm)?;

        DecodingKey::from_jwk(jwk).map_err(|error| {
            log::error!("Failed to build decoding key from JWKS: {error}");
            AppError::ServiceUnavailable("Authentication key set is invalid".into())
        })
    }

    async fn jwks(&self, force_refresh: bool) -> AppResult<Arc<JwkSet>> {
        if !force_refresh {
            if let Some((set, fetched_at)) = self.cached_jwks().await {
                if fetched_at.elapsed() < JWKS_CACHE_TTL {
                    return Ok(set);
                }
            }
        }

        // Serialize refreshes so concurrent cache misses or attacker-controlled
        // unknown kids cannot fan out into one JWKS request per API request.
        let mut refresh_state = self.refresh_state.lock().await;
        let cached = self.cached_jwks().await;

        // Another waiter may have refreshed the cache while this task was
        // waiting for the refresh lock.
        if !force_refresh {
            if let Some((set, fetched_at)) = &cached {
                if fetched_at.elapsed() < JWKS_CACHE_TTL {
                    return Ok(set.clone());
                }
            }
        } else {
            let now = Instant::now();
            if !forced_refresh_allowed(refresh_state.last_forced_attempt, now) {
                log::debug!("Skipping forced JWKS refresh during refresh cooldown");
                if let Some((set, _)) = cached {
                    return Ok(set);
                }
                return Err(AppError::ServiceUnavailable(
                    "Authentication key refresh is temporarily throttled".into(),
                ));
            }
            // Record attempts, not only successes, so an unavailable JWKS
            // endpoint cannot be hammered through repeated forced refreshes.
            refresh_state.last_forced_attempt = Some(now);
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
                if let Some((set, fetched_at)) = cached {
                    if fetched_at.elapsed() < JWKS_STALE_IF_ERROR_TTL {
                        log::warn!(
                            "JWKS refresh failed; temporarily using stale cached keys: {error}"
                        );
                        return Ok(set);
                    }
                    log::warn!(
                        "JWKS refresh failed and cached keys exceeded the stale-if-error window"
                    );
                }
                Err(error)
            }
            Err(error) => Err(error),
        }
    }

    async fn cached_jwks(&self) -> Option<(Arc<JwkSet>, Instant)> {
        let guard = self.jwks.read().await;
        guard
            .as_ref()
            .map(|cached| (cached.set.clone(), cached.fetched_at))
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

fn jwt_algorithm_allowed(mode: JwtSignatureMode, algorithm: Algorithm) -> bool {
    match mode {
        JwtSignatureMode::Rs256 => algorithm == Algorithm::RS256,
        JwtSignatureMode::Rs256Es384 => {
            matches!(algorithm, Algorithm::RS256 | Algorithm::ES384)
        }
        JwtSignatureMode::Es384 => algorithm == Algorithm::ES384,
    }
}

fn validate_jwk_for_algorithm(jwk: &Jwk, algorithm: Algorithm) -> AppResult<()> {
    if let Some(declared) = jwk.common.key_algorithm.clone() {
        let declared = Algorithm::try_from(declared).map_err(|_| {
            AppError::ServiceUnavailable(
                "Authentication JWKS contains an unsupported key algorithm".into(),
            )
        })?;
        if declared != algorithm {
            return Err(AppError::ServiceUnavailable(
                "Authentication JWKS key algorithm does not match the access token".into(),
            ));
        }
    }

    if let Some(public_key_use) = &jwk.common.public_key_use {
        if !matches!(public_key_use, PublicKeyUse::Signature) {
            return Err(AppError::ServiceUnavailable(
                "Authentication JWKS key is not intended for signature verification".into(),
            ));
        }
    }

    if let Some(operations) = &jwk.common.key_operations {
        if !operations
            .iter()
            .any(|operation| matches!(operation, KeyOperations::Verify))
        {
            return Err(AppError::ServiceUnavailable(
                "Authentication JWKS key does not permit signature verification".into(),
            ));
        }
    }

    let key_matches = match (algorithm, &jwk.algorithm) {
        (Algorithm::RS256, AlgorithmParameters::RSA(_)) => true,
        (Algorithm::ES384, AlgorithmParameters::EllipticCurve(parameters)) => {
            parameters.curve == EllipticCurve::P384
        }
        _ => false,
    };
    if !key_matches {
        return Err(AppError::ServiceUnavailable(
            "Authentication JWKS key type or curve does not match the access token algorithm"
                .into(),
        ));
    }

    Ok(())
}

fn forced_refresh_allowed(last_attempt: Option<Instant>, now: Instant) -> bool {
    match last_attempt {
        Some(last_attempt) => {
            now.saturating_duration_since(last_attempt) >= JWKS_FORCED_REFRESH_COOLDOWN
        }
        None => true,
    }
}

#[derive(Debug, Deserialize)]
struct AccessTokenClaims {
    sub: String,
    iat: i64,
}

impl TryFrom<AccessTokenClaims> for AuthenticatedIdentity {
    type Error = ClaimsVerificationError;

    fn try_from(claims: AccessTokenClaims) -> Result<Self, Self::Error> {
        let user_id =
            Uuid::parse_str(&claims.sub).map_err(|_| ClaimsVerificationError::InvalidIdentity)?;
        if claims.iat > Utc::now().timestamp() + JWT_CLOCK_SKEW_SECONDS {
            return Err(ClaimsVerificationError::InvalidIdentity);
        }

        Ok(Self { user_id })
    }
}

#[derive(Debug)]
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

    const TEST_ISSUER: &str = "https://auth.memo.test";
    const TEST_AUDIENCE: &str = "memo-api";
    const TEST_USER_ID: &str = "12345678-1234-4234-8234-123456789012";

    const TEST_RSA_MODULUS: &str = "tlnUM_RW7JKMCCHvVHJMiBP8EXJvvSiNVHgFUzBhNlubzwDDjOYo7MY6L1KZbfLnDVZAR_J5KpSwtChtROUyG0dLBuxHCb5GqC0wBQgl4meYQBAHavGUqh_eRKM6F7xugJcYDRTaEL7XvPK8LMYpx_NhImq39KQiPfF-BkB8GIinJE0rTJbPKzQa-Gao4jTd7sq3HKFdw6Inigq6NA1NbpPx-7wF-9L0mjLL-a_apkyhuIOrPn12LeROE-8mWPpOji0qMNg1fNOVrGlEzWUIOZmuvIiigM0y15IJU2LOl6NJ5U61QYjBEgW-nx8yHEIwjKzaeR_aSH2F7Zd1upoWoQ";
    const TEST_RSA_EXPONENT: &str = "AQAB";
    const TEST_EC_X: &str = "yaoQu-f0gTaWyBTD0BZ0r28LndWqh0Xn-rQOynEOkZrPkqxCLHf7Qnbv9K0TOAag";
    const TEST_EC_Y: &str = "0MIBkNuwwFXm0CqxaGYxjuGIwD_wKX9DqE16TFVnNcyHha9QVqZ8lhosVwKRKTt4";
    const VALID_ES384_TOKEN: &str = "eyJhbGciOiJFUzM4NCIsInR5cCI6IkpXVCIsImtpZCI6ImVjLXRlc3QifQ.eyJzdWIiOiIxMjM0NTY3OC0xMjM0LTQyMzQtODIzNC0xMjM0NTY3ODkwMTIiLCJpc3MiOiJodHRwczovL2F1dGgubWVtby50ZXN0IiwiYXVkIjoibWVtby1hcGkiLCJpYXQiOjE3MDAwMDAwMDAsImV4cCI6NDEwMjQ0NDgwMCwibmJmIjoxNzAwMDAwMDAwfQ.Hkju4wPXh0OU3FPNMJgIti1Gp08Ek1VD_ur1f9os7yc5N73q-KUZFg-PV-zipv0mJf9FBxTn1eEo-ofKxlDwf538cRYCnbC0MBodRfKm1skaA68uVf0hfgBt8axnHNjK";

    const VALID_TOKEN: &str = "eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3OC0xMjM0LTQyMzQtODIzNC0xMjM0NTY3ODkwMTIiLCJpc3MiOiJodHRwczovL2F1dGgubWVtby50ZXN0IiwiYXVkIjoibWVtby1hcGkiLCJpYXQiOjE3MDAwMDAwMDAsImV4cCI6NDEwMjQ0NDgwMCwibmJmIjoxNzAwMDAwMDAwfQ.PH4lN-zA2kKRaSPA2aWmt8lFiw8Wok3r2NKnXVwtQ2mNl4uOEQs3FcwhAsoaFyYsLwoqJOdvshtetqZIrUQFUzHI72Jilc1DDfnDkDG4RQOfcgs-T3wvrQpy8UjuWx1x70fwHaBLbhXhUrlCKa51jFTGN3Q5d57fMRFQzAQu1Y47QefykfgV0BUd1WNmeh9QKxKp0SDu53v6nV3S2EmHaARXjspOlyrCj4V0nju2niCQMDywdJS2kNYaTcZOQdvbac6aO1B-IPliaXAZ6_V4tjstmcUnSDBp2yszOqTNLpNC3vmHl25IZQUbb-ekSdbuZcvgqwweFbVB8wUZsG4HBw";
    const WRONG_AUDIENCE_TOKEN: &str = "eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3OC0xMjM0LTQyMzQtODIzNC0xMjM0NTY3ODkwMTIiLCJpc3MiOiJodHRwczovL2F1dGgubWVtby50ZXN0IiwiYXVkIjoib3RoZXItYXBpIiwiaWF0IjoxNzAwMDAwMDAwLCJleHAiOjQxMDI0NDQ4MDAsIm5iZiI6MTcwMDAwMDAwMH0.HHCNOau4DlsdRy4ZRCZ8ukNvJXAoJGyHlgkBb0EKUlvieHPauVNsDk8T0Wf6L0EDvS_G0gdylxd6UoD20HH68eX8xyLKrNk03y9p5F40wP-Ofir3mX-1Kufy-DSoKhu4RkBC2V3Qs7-lvNRkHRwWF1x1Ms7zE_O42sGDccqe6PIJnzoRMohNFllScrnYAzusgGgRfnLZM9j2ElmMiR0xJJsO48Lt-SjBPEWJUdFHZZ3zM5Jfx-HpmQBnQbfzZwPubDYhGN4zlrcpsIWI3tf3SZlbROqGnklYtAQVodTcV5awiBSXlTep1_duetc7MW2CPBrxgAtc2DWTUs_5fy6drg";
    const EXPIRED_TOKEN: &str = "eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3OC0xMjM0LTQyMzQtODIzNC0xMjM0NTY3ODkwMTIiLCJpc3MiOiJodHRwczovL2F1dGgubWVtby50ZXN0IiwiYXVkIjoibWVtby1hcGkiLCJpYXQiOjE1MDAwMDAwMDAsImV4cCI6MTYwMDAwMDAwMCwibmJmIjoxNTAwMDAwMDAwfQ.kBPL0cWfwAbdQ8rKYMWtOTfcyjooaZlXtyrtUCJNKTzdObRZgsryiBokjnsspuPwLlJuGWzf5kV-VfHcOKbYFnsriijPqCMEfyHTI9d_BPvKR2PSpXZN6eyTrJpQzl8SgNF0G6bTunXhM7qjwEh33gZjf3nMYJVxqWL_E7eOkPvBHYcNQh5m2R2PTVVr_q5uaBhLTESOgCVwKlBEGjoKq3aWrPW7kpdY6ocCXnMANKSbDsiy0pxw2fNGWtrLB_ADao6pLrZdZl_b32sJbsrka8uRKQ-pATa-9FAyKIIS-wvGVDfihuvxcujZmYE3wgPmnPy1L1suZvf3EYNaYD0lXA";
    const FUTURE_NBF_TOKEN: &str = "eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3OC0xMjM0LTQyMzQtODIzNC0xMjM0NTY3ODkwMTIiLCJpc3MiOiJodHRwczovL2F1dGgubWVtby50ZXN0IiwiYXVkIjoibWVtby1hcGkiLCJpYXQiOjE3MDAwMDAwMDAsImV4cCI6NDIwMDAwMDAwMCwibmJmIjo0MTAyNDQ0ODAwfQ.U-DvNnbNWaxK-XpXVI4yKqdOaUeuTPsjYoIxKeaDKAjaWHtGg2Ze5vkDDfmbr-GriLO7QxKZEJU3oll5J9I6DjXAyj_QaRR5kjoA_iOFZNfClaQ91w1bkdnrhfcgqliZO7OAinUWsk57sbeqEwlnqW387-M29Ac2qpdyXcBTt0zzgvMYAMtHfTaEj3iDLTTtV3E39ViRknMtBZSlXbb-6312MGaeQ5aquP-_KY6XtnqB4aMbgJwbIVtqe-t4-zJoOVpQR9ljDZwbbGjIrdn7WDByunfQRf6Ci7Jp9svDIRI5UOYaG_MV1tX2Ivo4EeuBmMBumWV4xHQg55H_MNMa6g";
    const FUTURE_IAT_TOKEN: &str = "eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3OC0xMjM0LTQyMzQtODIzNC0xMjM0NTY3ODkwMTIiLCJpc3MiOiJodHRwczovL2F1dGgubWVtby50ZXN0IiwiYXVkIjoibWVtby1hcGkiLCJpYXQiOjQxMDI0NDQ4MDAsImV4cCI6NDIwMDAwMDAwMCwibmJmIjoxNzAwMDAwMDAwfQ.rmdARz5KEmqAXtcBQGkvmY0ROwSyv8QBt8CIVv6bCtd_Kpk5VTzkkuNAXMBDqm5iRIXcqfg1kzUl9R6DztTrAsJAkex-migwhg7mU9P9I2iwZbI0ES7-nduW1Tau1AlXHdPLUHM6xiTIkZGeh9myQZP1HqJw8xACcRB9mVD-HNyHno9GRLz1n88y9nMjpvx2byDYz7G0vmpoDPQ6zzGhVzqeVpBYnulF6z0fisRVobDPlni_FHoLQsjW5V_7YW56S0-FR_-h4qmVxaFY8XRn-dxfpZQzcliATb1_lLCz7leqWqLeY9XW9VbFg25POdODjMkg2vtIv1rWznKkROdi5g";
    const INVALID_SUBJECT_TOKEN: &str = "eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJub3QtYS11dWlkIiwiaXNzIjoiaHR0cHM6Ly9hdXRoLm1lbW8udGVzdCIsImF1ZCI6Im1lbW8tYXBpIiwiaWF0IjoxNzAwMDAwMDAwLCJleHAiOjQxMDI0NDQ4MDAsIm5iZiI6MTcwMDAwMDAwMH0.i9Zp4kqvf4A90JiUoSSm3SqJ6TG0jfAua2zGivdvHyUOzOjoCRDO3uqQXHJmQnlxYBfGbghWxykFdm10gZw-bUFLQtPqSFRqwNw4vxwQUCmhsbiZiKpMkO80LVz2GiXn-_hm1WPUcBr5qJk--wGwO58zRHoXk-AF1j0tH4QvZ-1tCcua8bmbR8FdxcCPcdpfpYVcIUnunQvcnRKyHKyeENIrPF8yre7ArhziQcqyZtDjJGataBodvdvNek6Le27I9sRngopTeRzCU687L_awKDvTD4bEYpMilzh2pItJ4WpJizUrQbtF4PXoXAZqbdVabOXkikoZkolx6DEVy0x4uw";

    fn development_service() -> AuthService {
        AuthService::new(AuthConfig::Development)
    }

    fn verifier_with_mode(signature_mode: JwtSignatureMode) -> JwtVerifier {
        JwtVerifier::new(
            TEST_ISSUER.to_string(),
            TEST_AUDIENCE.to_string(),
            "https://unused.test/.well-known/jwks.json".to_string(),
            signature_mode,
        )
    }

    fn verifier() -> JwtVerifier {
        verifier_with_mode(JwtSignatureMode::Rs256)
    }

    fn rsa_key() -> DecodingKey {
        DecodingKey::from_rsa_components(TEST_RSA_MODULUS, TEST_RSA_EXPONENT)
            .expect("test RSA components must parse")
    }

    fn es384_key() -> DecodingKey {
        DecodingKey::from_ec_components(TEST_EC_X, TEST_EC_Y)
            .expect("test P-384 components must parse")
    }

    fn jwk(value: serde_json::Value) -> Jwk {
        serde_json::from_value(value).expect("test JWK must deserialize")
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
        let identity = verifier()
            .decode_claims(VALID_TOKEN, &rsa_key(), Algorithm::RS256)
            .expect("valid token must verify");

        assert_eq!(
            identity.user_id,
            Uuid::parse_str(TEST_USER_ID).expect("test UUID must parse")
        );
    }

    #[test]
    fn wrong_audience_is_rejected() {
        assert!(matches!(
            verifier().decode_claims(WRONG_AUDIENCE_TOKEN, &rsa_key(), Algorithm::RS256),
            Err(ClaimsVerificationError::Jwt(_))
        ));
    }

    #[test]
    fn expired_token_is_rejected() {
        assert!(matches!(
            verifier().decode_claims(EXPIRED_TOKEN, &rsa_key(), Algorithm::RS256),
            Err(ClaimsVerificationError::Jwt(_))
        ));
    }

    #[test]
    fn future_not_before_is_rejected() {
        assert!(matches!(
            verifier().decode_claims(FUTURE_NBF_TOKEN, &rsa_key(), Algorithm::RS256),
            Err(ClaimsVerificationError::Jwt(_))
        ));
    }

    #[test]
    fn future_issued_at_is_rejected() {
        assert!(matches!(
            verifier().decode_claims(FUTURE_IAT_TOKEN, &rsa_key(), Algorithm::RS256),
            Err(ClaimsVerificationError::InvalidIdentity)
        ));
    }

    #[test]
    fn access_token_subject_must_be_uuid() {
        assert!(matches!(
            verifier().decode_claims(INVALID_SUBJECT_TOKEN, &rsa_key(), Algorithm::RS256),
            Err(ClaimsVerificationError::InvalidIdentity)
        ));
    }

    #[test]
    fn signature_modes_accept_only_the_declared_migration_algorithms() {
        assert!(jwt_algorithm_allowed(
            JwtSignatureMode::Rs256,
            Algorithm::RS256
        ));
        assert!(!jwt_algorithm_allowed(
            JwtSignatureMode::Rs256,
            Algorithm::ES384
        ));

        assert!(jwt_algorithm_allowed(
            JwtSignatureMode::Rs256Es384,
            Algorithm::RS256
        ));
        assert!(jwt_algorithm_allowed(
            JwtSignatureMode::Rs256Es384,
            Algorithm::ES384
        ));
        assert!(!jwt_algorithm_allowed(
            JwtSignatureMode::Rs256Es384,
            Algorithm::ES256
        ));
        assert!(!jwt_algorithm_allowed(
            JwtSignatureMode::Rs256Es384,
            Algorithm::RS384
        ));

        assert!(jwt_algorithm_allowed(
            JwtSignatureMode::Es384,
            Algorithm::ES384
        ));
        assert!(!jwt_algorithm_allowed(
            JwtSignatureMode::Es384,
            Algorithm::RS256
        ));
    }

    #[test]
    fn valid_es384_access_token_is_accepted() {
        let identity = verifier_with_mode(JwtSignatureMode::Es384)
            .decode_claims(VALID_ES384_TOKEN, &es384_key(), Algorithm::ES384)
            .expect("valid ES384 token must verify");

        assert_eq!(
            identity.user_id,
            Uuid::parse_str(TEST_USER_ID).expect("test UUID must parse")
        );
    }

    #[test]
    fn jwk_metadata_must_match_rs256_or_es384_verification_contract() {
        let rsa = jwk(serde_json::json!({
            "kty": "RSA",
            "use": "sig",
            "key_ops": ["verify"],
            "alg": "RS256",
            "kid": "rsa",
            "n": TEST_RSA_MODULUS,
            "e": TEST_RSA_EXPONENT
        }));
        assert!(validate_jwk_for_algorithm(&rsa, Algorithm::RS256).is_ok());
        assert!(validate_jwk_for_algorithm(&rsa, Algorithm::ES384).is_err());

        let p384 = jwk(serde_json::json!({
            "kty": "EC",
            "use": "sig",
            "key_ops": ["verify"],
            "alg": "ES384",
            "kid": "ec",
            "crv": "P-384",
            "x": TEST_EC_X,
            "y": TEST_EC_Y
        }));
        assert!(validate_jwk_for_algorithm(&p384, Algorithm::ES384).is_ok());
        assert!(validate_jwk_for_algorithm(&p384, Algorithm::RS256).is_err());

        let wrong_curve = jwk(serde_json::json!({
            "kty": "EC",
            "use": "sig",
            "key_ops": ["verify"],
            "alg": "ES384",
            "kid": "wrong-curve",
            "crv": "P-256",
            "x": TEST_EC_X,
            "y": TEST_EC_Y
        }));
        assert!(validate_jwk_for_algorithm(&wrong_curve, Algorithm::ES384).is_err());
    }

    #[test]
    fn jwk_encryption_use_or_non_verify_operations_are_rejected() {
        let encryption_use = jwk(serde_json::json!({
            "kty": "RSA",
            "use": "enc",
            "alg": "RS256",
            "kid": "enc",
            "n": TEST_RSA_MODULUS,
            "e": TEST_RSA_EXPONENT
        }));
        assert!(validate_jwk_for_algorithm(&encryption_use, Algorithm::RS256).is_err());

        let signing_only = jwk(serde_json::json!({
            "kty": "RSA",
            "key_ops": ["sign"],
            "alg": "RS256",
            "kid": "sign-only",
            "n": TEST_RSA_MODULUS,
            "e": TEST_RSA_EXPONENT
        }));
        assert!(validate_jwk_for_algorithm(&signing_only, Algorithm::RS256).is_err());
    }

    #[test]
    fn forced_jwks_refresh_is_throttled_within_cooldown() {
        let first_attempt = Instant::now();

        assert!(forced_refresh_allowed(None, first_attempt));
        assert!(!forced_refresh_allowed(
            Some(first_attempt),
            first_attempt + JWKS_FORCED_REFRESH_COOLDOWN - Duration::from_millis(1)
        ));
        assert!(forced_refresh_allowed(
            Some(first_attempt),
            first_attempt + JWKS_FORCED_REFRESH_COOLDOWN
        ));
    }
}
