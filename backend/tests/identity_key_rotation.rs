mod config {
    pub use memo_app_backend::config::*;
}

mod error {
    pub use memo_app_backend::error::*;
}

mod infrastructure {
    pub(crate) mod auth_observability {
        use crate::error::AppResult;

        // The rotation regression includes the production verifier source so it
        // can exercise its private cache state. Keep only the observability
        // dependency surface here; observability behavior itself is covered by
        // the canonical module's dedicated unit tests.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub(crate) enum JwksRefreshTrigger {
            Scheduled,
            UnknownKid,
            InvalidSignature,
        }

        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub(crate) enum JwksRefreshOutcome {
            Success,
            StaleFallback,
            Throttled,
            Unavailable,
            InvalidResponse,
        }

        pub(crate) fn observe_jwks_refresh<T>(
            _trigger: JwksRefreshTrigger,
            _outcome: JwksRefreshOutcome,
            result: AppResult<T>,
        ) -> AppResult<T> {
            result
        }
    }
}

mod auth_under_test {
    include!("../src/infrastructure/auth.rs");

    #[cfg(test)]
    mod rotation_rollout {
        use super::*;

        const TEST_ISSUER: &str = "https://auth.memo.test";
        const TEST_AUDIENCE: &str = "memo-api";
        const TEST_USER_ID: &str = "12345678-1234-4234-8234-123456789012";

        // Fixture-only bound: the static signed tokens deliberately outlive the
        // deployment policy range so this regression remains deterministic.
        // Production configuration is still constrained by AppConfig tests.
        const TEST_TOKEN_LIFETIME_SECONDS: u64 = 3_000_000_000;

        const OLD_EC_X: &str = "yaoQu-f0gTaWyBTD0BZ0r28LndWqh0Xn-rQOynEOkZrPkqxCLHf7Qnbv9K0TOAag";
        const OLD_EC_Y: &str = "0MIBkNuwwFXm0CqxaGYxjuGIwD_wKX9DqE16TFVnNcyHha9QVqZ8lhosVwKRKTt4";
        const NEW_EC_X: &str = "EARZSdKbfLYw8HosQxf-kuNyASrWCmIqCHBdnE8LB-LeIdxy84IsFlm_O9ShEIio";
        const NEW_EC_Y: &str = "q_iMicqi1KZFC4hSKYIZnI2aXFhiHKtGtbab-4-nXIzGHtLnpeyYOt6-phnnkyuk";

        const OLD_TOKEN: &str =
            "eyJhbGciOiJFUzM4NCIsInR5cCI6IkpXVCIsImtpZCI6ImVjLXRlc3QifQ.eyJzdWIiOiIxMjM0NTY3OC0xMjM0LTQyMzQtODIzNC0xMjM0NTY3ODkwMTIiLCJpc3MiOiJodHRwczovL2F1dGgubWVtby50ZXN0IiwiYXVkIjoibWVtby1hcGkiLCJpYXQiOjE3MDAwMDAwMDAsImV4cCI6NDEwMjQ0NDgwMCwibmJmIjoxNzAwMDAwMDAwfQ.Hkju4wPXh0OU3FPNMJgIti1Gp08Ek1VD_ur1f9os7yc5N73q-KUZFg-PV-zipv0mJf9FBxTn1eEo-ofKxlDwf538cRYCnbC0MBodRfKm1skaA68uVf0hfgBt8axnHNjK";
        const NEW_TOKEN: &str =
            "eyJhbGciOiJFUzM4NCIsInR5cCI6IkpXVCIsImtpZCI6ImVjLW5leHQifQ.eyJzdWIiOiIxMjM0NTY3OC0xMjM0LTQyMzQtODIzNC0xMjM0NTY3ODkwMTIiLCJpc3MiOiJodHRwczovL2F1dGgubWVtby50ZXN0IiwiYXVkIjoibWVtby1hcGkiLCJpYXQiOjE3MDAwMDAwMDAsImV4cCI6NDEwMjQ0NDgwMCwibmJmIjoxNzAwMDAwMDAwfQ.D8Yb8SAkGuKzVOBcBsnK65zLJO-GE2g3eEIVKS0J8YchmHg22M4CofUzWpXhLXQ-9DXspStzpknPuBTO7MQl86MUwSUxvXQwWkAMvA41ZALDMtO7MSENjl1wU2Bn8NTL";

