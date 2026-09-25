// Staged managed-PRF adapter for HIGH search seeds. A concrete cloud/HSM
// client remains deployment-specific and must bind to immutable key material.
#![allow(dead_code)]

use std::sync::Arc;

use async_trait::async_trait;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::{
    application::crypto_search::{search_version_identifier_is_valid, MAX_SEARCH_VERSION_ID_CHARS},
    error::{AppError, AppResult},
};

use super::crypto_search_keys::{
    ResolvedSearchKeySeed, SearchKeySeedProvider, SecretSearchKeySeed, SEARCH_KEY_SEED_BYTES,
};

const SEARCH_SEED_PRF_VERSION: &str = "prf384-v1";
const SEARCH_SEED_PRF_DOMAIN: &[u8] = b"memo_server:search:seed-prf:v1\0";
pub(crate) const MANAGED_PRF_PROVIDER_AWS_KMS: &str = "aws-kms";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ManagedPrfClientBinding<'a> {
    pub(crate) provider: &'static str,
    pub(crate) immutable_key_reference: &'a str,
}

#[async_trait]
pub(crate) trait ManagedSearchSeedPrfClient: Send + Sync {
    /// Identify the provider and immutable key reference actually used by this
    /// client so higher-level composition can bind runtime configuration to the
    /// concrete cryptographic key.
    fn binding(&self) -> ManagedPrfClientBinding<'_>;

    /// Compute HMAC-SHA-384 with provider-managed, non-exportable key material.
    ///
    /// The concrete implementation must be bound to immutable provider key
    /// material for the configured application seed version. Mutable aliases
    /// must not silently change the underlying key while keeping the same
    /// application-owned version identifier.
    async fn hmac_sha384(&self, message: &[u8]) -> AppResult<Zeroizing<Vec<u8>>>;
}

/// Adapts a managed provider-side HMAC/PRF operation into SearchKeySeedProvider.
///
/// The provider receives only domain-separated operational metadata. It never
/// receives memo plaintext, search terms, or blind tokens.
pub(super) struct ManagedPrfSearchKeySeedProvider {
    client: Arc<dyn ManagedSearchSeedPrfClient>,
    provider_seed_version: String,
}

impl ManagedPrfSearchKeySeedProvider {
    pub(super) fn new(
        client: Arc<dyn ManagedSearchSeedPrfClient>,
        provider_seed_version: String,
    ) -> AppResult<Self> {
        if !search_version_identifier_is_valid(&provider_seed_version) {
            return Err(AppError::ServiceUnavailable(format!(
                "Managed HIGH search provider seed version must be 1..={MAX_SEARCH_VERSION_ID_CHARS} ASCII identifier characters"
            )));
        }

        let seed_version = format!("{SEARCH_SEED_PRF_VERSION}:{provider_seed_version}");
        if !search_version_identifier_is_valid(&seed_version) {
            return Err(AppError::ServiceUnavailable(format!(
                "Managed HIGH search derived seed version must be 1..={MAX_SEARCH_VERSION_ID_CHARS} ASCII identifier characters"
            )));
        }

        Ok(Self {
            client,
            provider_seed_version,
        })
    }

    fn seed_version(&self) -> String {
        format!("{SEARCH_SEED_PRF_VERSION}:{}", self.provider_seed_version)
    }

    fn prf_message(&self, owner_partition: Uuid) -> AppResult<Vec<u8>> {
        let seed_version = self.seed_version();
        let version = seed_version.as_bytes();
        let version_len = u32::try_from(version.len()).map_err(|_| {
            AppError::ServiceUnavailable("Managed HIGH search seed version is too large".into())
        })?;

        let mut message = Vec::with_capacity(SEARCH_SEED_PRF_DOMAIN.len() + 4 + version.len() + 16);
        message.extend_from_slice(SEARCH_SEED_PRF_DOMAIN);
        message.extend_from_slice(&version_len.to_be_bytes());
        message.extend_from_slice(version);
        message.extend_from_slice(owner_partition.as_bytes());
        Ok(message)
    }
}

