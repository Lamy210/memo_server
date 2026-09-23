// Staged search-key boundary. This remains runtime-unreachable until
// SEARCH-HIGH-1 is deployed and protected Manticore projection is wired.
#![allow(dead_code)]

use std::{fmt, sync::Arc};

use async_trait::async_trait;
use ring::hkdf;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::{
    application::crypto_search::{search_version_identifier_is_valid, MAX_SEARCH_VERSION_ID_CHARS},
    error::{AppError, AppResult},
};

pub(super) const SEARCH_KEY_BYTES: usize = 48;
const SEARCH_KEY_SEED_BYTES: usize = 48;
const SEARCH_KEY_DERIVATION_SALT: &[u8] = b"memo_server:search:root:v1\0";
const SEARCH_USER_KEY_INFO: &[u8] = b"memo_server:search:user-key:v1\0";

pub(super) struct SecretSearchKeySeed(Zeroizing<[u8; SEARCH_KEY_SEED_BYTES]>);

impl SecretSearchKeySeed {
    pub(super) fn new(bytes: [u8; SEARCH_KEY_SEED_BYTES]) -> Self {
        Self(Zeroizing::new(bytes))
    }

    pub(super) fn expose(&self) -> &[u8; SEARCH_KEY_SEED_BYTES] {
        &self.0
    }
}

impl fmt::Debug for SecretSearchKeySeed {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretSearchKeySeed([REDACTED])")
    }
}

pub(super) struct ResolvedSearchKeySeed {
    pub(super) plaintext: SecretSearchKeySeed,
    pub(super) key_version: String,
}

impl ResolvedSearchKeySeed {
    fn validate(&self) -> AppResult<()> {
        if !search_version_identifier_is_valid(&self.key_version) {
            return Err(AppError::ServiceUnavailable(format!(
                "Search-key seed provider key version must be 1..={MAX_SEARCH_VERSION_ID_CHARS} ASCII identifier characters"
            )));
        }
        Ok(())
    }
}

impl fmt::Debug for ResolvedSearchKeySeed {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedSearchKeySeed")
            .field("plaintext", &self.plaintext)
            .field("key_version", &self.key_version)
            .finish()
    }
}

#[async_trait]
pub(super) trait SearchKeySeedProvider: Send + Sync {
    /// Resolve one owner-scoped, versioned 384-bit seed for HIGH search.
    ///
    /// A production managed-KMS implementation may derive this seed with a
    /// provider-side HMAC/PRF operation. Long-lived root key material must not
    /// be returned to memo_server, stored in source control, or placed in
    /// ordinary application configuration.
    async fn resolve_search_seed(&self, owner_partition: Uuid) -> AppResult<ResolvedSearchKeySeed>;
}

pub(super) struct SecretSearchKey(Zeroizing<[u8; SEARCH_KEY_BYTES]>);

impl SecretSearchKey {
    pub(super) fn new(bytes: [u8; SEARCH_KEY_BYTES]) -> Self {
        Self(Zeroizing::new(bytes))
    }

    pub(super) fn expose(&self) -> &[u8; SEARCH_KEY_BYTES] {
        &self.0
    }
}

impl fmt::Debug for SecretSearchKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretSearchKey([REDACTED])")
    }
}

pub(super) struct ResolvedSearchKey {
    pub(super) plaintext: SecretSearchKey,
    pub(super) key_version: String,
}

impl ResolvedSearchKey {
    pub(super) fn validate(&self) -> AppResult<()> {
        if !search_version_identifier_is_valid(&self.key_version) {
            return Err(AppError::ServiceUnavailable(format!(
                "Search-key provider key version must be 1..={MAX_SEARCH_VERSION_ID_CHARS} ASCII identifier characters"
            )));
        }
        Ok(())
    }
}

impl fmt::Debug for ResolvedSearchKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedSearchKey")
            .field("plaintext", &self.plaintext)
            .field("key_version", &self.key_version)
            .finish()
    }
}

#[async_trait]
pub(super) trait SearchKeyProvider: Send + Sync {
    /// Resolve the independent per-user HIGH search key.
    ///
    /// Implementations must not reuse memo-encryption DEKs or return a key
    /// shared across owner partitions.
    async fn resolve_search_key(&self, owner_partition: Uuid) -> AppResult<ResolvedSearchKey>;
}

/// Deterministically derives one 384-bit search key per owner partition from a
/// provider-resolved owner-scoped seed using HKDF-SHA-384.
///
/// This keeps blind tokens stable for the same owner/seed version and adds
/// local protocol/domain separation after the external key boundary. A future
/// managed-KMS adapter can produce the seed with a provider-side HMAC/PRF
/// operation, keeping long-lived root key material outside memo_server.
pub(super) struct HkdfSearchKeyProvider {
    seeds: Arc<dyn SearchKeySeedProvider>,
}

impl HkdfSearchKeyProvider {
    pub(super) fn new(seeds: Arc<dyn SearchKeySeedProvider>) -> Self {
        Self { seeds }
    }
}

