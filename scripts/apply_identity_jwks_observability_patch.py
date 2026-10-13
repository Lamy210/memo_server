from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
AUTH_RS = ROOT / "backend" / "src" / "infrastructure" / "auth.rs"
source = AUTH_RS.read_text(encoding="utf-8")


def replace_once(old: str, new: str) -> None:
    global source
    count = source.count(old)
    if count != 1:
        raise SystemExit(f"expected exactly one patch target, found {count}: {old[:80]!r}")
    source = source.replace(old, new, 1)


replace_once(
    """use crate::{
    config::{AuthConfig, JwtAccessTokenTypeMode, JwtSignatureMode},
    error::{AppError, AppResult},
};
""",
    """use crate::{
    config::{AuthConfig, JwtAccessTokenTypeMode, JwtSignatureMode},
    error::{AppError, AppResult},
    infrastructure::auth_observability::{
        observe_jwks_refresh, JwksRefreshOutcome, JwksRefreshTrigger,
    },
};
""",
)

replace_once(
    """#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct JwksCachePolicy {
    fresh_ttl: Duration,
    stale_if_error_ttl: Duration,
}
""",
    """#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct JwksCachePolicy {
    fresh_ttl: Duration,
    stale_if_error_ttl: Duration,
}

struct JwksFetchError {
    outcome: JwksRefreshOutcome,
    error: AppError,
}
""",
)

replace_once(
    """            .build()
            .map_err(|error| {
                log::error!(\"Failed to build authentication JWKS HTTP client: {error}\");
                AppError::ServiceUnavailable(
                    \"Authentication key client could not be initialized\".into(),
                )
            })?;
""",
    """            .build()
            .map_err(|_| {
                log::error!(\"Failed to build authentication JWKS HTTP client\");
                AppError::ServiceUnavailable(
                    \"Authentication key client could not be initialized\".into(),
                )
            })?;
""",
)

replace_once(
    """        let key = self.decoding_key(kid, algorithm, false).await?;
""",
    """        let key = self.decoding_key(kid, algorithm, None).await?;
""",
)

replace_once(
    """                let refreshed_key = self.decoding_key(kid, algorithm, true).await?;
""",
    """                let refreshed_key = self
                    .decoding_key(
                        kid,
                        algorithm,
                        Some(JwksRefreshTrigger::InvalidSignature),
                    )
                    .await?;
""",
)

replace_once(
    """    async fn decoding_key(
        &self,
        kid: &str,
        algorithm: Algorithm,
        force_refresh: bool,
    ) -> AppResult<DecodingKey> {
        let mut set = self.jwks(force_refresh).await?;

        if select_unique_jwk(&set, kid)?.is_none() && !force_refresh {
            set = self.jwks(true).await?;
        }

        let jwk = select_unique_jwk(&set, kid)?.ok_or_else(|| {
            AppError::Unauthorized(\"Access token signing key is not recognized\".into())
        })?;
        validate_jwk_for_algorithm(jwk, algorithm)?;

        DecodingKey::from_jwk(jwk).map_err(|error| {
            log::error!(\"Failed to build decoding key from JWKS: {error}\");
            AppError::ServiceUnavailable(\"Authentication key set is invalid\".into())
        })
    }
""",
    """    async fn decoding_key(
        &self,
        kid: &str,
        algorithm: Algorithm,
        refresh_trigger: Option<JwksRefreshTrigger>,
    ) -> AppResult<DecodingKey> {
        let mut set = self.jwks(refresh_trigger).await?;

        if select_unique_jwk(&set, kid)?.is_none() && refresh_trigger.is_none() {
            set = self.jwks(Some(JwksRefreshTrigger::UnknownKid)).await?;
        }

        let jwk = select_unique_jwk(&set, kid)?.ok_or_else(|| {
            AppError::Unauthorized(\"Access token signing key is not recognized\".into())
        })?;
        validate_jwk_for_algorithm(jwk, algorithm)?;

        DecodingKey::from_jwk(jwk).map_err(|_| {
            log::error!(\"Failed to build decoding key from authentication JWKS\");
            AppError::ServiceUnavailable(\"Authentication key set is invalid\".into())
        })
    }
""",
)