        fn verifier() -> JwtVerifier {
            JwtVerifier::new(
                TEST_ISSUER.to_string(),
                TEST_AUDIENCE.to_string(),
                "https://jwks.invalid/.well-known/jwks.json".to_string(),
                JwtSignatureMode::Es384,
                JwtAccessTokenTypeMode::LegacyAny,
                TEST_TOKEN_LIFETIME_SECONDS,
            )
            .expect("test verifier must initialize")
        }

        fn es384_jwk(kid: &str, x: &str, y: &str) -> Jwk {
            serde_json::from_value(serde_json::json!({
                "kty": "EC",
                "use": "sig",
                "key_ops": ["verify"],
                "alg": "ES384",
                "kid": kid,
                "crv": "P-384",
                "x": x,
                "y": y
            }))
            .expect("test JWK must decode")
        }

        fn old_key() -> Jwk {
            es384_jwk("ec-test", OLD_EC_X, OLD_EC_Y)
        }

        fn new_key() -> Jwk {
            es384_jwk("ec-next", NEW_EC_X, NEW_EC_Y)
        }

        async fn install_rotation_snapshot(verifier: &JwtVerifier, keys: Vec<Jwk>) {
            let set = JwkSet { keys };
            validate_jwks_set(&set).expect("rotation fixture JWKS must satisfy production shape");
            *verifier.jwks.write().await = Some(CachedJwks {
                set: Arc::new(set),
                fetched_at: Instant::now(),
            });
        }

        async fn suppress_external_forced_refresh(verifier: &JwtVerifier) {
            verifier.refresh_state.lock().await.last_forced_attempt = Some(Instant::now());
        }

        async fn assert_valid(verifier: &JwtVerifier, token: &str) {
            let identity = verifier
                .verify(token)
                .await
                .expect("token should verify in this rollout phase");
            assert_eq!(
                identity.user_id,
                Uuid::parse_str(TEST_USER_ID).expect("test UUID must parse")
            );
        }

        fn jwks_http_document(include_new: bool) -> String {
            let mut keys = vec![serde_json::json!({
                "kty": "EC",
                "use": "sig",
                "key_ops": ["verify"],
                "alg": "ES384",
                "kid": "ec-test",
                "crv": "P-384",
                "x": OLD_EC_X,
                "y": OLD_EC_Y
            })];
            if include_new {
                keys.push(serde_json::json!({
                    "kty": "EC",
                    "use": "sig",
                    "key_ops": ["verify"],
                    "alg": "ES384",
                    "kid": "ec-next",
                    "crv": "P-384",
                    "x": NEW_EC_X,
                    "y": NEW_EC_Y
                }));
            }
            serde_json::json!({ "keys": keys }).to_string()
        }

        async fn jwks_http_fixture(
            responses: Vec<(u16, String)>,
        ) -> (JwtVerifier, tokio::task::JoinHandle<()>) {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};

            // The production verifier remains HTTPS-only. The local HTTP
            // override is confined to this test and exercises its real
            // reqwest fetch/validation/cache implementation end-to-end.
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("fixture listener must bind loopback");
            let address = listener.local_addr().expect("fixture address must exist");
            let mut verifier = verifier();
            verifier.jwks_uri = format!("http://{address}/jwks");
            verifier.client = Client::builder()
                .https_only(false)
                .redirect(Policy::none())
                .no_proxy()
                .build()
                .expect("test-only client must initialize");

            let server = tokio::spawn(async move {
                for (status, body) in responses {
                    let (mut socket, _) =
                        tokio::time::timeout(Duration::from_secs(10), listener.accept())
                            .await
                            .expect("expected another JWKS HTTP request")
                            .expect("fixture must accept connection");
                    let mut request = [0_u8; 2048];
                    let received = socket
                        .read(&mut request)
                        .await
                        .expect("HTTP read must succeed");
                    assert!(
                        request[..received].starts_with(b"GET /jwks HTTP/1.1"),
                        "verifier must request the fixture JWKS path"
                    );
                    let reason = match status {
                        200 => "OK",
                        503 => "Service Unavailable",
                        _ => panic!("unsupported fixture status"),
                    };
                    let response = format!(
                        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    socket
                        .write_all(response.as_bytes())
                        .await
                        .expect("HTTP response must be written");
                    socket
                        .shutdown()
                        .await
                        .expect("fixture must close connection");
                }
            });
            (verifier, server)
        }