#[async_trait]
impl SearchKeyProvider for HkdfSearchKeyProvider {
    async fn resolve_search_key(&self, owner_partition: Uuid) -> AppResult<ResolvedSearchKey> {
        let seed = self.seeds.resolve_search_seed(owner_partition).await?;
        seed.validate()?;

        let salt = hkdf::Salt::new(hkdf::HKDF_SHA384, SEARCH_KEY_DERIVATION_SALT);
        let prk = salt.extract(seed.plaintext.expose());
        let owner: &[u8] = owner_partition.as_bytes();
        let info = [SEARCH_USER_KEY_INFO, seed.key_version.as_bytes(), owner];
        let okm = prk.expand(&info, hkdf::HKDF_SHA384).map_err(|_| {
            AppError::InternalServerError("Failed to derive HIGH per-user search key".into())
        })?;

        let mut key_bytes = [0u8; SEARCH_KEY_BYTES];
        okm.fill(&mut key_bytes).map_err(|_| {
            AppError::InternalServerError("Failed to materialize HIGH per-user search key".into())
        })?;

        let resolved = ResolvedSearchKey {
            plaintext: SecretSearchKey::new(key_bytes),
            key_version: seed.key_version,
        };
        resolved.validate()?;
        Ok(resolved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixedSeedProvider {
        bytes: [u8; SEARCH_KEY_SEED_BYTES],
        version: String,
    }

    #[async_trait]
    impl SearchKeySeedProvider for FixedSeedProvider {
        async fn resolve_search_seed(
            &self,
            _owner_partition: Uuid,
        ) -> AppResult<ResolvedSearchKeySeed> {
            Ok(ResolvedSearchKeySeed {
                plaintext: SecretSearchKeySeed::new(self.bytes),
                key_version: self.version.clone(),
            })
        }
    }

    fn hkdf_provider(byte: u8, version: &str) -> HkdfSearchKeyProvider {
        HkdfSearchKeyProvider::new(Arc::new(FixedSeedProvider {
            bytes: [byte; SEARCH_KEY_SEED_BYTES],
            version: version.into(),
        }))
    }

    #[test]
    fn search_key_debug_is_redacted() {
        let key = SecretSearchKey::new([0xAB; SEARCH_KEY_BYTES]);
        let debug = format!("{key:?}");

        assert_eq!(debug, "SecretSearchKey([REDACTED])");
        assert!(!debug.contains("171"));
        assert_eq!(key.expose(), &[0xAB; SEARCH_KEY_BYTES]);
    }

    #[test]
    fn resolved_key_requires_version() {
        let valid = ResolvedSearchKey {
            plaintext: SecretSearchKey::new([0x01; SEARCH_KEY_BYTES]),
            key_version: "search-key-v1".into(),
        };
        assert!(valid.validate().is_ok());

        let invalid = ResolvedSearchKey {
            plaintext: SecretSearchKey::new([0x01; SEARCH_KEY_BYTES]),
            key_version: " ".into(),
        };
        assert!(invalid.validate().is_err());

        let invalid = ResolvedSearchKey {
            plaintext: SecretSearchKey::new([0x01; SEARCH_KEY_BYTES]),
            key_version: "search key v1".into(),
        };
        assert!(invalid.validate().is_err());

        let invalid = ResolvedSearchKey {
            plaintext: SecretSearchKey::new([0x01; SEARCH_KEY_BYTES]),
            key_version: "x".repeat(MAX_SEARCH_VERSION_ID_CHARS + 1),
        };
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn search_seed_debug_is_redacted() {
        let seed = SecretSearchKeySeed::new([0xCD; SEARCH_KEY_SEED_BYTES]);
        let debug = format!("{seed:?}");

        assert_eq!(debug, "SecretSearchKeySeed([REDACTED])");
        assert!(!debug.contains("205"));
    }

    #[tokio::test]
    async fn hkdf_search_key_is_stable_for_same_owner_and_seed_version() {
        let provider = hkdf_provider(0x11, "search-seed-v1");
        let owner = Uuid::new_v4();

        let first = provider.resolve_search_key(owner).await.unwrap();
        let second = provider.resolve_search_key(owner).await.unwrap();

        assert_eq!(first.plaintext.expose(), second.plaintext.expose());
        assert_eq!(first.key_version, "search-seed-v1");
    }

    #[tokio::test]
    async fn hkdf_search_key_is_owner_scoped() {
        let provider = hkdf_provider(0x22, "search-seed-v1");

        let first = provider.resolve_search_key(Uuid::new_v4()).await.unwrap();
        let second = provider.resolve_search_key(Uuid::new_v4()).await.unwrap();

        assert_ne!(first.plaintext.expose(), second.plaintext.expose());
    }

    #[tokio::test]
    async fn hkdf_search_key_changes_on_seed_rotation() {
        let owner = Uuid::new_v4();
        let first = hkdf_provider(0x33, "search-seed-v1")
            .resolve_search_key(owner)
            .await
            .unwrap();
        let second = hkdf_provider(0x44, "search-seed-v2")
            .resolve_search_key(owner)
            .await
            .unwrap();

        assert_ne!(first.plaintext.expose(), second.plaintext.expose());
        assert_ne!(first.key_version, second.key_version);
    }

    #[tokio::test]
    async fn hkdf_search_key_rejects_invalid_seed_version() {
        let provider = hkdf_provider(0x55, "search seed v1");

        assert!(matches!(
            provider.resolve_search_key(Uuid::new_v4()).await,
            Err(AppError::ServiceUnavailable(_))
        ));
    }
}
