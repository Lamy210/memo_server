// Staged search-key boundary. This remains runtime-unreachable until
// SEARCH-HIGH-1 is deployed and protected Manticore projection is wired.
#![allow(dead_code)]

use std::fmt;

use async_trait::async_trait;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::{
    application::crypto_search::{search_version_identifier_is_valid, MAX_SEARCH_VERSION_ID_CHARS},
    error::{AppError, AppResult},
};

pub(super) const SEARCH_KEY_BYTES: usize = 48;

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