replace_once(
    """    async fn jwks(&self, force_refresh: bool) -> AppResult<Arc<JwkSet>> {
        if !force_refresh {
            if let Some((set, fetched_at)) = self.cached_jwks().await {
                if fetched_at.elapsed() < self.jwks_cache_policy.fresh_ttl {
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
                if fetched_at.elapsed() < self.jwks_cache_policy.fresh_ttl {
                    return Ok(set.clone());
                }
            }
        } else {
            let now = Instant::now();
            if !forced_refresh_allowed(refresh_state.last_forced_attempt, now) {
                log::debug!(\"Skipping forced JWKS refresh during refresh cooldown\");
                if let Some((set, _)) = cached {
                    return Ok(set);
                }
                return Err(AppError::ServiceUnavailable(
                    \"Authentication key refresh is temporarily throttled\".into(),
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
                    if fetched_at.elapsed() < self.jwks_cache_policy.stale_if_error_ttl {
                        log::warn!(
                            \"JWKS refresh failed; temporarily using stale cached keys: {error}\"
                        );
                        return Ok(set);
                    }
                    log::warn!(
                        \"JWKS refresh failed and cached keys exceeded the stale-if-error window\"
                    );
                }
                Err(error)
            }
            Err(error) => Err(error),
        }
    }
""",
    """    async fn jwks(
        &self,
        refresh_trigger: Option<JwksRefreshTrigger>,
    ) -> AppResult<Arc<JwkSet>> {
        let force_refresh = refresh_trigger.is_some();
        if !force_refresh {
            if let Some((set, fetched_at)) = self.cached_jwks().await {
                if fetched_at.elapsed() < self.jwks_cache_policy.fresh_ttl {
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
                if fetched_at.elapsed() < self.jwks_cache_policy.fresh_ttl {
                    return Ok(set.clone());
                }
            }
        }

        let trigger = refresh_trigger.unwrap_or(JwksRefreshTrigger::Scheduled);
        if force_refresh {
            let now = Instant::now();
            if !forced_refresh_allowed(refresh_state.last_forced_attempt, now) {
                if let Some((set, _)) = cached {
                    return observe_jwks_refresh(
                        trigger,
                        JwksRefreshOutcome::Throttled,
                        Ok(set),
                    );
                }
                return observe_jwks_refresh(
                    trigger,
                    JwksRefreshOutcome::Throttled,
                    Err(AppError::ServiceUnavailable(
                        \"Authentication key refresh is temporarily throttled\".into(),
                    )),
                );
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
                observe_jwks_refresh(trigger, JwksRefreshOutcome::Success, Ok(set))
            }
            Err(fetch_error) => {
                if !force_refresh {
                    if let Some((set, fetched_at)) = cached {
                        if fetched_at.elapsed() < self.jwks_cache_policy.stale_if_error_ttl {
                            return observe_jwks_refresh(
                                trigger,
                                JwksRefreshOutcome::StaleFallback,
                                Ok(set),
                            );
                        }
                    }
                }
                observe_jwks_refresh(trigger, fetch_error.outcome, Err(fetch_error.error))
            }
        }
    }
""",
)

replace_once(
    """    async fn fetch_jwks(&self) -> AppResult<JwkSet> {
        let mut response = self
            .client
            .get(&self.jwks_uri)
            .timeout(JWKS_REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|error| {
                log::warn!(\"Failed to fetch authentication JWKS: {error}\");
                AppError::ServiceUnavailable(\"Authentication key service is unavailable\".into())
            })?;

        if !response.status().is_success() {
            log::warn!(
                \"Authentication JWKS endpoint returned non-success status {}\",
                response.status()
            );
            return Err(AppError::ServiceUnavailable(
                \"Authentication key service is unavailable\".into(),
            ));
        }

        if response
            .content_length()
            .is_some_and(|length| length > JWKS_MAX_BODY_BYTES as u64)
        {
            return Err(AppError::ServiceUnavailable(
                \"Authentication JWKS response exceeds the configured safety limit\".into(),
            ));
        }

        let mut body = Vec::with_capacity(
            response
                .content_length()
                .unwrap_or(0)
                .min(JWKS_MAX_BODY_BYTES as u64) as usize,
        );
        while let Some(chunk) = response.chunk().await.map_err(|error| {
            log::warn!(\"Authentication JWKS body read failed: {error}\");
            AppError::ServiceUnavailable(\"Authentication key service is unavailable\".into())
        })? {
            append_bounded_jwks_chunk(&mut body, &chunk)?;
        }

        let set = serde_json::from_slice::<JwkSet>(&body).map_err(|error| {
            log::error!(\"Authentication JWKS response could not be decoded: {error}\");
            AppError::ServiceUnavailable(\"Authentication key service returned invalid data\".into())
        })?;
        validate_jwks_set(&set)?;
        Ok(set)
    }
""",
    """    async fn fetch_jwks(&self) -> Result<JwkSet, JwksFetchError> {
        let mut response = self
            .client
            .get(&self.jwks_uri)
            .timeout(JWKS_REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|_| JwksFetchError {
                outcome: JwksRefreshOutcome::Unavailable,
                error: AppError::ServiceUnavailable(
                    \"Authentication key service is unavailable\".into(),
                ),
            })?;

        if !response.status().is_success() {
            return Err(JwksFetchError {
                outcome: JwksRefreshOutcome::Unavailable,
                error: AppError::ServiceUnavailable(
                    \"Authentication key service is unavailable\".into(),
                ),
            });
        }

        if response
            .content_length()
            .is_some_and(|length| length > JWKS_MAX_BODY_BYTES as u64)
        {
            return Err(JwksFetchError {
                outcome: JwksRefreshOutcome::InvalidResponse,
                error: AppError::ServiceUnavailable(
                    \"Authentication JWKS response exceeds the configured safety limit\".into(),
                ),
            });
        }

        let mut body = Vec::with_capacity(
            response
                .content_length()
                .unwrap_or(0)
                .min(JWKS_MAX_BODY_BYTES as u64) as usize,
        );
        while let Some(chunk) = response.chunk().await.map_err(|_| JwksFetchError {
            outcome: JwksRefreshOutcome::Unavailable,
            error: AppError::ServiceUnavailable(
                \"Authentication key service is unavailable\".into(),
            ),
        })? {
            append_bounded_jwks_chunk(&mut body, &chunk).map_err(|error| JwksFetchError {
                outcome: JwksRefreshOutcome::InvalidResponse,
                error,
            })?;
        }

        let set = serde_json::from_slice::<JwkSet>(&body).map_err(|_| JwksFetchError {
            outcome: JwksRefreshOutcome::InvalidResponse,
            error: AppError::ServiceUnavailable(
                \"Authentication key service returned invalid data\".into(),
            ),
        })?;
        validate_jwks_set(&set).map_err(|error| JwksFetchError {
            outcome: JwksRefreshOutcome::InvalidResponse,
            error,
        })?;
        Ok(set)
    }
""",
)

AUTH_RS.write_text(source, encoding="utf-8")
