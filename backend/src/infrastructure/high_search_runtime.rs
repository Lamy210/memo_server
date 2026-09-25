#![allow(dead_code)]

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use uuid::Uuid;

use crate::{
    application::{
        crypto_search_orchestration::{HighSearchAnalysisBudget, HighSearchProjectionService},
        crypto_search_rotation::HighSearchKeyCacheControl,
    },
    config::HighSearchConfig,
    error::{AppError, AppResult},
};

use super::{
    crypto_search::RingHighSearchTokenCryptography,
    crypto_search_analyzer::IcuHighSearchTextAnalyzer,
    crypto_search_keys::{
        CachingSearchKeyProvider, HkdfSearchKeyProvider, SearchKeyCachePolicy, SearchKeyProvider,
    },
    crypto_search_seed_provider::{
        ManagedPrfSearchKeySeedProvider, ManagedSearchSeedPrfClient, MANAGED_PRF_PROVIDER_AWS_KMS,
    },
    persistence::manticore_high::HighManticoreClient,
};

/// Staged runtime composition for SEARCH-HIGH-1.
///
/// This stack proves that the accepted analyzer, managed-PRF seed boundary,
/// HKDF owner-key derivation, bounded derived-key cache, blind-token
/// cryptography, and protected Manticore projection can be composed from one
/// fail-closed configuration contract. It is intentionally not installed into
/// the request path yet.
pub(crate) struct HighSearchRuntimeStack {
    projection_service: Arc<HighSearchProjectionService>,
    key_cache: Arc<CachingSearchKeyProvider>,
    cache_sweep_interval: Duration,
}

impl HighSearchRuntimeStack {
    pub(crate) fn build_managed_prf(
        config: &HighSearchConfig,
        search_uri: &str,
        prf_client: Option<Arc<dyn ManagedSearchSeedPrfClient>>,
    ) -> AppResult<Option<Self>> {
        let HighSearchConfig::AwsKms {
            key_arn,
            provider_seed_version,
            cache_ttl_seconds,
            cache_max_entries,
            cache_sweep_seconds,
            max_document_content_terms,
            max_query_content_terms,
            max_normalized_term_bytes,
            ..
        } = config
        else {
            return Ok(None);
        };

        let prf_client = prf_client.ok_or_else(|| {
            AppError::ServiceUnavailable(
                "HIGH search is enabled but no managed PRF client was composed".into(),
            )
        })?;
        let binding = prf_client.binding();
        if binding.provider != MANAGED_PRF_PROVIDER_AWS_KMS
            || binding.immutable_key_reference != key_arn
        {
            return Err(AppError::ServiceUnavailable(
                "HIGH search managed PRF client does not match configured AWS KMS key".into(),
            ));
        }

        let seed_provider = Arc::new(ManagedPrfSearchKeySeedProvider::new(
            prf_client,
            provider_seed_version.clone(),
        )?);
        let owner_keys: Arc<dyn SearchKeyProvider> =
            Arc::new(HkdfSearchKeyProvider::new(seed_provider));

        let cache_policy =
            SearchKeyCachePolicy::new(Duration::from_secs(*cache_ttl_seconds), *cache_max_entries)?;
        if *cache_sweep_seconds == 0 {
            return Err(AppError::ServiceUnavailable(
                "HIGH search key-cache sweep interval must be greater than zero".into(),
            ));
        }
        let key_cache = Arc::new(CachingSearchKeyProvider::new(owner_keys, cache_policy));
        let cryptography = Arc::new(RingHighSearchTokenCryptography::new(key_cache.clone()));
        let analyzer = Arc::new(IcuHighSearchTextAnalyzer::new());
        let projection = Arc::new(HighManticoreClient::new(search_uri)?);
        let budget = HighSearchAnalysisBudget::new(
            *max_document_content_terms,
            *max_query_content_terms,
            *max_normalized_term_bytes,
        )?;

        let projection_service = Arc::new(HighSearchProjectionService::new(
            analyzer,
            cryptography,
            projection,
            budget,
        ));

        Ok(Some(Self {
            projection_service,
            key_cache,
            cache_sweep_interval: Duration::from_secs(*cache_sweep_seconds),
        }))
    }

    pub(crate) fn projection_service(&self) -> Arc<HighSearchProjectionService> {
        self.projection_service.clone()
    }

    pub(crate) fn cache_sweep_interval(&self) -> Duration {
        self.cache_sweep_interval
    }

    pub(crate) async fn purge_expired_keys(&self) {
        self.key_cache.purge_expired_entries().await;
    }

    pub(crate) async fn invalidate_owner_key(&self, owner_partition: Uuid) {
        self.key_cache.invalidate(owner_partition).await;
    }

    pub(crate) async fn clear_cached_keys(&self) {
        self.key_cache.clear().await;
    }
}

