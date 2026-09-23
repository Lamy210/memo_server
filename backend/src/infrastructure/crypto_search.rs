// Staged SEARCH-HIGH-1 cryptography. Not wired into request-path search.
#![allow(dead_code)]

use std::{fmt::Write as _, sync::Arc};

use async_trait::async_trait;
use ring::hmac;
use uuid::Uuid;

use crate::{
    application::crypto_search::{
        validate_normalized_search_term, HighSearchToken, HighSearchTokenCryptography,
        SEARCH_HIGH_SUITE_ID,
    },
    error::{AppError, AppResult},
    infrastructure::crypto_search_keys::SearchKeyProvider,
};

const SEARCH_TOKEN_DOMAIN: &[u8] = b"memo_server:search:blind:v1\0";

pub(super) struct RingHighSearchTokenCryptography {
    keys: Arc<dyn SearchKeyProvider>,
}

impl RingHighSearchTokenCryptography {
    pub(super) fn new(keys: Arc<dyn SearchKeyProvider>) -> Self {
        Self { keys }
    }

    fn derive_with_key(
        owner_partition: Uuid,
        normalized_term: &str,
        key: &hmac::Key,
        key_version: &str,
    ) -> AppResult<HighSearchToken> {
        let term = normalized_term.as_bytes();
        let term_len = u32::try_from(term.len()).map_err(|_| {
            AppError::ValidationError("HIGH search token input is too large".into())
        })?;

        let mut input = Vec::with_capacity(
            SEARCH_TOKEN_DOMAIN.len() + owner_partition.as_bytes().len() + 4 + term.len(),
        );
        input.extend_from_slice(SEARCH_TOKEN_DOMAIN);
        input.extend_from_slice(owner_partition.as_bytes());
        input.extend_from_slice(&term_len.to_be_bytes());
        input.extend_from_slice(term);

        let signature = hmac::sign(key, &input);
        let mut value = String::with_capacity(signature.as_ref().len() * 2);
        for byte in signature.as_ref() {
            let _ = write!(&mut value, "{byte:02x}");
        }

        let token = HighSearchToken {
            value,
            key_version: key_version.to_string(),
            suite_id: SEARCH_HIGH_SUITE_ID.into(),
        };
        token.validate()?;
        Ok(token)
    }
}

#[async_trait]
impl HighSearchTokenCryptography for RingHighSearchTokenCryptography {
    async fn derive_token(
        &self,
        owner_partition: Uuid,
        normalized_term: &str,
    ) -> AppResult<HighSearchToken> {
        validate_normalized_search_term(normalized_term)?;

        let resolved = self.keys.resolve_search_key(owner_partition).await?;
        resolved.validate()?;
        let key = hmac::Key::new(hmac::HMAC_SHA384, resolved.plaintext.expose());

        Self::derive_with_key(
            owner_partition,
            normalized_term,
            &key,
            &resolved.key_version,
        )
    }