        async fn age_cached_jwks(verifier: &JwtVerifier, age: Duration) {
            let mut cached = verifier.jwks.write().await;
            cached
                .as_mut()
                .expect("fixture must have installed JWKS")
                .fetched_at = Instant::now() - age;
        }

        async fn assert_fixture_requests_complete(server: tokio::task::JoinHandle<()>) {
            tokio::time::timeout(Duration::from_secs(12), server)
                .await
                .expect("all expected JWKS requests must have completed")
                .expect("fixture server must not panic");
        }

        #[tokio::test]
        async fn http_jwks_rotation_outage_invalid_json_and_recovery_preserve_cache_bounds() {
            // Request sequence: initial publication, key overlap, temporary
            // upstream 503, malformed JSON, expired-cache 503, recovery.
            let (verifier, server) = jwks_http_fixture(vec![
                (200, jwks_http_document(false)),
                (200, jwks_http_document(true)),
                (503, "temporarily unavailable".into()),
                (200, "{invalid-json".into()),
                (503, "temporarily unavailable".into()),
                (200, jwks_http_document(true)),
            ])
            .await;

            assert_valid(&verifier, OLD_TOKEN).await;
            // Unknown kid fetches an overlapping JWKS snapshot over HTTP.
            assert_valid(&verifier, NEW_TOKEN).await;

            // Both failure modes retain known public keys within the
            // bounded stale-if-error horizon.
            age_cached_jwks(&verifier, Duration::from_secs(301)).await;
            assert_valid(&verifier, OLD_TOKEN).await;
            age_cached_jwks(&verifier, Duration::from_secs(301)).await;
            assert_valid(&verifier, NEW_TOKEN).await;

            // No stale fallback once the configured maximum has elapsed.
            age_cached_jwks(
                &verifier,
                JWKS_STALE_IF_ERROR_TTL_MAX + Duration::from_secs(1),
            )
            .await;
            assert!(matches!(
                verifier.verify(OLD_TOKEN).await,
                Err(AppError::ServiceUnavailable(_))
            ));

            // The next healthy response atomically replaces the stale cache.
            assert_valid(&verifier, OLD_TOKEN).await;
            assert_valid(&verifier, NEW_TOKEN).await;
            assert_fixture_requests_complete(server).await;
        }

        #[tokio::test]
        async fn http_jwks_unknown_kid_outage_fails_closed_and_recovers_after_cooldown() {
            let (verifier, server) = jwks_http_fixture(vec![
                (200, jwks_http_document(false)),
                (503, "temporarily unavailable".into()),
                (200, jwks_http_document(true)),
            ])
            .await;

            assert_valid(&verifier, OLD_TOKEN).await;
            assert!(matches!(
                verifier.verify(NEW_TOKEN).await,
                Err(AppError::ServiceUnavailable(_))
            ));
            // A throttled retry must not turn an unavailable unknown-kid
            // rotation into successful verification or another HTTP fetch.
            assert!(matches!(
                verifier.verify(NEW_TOKEN).await,
                Err(AppError::Unauthorized(_))
            ));
            assert_valid(&verifier, OLD_TOKEN).await;

            // Expire the fixture's cooldown without sleeping for five seconds.
            verifier.refresh_state.lock().await.last_forced_attempt =
                Some(Instant::now() - JWKS_FORCED_REFRESH_COOLDOWN);
            assert_valid(&verifier, NEW_TOKEN).await;
            assert_valid(&verifier, OLD_TOKEN).await;
            assert_fixture_requests_complete(server).await;
        }

