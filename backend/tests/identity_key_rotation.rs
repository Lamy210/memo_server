mod config {
    pub use memo_app_backend::config::*;
}

mod error {
    pub use memo_app_backend::error::*;
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