#[async_trait]
impl HighSearchKeyCacheControl for HighSearchRuntimeStack {
    async fn clear_cached_keys(&self) -> AppResult<()> {
        self.key_cache.clear().await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use ring::hmac;
    use zeroize::Zeroizing;

    use super::*;

    struct TestManagedPrfClient {
        key: hmac::Key,
        provider: &'static str,
        key_reference: String,
    }

    impl TestManagedPrfClient {
        fn matching_config() -> Self {
            Self {
                key: hmac::Key::new(hmac::HMAC_SHA384, &[0x42; 48]),
                provider: MANAGED_PRF_PROVIDER_AWS_KMS,
                key_reference:
                    "arn:aws:kms:ap-northeast-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab"
                        .into(),
            }
        }
    }

    #[async_trait]
    impl ManagedSearchSeedPrfClient for TestManagedPrfClient {
        fn binding(
            &self,
        ) -> super::super::crypto_search_seed_provider::ManagedPrfClientBinding<'_> {
            super::super::crypto_search_seed_provider::ManagedPrfClientBinding {
                provider: self.provider,
                immutable_key_reference: &self.key_reference,
            }
        }

        async fn hmac_sha384(&self, message: &[u8]) -> AppResult<Zeroizing<Vec<u8>>> {
            Ok(Zeroizing::new(
                hmac::sign(&self.key, message).as_ref().to_vec(),
            ))
        }
    }

    fn enabled_config() -> HighSearchConfig {
        HighSearchConfig::AwsKms {
            key_arn:
                "arn:aws:kms:ap-northeast-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab"
                    .into(),
            region: "ap-northeast-1".into(),
            provider_seed_version: "search-seed-v1".into(),
            cache_ttl_seconds: 60,
            cache_max_entries: 512,
            cache_sweep_seconds: 30,
            max_document_content_terms: 2048,
            max_query_content_terms: 64,
            max_normalized_term_bytes: 256,
        }
    }

    #[test]
    fn disabled_high_search_does_not_require_runtime_dependencies() {
        let stack = HighSearchRuntimeStack::build_managed_prf(
            &HighSearchConfig::Disabled,
            "not-a-url",
            None,
        )
        .unwrap();

        assert!(stack.is_none());
    }

    #[test]
    fn enabled_high_search_requires_managed_prf_client() {
        assert!(matches!(
            HighSearchRuntimeStack::build_managed_prf(
                &enabled_config(),
                "http://127.0.0.1:9308",
                None,
            ),
            Err(AppError::ServiceUnavailable(_))
        ));
    }

    #[test]
    fn enabled_high_search_rejects_mismatched_managed_prf_binding() {
        let mut client = TestManagedPrfClient::matching_config();
        client.key_reference =
            "arn:aws:kms:ap-northeast-1:111122223333:key/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
                .into();

        assert!(matches!(
            HighSearchRuntimeStack::build_managed_prf(
                &enabled_config(),
                "http://127.0.0.1:9308",
                Some(Arc::new(client)),
            ),
            Err(AppError::ServiceUnavailable(_))
        ));

        let mut client = TestManagedPrfClient::matching_config();
        client.provider = "other-provider";
        assert!(matches!(
            HighSearchRuntimeStack::build_managed_prf(
                &enabled_config(),
                "http://127.0.0.1:9308",
                Some(Arc::new(client)),
            ),
            Err(AppError::ServiceUnavailable(_))
        ));
    }

    #[test]
    fn enabled_high_search_composes_staged_runtime_stack() {
        let client = Arc::new(TestManagedPrfClient::matching_config());
        let stack = HighSearchRuntimeStack::build_managed_prf(
            &enabled_config(),
            "http://127.0.0.1:9308",
            Some(client),
        )
        .unwrap()
        .unwrap();

        assert_eq!(stack.cache_sweep_interval(), Duration::from_secs(30));
        let service = stack.projection_service();
        assert_eq!(Arc::strong_count(&service), 2);
    }

    #[test]
    fn runtime_stack_revalidates_analysis_budget_defense_in_depth() {
        let mut config = enabled_config();
        let HighSearchConfig::AwsKms {
            max_document_content_terms,
            ..
        } = &mut config
        else {
            unreachable!();
        };
        *max_document_content_terms = 0;

        assert!(matches!(
            HighSearchRuntimeStack::build_managed_prf(
                &config,
                "http://127.0.0.1:9308",
                Some(Arc::new(TestManagedPrfClient::matching_config())),
            ),
            Err(AppError::ValidationError(_))
        ));
    }

    #[test]
    fn runtime_stack_revalidates_cache_sweep_interval_defense_in_depth() {
        let mut config = enabled_config();
        let HighSearchConfig::AwsKms {
            cache_sweep_seconds,
            ..
        } = &mut config
        else {
            unreachable!();
        };
        *cache_sweep_seconds = 0;

        assert!(matches!(
            HighSearchRuntimeStack::build_managed_prf(
                &config,
                "http://127.0.0.1:9308",
                Some(Arc::new(TestManagedPrfClient::matching_config())),
            ),
            Err(AppError::ServiceUnavailable(_))
        ));
    }
}