        #[tokio::test]
        async fn http_jwks_multi_replica_rotation_converges_after_partial_issuer_outage() {
            // Two API replicas intentionally have separate JWKS caches while
            // consulting the same issuer. Responses represent prepublication,
            // overlap, a transient outage on replica B, and recovery.
            let (replica_a, server) = jwks_http_fixture(vec![
                (200, jwks_http_document(false)),
                (200, jwks_http_document(false)),
                (200, jwks_http_document(true)),
                (503, "temporarily unavailable".into()),
                (200, jwks_http_document(true)),
            ])
            .await;
            let mut replica_b = verifier();
            replica_b.jwks_uri = replica_a.jwks_uri.clone();
            replica_b.client = replica_a.client.clone();

            assert_valid(&replica_a, OLD_TOKEN).await;
            assert_valid(&replica_b, OLD_TOKEN).await;

            // Replica A learns about the newly published key, while B still
            // holds the old snapshot and cannot infer keys from A's cache.
            assert_valid(&replica_a, NEW_TOKEN).await;
            assert_valid(&replica_a, OLD_TOKEN).await;
            assert!(matches!(
                replica_b.verify(NEW_TOKEN).await,
                Err(AppError::ServiceUnavailable(_))
            ));
            assert_valid(&replica_b, OLD_TOKEN).await;

            // The forced-refresh cooldown does not create an extra HTTP
            // request and must not authenticate a token with an unknown kid.
            assert!(matches!(
                replica_b.verify(NEW_TOKEN).await,
                Err(AppError::Unauthorized(_))
            ));
            assert_valid(&replica_a, NEW_TOKEN).await;

            // After the cooldown, B independently retrieves the overlap and
            // converges without invalidating the old signing key.
            replica_b.refresh_state.lock().await.last_forced_attempt =
                Some(Instant::now() - JWKS_FORCED_REFRESH_COOLDOWN);
            assert_valid(&replica_b, NEW_TOKEN).await;
            assert_valid(&replica_b, OLD_TOKEN).await;
            assert_valid(&replica_a, OLD_TOKEN).await;
            assert_fixture_requests_complete(server).await;
        }

        #[tokio::test]
        async fn http_jwks_partial_body_timeout_fails_closed_without_caching_keys() {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};

            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("fixture listener must bind loopback");
            let address = listener.local_addr().expect("fixture address must exist");
            let mut verifier = verifier();
            verifier.jwks_uri = format!("http://{address}/jwks");
            verifier.client = Client::builder()
                .https_only(false)
                .redirect(Policy::none())
                .no_proxy()
                .build()
                .expect("test-only client must initialize");

            // Return successful HTTP headers and a truncated JSON fragment,
            // then hold the socket open. The transport deadline must cover
            // the response stream, not merely receipt of response headers.
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener
                    .accept()
                    .await
                    .expect("fixture must accept JWKS request");
                let mut request = [0_u8; 2048];
                let received = socket
                    .read(&mut request)
                    .await
                    .expect("fixture must read request");
                assert!(request[..received].starts_with(b"GET /jwks HTTP/1.1"));
                socket
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 4096\r\nConnection: close\r\n\r\n{\"keys\":[")
                    .await
                    .expect("fixture must send partial response");
                tokio::time::sleep(JWKS_REQUEST_TIMEOUT + Duration::from_secs(4)).await;
            });

            let result = tokio::time::timeout(
                JWKS_REQUEST_TIMEOUT + Duration::from_secs(2),
                verifier.verify(OLD_TOKEN),
            )
            .await
            .expect("JWKS request must finish within its configured deadline");
            assert!(matches!(result, Err(AppError::ServiceUnavailable(_))));
            assert!(
                verifier.jwks.read().await.is_none(),
                "incomplete JWKS must never enter the verification cache"
            );
            server.abort();
            let _ = server.await;
        }

        #[tokio::test]
        async fn signing_key_overlap_supports_cutover_and_issuer_rollback() {
            let verifier = verifier();

            install_rotation_snapshot(&verifier, vec![old_key()]).await;
            assert_valid(&verifier, OLD_TOKEN).await;

            install_rotation_snapshot(&verifier, vec![old_key(), new_key()]).await;
            assert_valid(&verifier, OLD_TOKEN).await;
            assert_valid(&verifier, NEW_TOKEN).await;

            // Rollback means issuing with the old key again while the overlap
            // remains published. Previously issued new-key tokens stay valid.
            assert_valid(&verifier, OLD_TOKEN).await;
            assert_valid(&verifier, NEW_TOKEN).await;
        }

        #[tokio::test]
        async fn rollback_can_retire_new_key_only_after_its_token_horizon() {
            let verifier = verifier();

            install_rotation_snapshot(&verifier, vec![old_key(), new_key()]).await;
            assert_valid(&verifier, NEW_TOKEN).await;

            // This snapshot models the state only after every token signed by
            // the new key has passed its validity horizon. The forced-refresh
            // cooldown prevents this deterministic test from contacting the
            // intentionally invalid external JWKS origin when `kid` is absent.
            install_rotation_snapshot(&verifier, vec![old_key()]).await;
            suppress_external_forced_refresh(&verifier).await;

            assert_valid(&verifier, OLD_TOKEN).await;
            assert!(matches!(
                verifier.verify(NEW_TOKEN).await,
                Err(AppError::Unauthorized(_))
            ));
        }
    }
}