#[async_trait]
impl SearchKeySeedProvider for ManagedPrfSearchKeySeedProvider {
    async fn resolve_search_seed(&self, owner_partition: Uuid) -> AppResult<ResolvedSearchKeySeed> {
        let message = self.prf_message(owner_partition)?;
        let output = self.client.hmac_sha384(&message).await?;

        if output.len() != SEARCH_KEY_SEED_BYTES {
            return Err(AppError::ServiceUnavailable(format!(
                "Managed HIGH search seed PRF returned {} bytes; expected {SEARCH_KEY_SEED_BYTES}",
                output.len()
            )));
        }

        let mut bytes = [0u8; SEARCH_KEY_SEED_BYTES];
        bytes.copy_from_slice(output.as_slice());

        Ok(ResolvedSearchKeySeed {
            plaintext: SecretSearchKeySeed::new(bytes),
            key_version: self.seed_version(),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use ring::hmac;

    use super::*;

    struct FakeManagedPrfClient {
        key: hmac::Key,
        messages: Mutex<Vec<Vec<u8>>>,
    }

    impl FakeManagedPrfClient {
        fn new(byte: u8) -> Self {
            Self {
                key: hmac::Key::new(hmac::HMAC_SHA384, &[byte; SEARCH_KEY_SEED_BYTES]),
                messages: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl ManagedSearchSeedPrfClient for FakeManagedPrfClient {
        fn binding(&self) -> ManagedPrfClientBinding<'_> {
            ManagedPrfClientBinding {
                provider: "test",
                immutable_key_reference: "test-key",
            }
        }

        async fn hmac_sha384(&self, message: &[u8]) -> AppResult<Zeroizing<Vec<u8>>> {
            self.messages.lock().unwrap().push(message.to_vec());
            Ok(Zeroizing::new(
                hmac::sign(&self.key, message).as_ref().to_vec(),
            ))
        }
    }

    struct WrongLengthPrfClient;

    #[async_trait]
    impl ManagedSearchSeedPrfClient for WrongLengthPrfClient {
        fn binding(&self) -> ManagedPrfClientBinding<'_> {
            ManagedPrfClientBinding {
                provider: "test",
                immutable_key_reference: "test-key",
            }
        }

        async fn hmac_sha384(&self, _message: &[u8]) -> AppResult<Zeroizing<Vec<u8>>> {
            Ok(Zeroizing::new(vec![0xAB; SEARCH_KEY_SEED_BYTES - 1]))
        }
    }

    #[tokio::test]
    async fn managed_prf_seed_is_stable_and_owner_scoped() {
        let client = Arc::new(FakeManagedPrfClient::new(0x11));
        let provider =
            ManagedPrfSearchKeySeedProvider::new(client.clone(), "search-seed-v1".into()).unwrap();
        let owner = Uuid::new_v4();

        let first = provider.resolve_search_seed(owner).await.unwrap();
        let second = provider.resolve_search_seed(owner).await.unwrap();
        let other = provider.resolve_search_seed(Uuid::new_v4()).await.unwrap();

        assert_eq!(first.plaintext.expose(), second.plaintext.expose());
        assert_ne!(first.plaintext.expose(), other.plaintext.expose());
        assert_eq!(first.key_version, "prf384-v1:search-seed-v1");
        assert_eq!(client.messages.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn managed_prf_seed_version_is_domain_bound() {
        let owner = Uuid::new_v4();
        let first = ManagedPrfSearchKeySeedProvider::new(
            Arc::new(FakeManagedPrfClient::new(0x22)),
            "search-seed-v1".into(),
        )
        .unwrap()
        .resolve_search_seed(owner)
        .await
        .unwrap();
        let second = ManagedPrfSearchKeySeedProvider::new(
            Arc::new(FakeManagedPrfClient::new(0x22)),
            "search-seed-v2".into(),
        )
        .unwrap()
        .resolve_search_seed(owner)
        .await
        .unwrap();

        assert_ne!(first.plaintext.expose(), second.plaintext.expose());
        assert_ne!(first.key_version, second.key_version);
    }

    #[tokio::test]
    async fn managed_prf_rejects_wrong_output_length() {
        let provider = ManagedPrfSearchKeySeedProvider::new(
            Arc::new(WrongLengthPrfClient),
            "search-seed-v1".into(),
        )
        .unwrap();

        assert!(matches!(
            provider.resolve_search_seed(Uuid::new_v4()).await,
            Err(AppError::ServiceUnavailable(_))
        ));
    }

    #[test]
    fn managed_prf_rejects_invalid_seed_version_before_provider_use() {
        assert!(ManagedPrfSearchKeySeedProvider::new(
            Arc::new(FakeManagedPrfClient::new(0x33)),
            "search seed v1".into(),
        )
        .is_err());

        let prefix_len = SEARCH_SEED_PRF_VERSION.len() + 1;
        let provider_version = "x".repeat(MAX_SEARCH_VERSION_ID_CHARS - prefix_len + 1);
        assert!(ManagedPrfSearchKeySeedProvider::new(
            Arc::new(FakeManagedPrfClient::new(0x33)),
            provider_version,
        )
        .is_err());
    }

    #[test]
    fn managed_prf_message_is_exactly_domain_version_and_owner_partition() {
        let provider = ManagedPrfSearchKeySeedProvider::new(
            Arc::new(FakeManagedPrfClient::new(0x44)),
            "search-seed-v1".into(),
        )
        .unwrap();
        let owner = Uuid::from_u128(0x11223344556677889900aabbccddeeff);
        let message = provider.prf_message(owner).unwrap();
        let version = b"prf384-v1:search-seed-v1";

        let mut expected =
            Vec::with_capacity(SEARCH_SEED_PRF_DOMAIN.len() + 4 + version.len() + 16);
        expected.extend_from_slice(SEARCH_SEED_PRF_DOMAIN);
        expected.extend_from_slice(&(version.len() as u32).to_be_bytes());
        expected.extend_from_slice(version);
        expected.extend_from_slice(owner.as_bytes());

        assert_eq!(message, expected);
    }
}