    async fn derive_tokens(
        &self,
        owner_partition: Uuid,
        normalized_terms: &[String],
    ) -> AppResult<Vec<HighSearchToken>> {
        if normalized_terms.is_empty() {
            return Ok(Vec::new());
        }
        for term in normalized_terms {
            validate_normalized_search_term(term)?;
        }

        let resolved = self.keys.resolve_search_key(owner_partition).await?;
        resolved.validate()?;
        let key = hmac::Key::new(hmac::HMAC_SHA384, resolved.plaintext.expose());

        normalized_terms
            .iter()
            .map(|term| {
                Self::derive_with_key(owner_partition, term, &key, &resolved.key_version)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;

    use super::*;
    use crate::infrastructure::crypto_search_keys::{
        ResolvedSearchKey, SearchKeyProvider, SecretSearchKey, SEARCH_KEY_BYTES,
    };

    struct TestSearchKeyProvider;

    #[async_trait]
    impl SearchKeyProvider for TestSearchKeyProvider {
        async fn resolve_search_key(&self, owner_partition: Uuid) -> AppResult<ResolvedSearchKey> {
            let mut bytes = [0u8; SEARCH_KEY_BYTES];
            bytes[..16].copy_from_slice(owner_partition.as_bytes());
            bytes[16..32].copy_from_slice(owner_partition.as_bytes());
            bytes[32..48].copy_from_slice(owner_partition.as_bytes());

            Ok(ResolvedSearchKey {
                plaintext: SecretSearchKey::new(bytes),
                key_version: "test-search-v1".into(),
            })
        }
    }

    struct CountingSearchKeyProvider {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl SearchKeyProvider for CountingSearchKeyProvider {
        async fn resolve_search_key(&self, owner_partition: Uuid) -> AppResult<ResolvedSearchKey> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            let mut bytes = [0u8; SEARCH_KEY_BYTES];
            bytes[..16].copy_from_slice(owner_partition.as_bytes());
            bytes[16..32].copy_from_slice(owner_partition.as_bytes());
            bytes[32..48].copy_from_slice(owner_partition.as_bytes());

            Ok(ResolvedSearchKey {
                plaintext: SecretSearchKey::new(bytes),
                key_version: "test-search-v1".into(),
            })
        }
    }

    fn cryptography() -> RingHighSearchTokenCryptography {
        RingHighSearchTokenCryptography::new(Arc::new(TestSearchKeyProvider))
    }

    #[tokio::test]
    async fn blind_token_is_deterministic_for_same_owner_and_term() {
        let crypto = cryptography();
        let owner = Uuid::new_v4();

        let first = crypto.derive_token(owner, "snow").await.unwrap();
        let second = crypto.derive_token(owner, "snow").await.unwrap();

        assert_eq!(first, second);
        assert_eq!(first.suite_id, SEARCH_HIGH_SUITE_ID);
        assert_eq!(first.key_version, "test-search-v1");
        assert!(!first.value.contains("snow"));
    }

    #[tokio::test]
    async fn blind_token_is_owner_scoped() {
        let crypto = cryptography();

        let first = crypto
            .derive_token(Uuid::new_v4(), "same-term")
            .await
            .unwrap();
        let second = crypto
            .derive_token(Uuid::new_v4(), "same-term")
            .await
            .unwrap();

        assert_ne!(first.value, second.value);
    }

    #[tokio::test]
    async fn blind_token_changes_with_normalized_term() {
        let crypto = cryptography();
        let owner = Uuid::new_v4();

        let first = crypto.derive_token(owner, "snow").await.unwrap();
        let second = crypto.derive_token(owner, "snowfall").await.unwrap();

        assert_ne!(first.value, second.value);
    }

    #[tokio::test]
    async fn blind_token_rejects_non_normalized_input() {
        let crypto = cryptography();
        let owner = Uuid::new_v4();

        assert!(matches!(
            crypto.derive_token(owner, " snow ").await,
            Err(AppError::ValidationError(_))
        ));
    }

    #[tokio::test]
    async fn batched_blind_tokens_resolve_search_key_once() {
        let keys = Arc::new(CountingSearchKeyProvider {
            calls: AtomicUsize::new(0),
        });
        let crypto = RingHighSearchTokenCryptography::new(keys.clone());
        let owner = Uuid::new_v4();
        let terms = vec!["snow".to_string(), "memo".to_string(), "tag".to_string()];

        let tokens = crypto.derive_tokens(owner, &terms).await.unwrap();

        assert_eq!(tokens.len(), terms.len());
        assert_eq!(keys.calls.load(Ordering::Relaxed), 1);
        assert!(tokens
            .iter()
            .all(|token| token.key_version == "test-search-v1"));
        assert_ne!(tokens[0].value, tokens[1].value);
    }

    #[tokio::test]
    async fn empty_token_batch_does_not_resolve_search_key() {
        let keys = Arc::new(CountingSearchKeyProvider {
            calls: AtomicUsize::new(0),
        });
        let crypto = RingHighSearchTokenCryptography::new(keys.clone());

        let tokens = crypto.derive_tokens(Uuid::new_v4(), &[]).await.unwrap();

        assert!(tokens.is_empty());
        assert_eq!(keys.calls.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn invalid_token_batch_fails_before_search_key_resolution() {
        let keys = Arc::new(CountingSearchKeyProvider {
            calls: AtomicUsize::new(0),
        });
        let crypto = RingHighSearchTokenCryptography::new(keys.clone());
        let terms = vec!["snow".to_string(), " invalid ".to_string()];

        assert!(matches!(
            crypto.derive_tokens(Uuid::new_v4(), &terms).await,
            Err(AppError::ValidationError(_))
        ));
        assert_eq!(keys.calls.load(Ordering::Relaxed), 0);
    }
}
