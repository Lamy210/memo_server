use std::{collections::BTreeMap, fmt};

use async_trait::async_trait;
use zeroize::Zeroizing;

use crate::{
    application::crypto::HighMemoAad,
    error::{AppError, AppResult},
};

pub(super) const DATA_KEY_BYTES: usize = 32;

pub(super) struct SecretDataKey(Zeroizing<[u8; DATA_KEY_BYTES]>);

impl SecretDataKey {
    pub(super) fn new(bytes: [u8; DATA_KEY_BYTES]) -> Self {
        Self(Zeroizing::new(bytes))
    }

    pub(super) fn expose(&self) -> &[u8; DATA_KEY_BYTES] {
        &self.0
    }
}

impl fmt::Debug for SecretDataKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretDataKey([REDACTED])")
    }
}

pub(super) struct GeneratedDataKey {
    pub(super) plaintext: SecretDataKey,
    pub(super) wrapped_dek: Vec<u8>,
    pub(super) key_version: String,
}

impl GeneratedDataKey {
    pub(super) fn validate(&self) -> AppResult<()> {
        if self.wrapped_dek.is_empty() {
            return Err(AppError::ServiceUnavailable(
                "Data-key provider returned an empty wrapped DEK".into(),
            ));
        }
        if self.key_version.trim().is_empty() {
            return Err(AppError::ServiceUnavailable(
                "Data-key provider returned an empty key version".into(),
            ));
        }
        Ok(())
    }
}

impl fmt::Debug for GeneratedDataKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GeneratedDataKey")
            .field("plaintext", &self.plaintext)
            .field("wrapped_dek_len", &self.wrapped_dek.len())
            .field("key_version", &self.key_version)
            .finish()
    }
}

#[async_trait]
pub(super) trait DataKeyProvider: Send + Sync {
    /// Generate a fresh 256-bit plaintext DEK plus the provider-wrapped copy
    /// that may be persisted with one memo version.
    async fn generate_data_key(&self, aad: &HighMemoAad) -> AppResult<GeneratedDataKey>;

    /// Unwrap one persisted DEK. Implementations must bind the same non-secret
    /// context used at generation/wrapping time and fail closed on mismatch.
    async fn unwrap_data_key(
        &self,
        wrapped_dek: &[u8],
        key_version: &str,
        aad: &HighMemoAad,
    ) -> AppResult<SecretDataKey>;
}

/// Canonical non-secret context for a wrapping provider such as a managed KMS.
///
/// The same logical fields are authenticated by memo AEAD AAD. A KMS adapter
/// can additionally bind this map as its encryption context without exposing
/// memo plaintext.
pub(super) fn data_key_encryption_context(aad: &HighMemoAad) -> AppResult<BTreeMap<String, String>> {
    aad.validate()?;

    Ok(BTreeMap::from([
        ("domain".into(), "memo_server:high".into()),
        ("owner_partition".into(), aad.owner_partition.to_string()),
        ("memo_id".into(), aad.memo_id.to_string()),
        ("version".into(), aad.version.to_string()),
        ("schema_version".into(), aad.schema_version.to_string()),
        ("crypto_suite_id".into(), aad.crypto_suite_id.clone()),
    ]))
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;
    use crate::application::crypto::{
        MEMO_HIGH_SCHEMA_VERSION, MEMO_HIGH_SUITE_ID,
    };

    fn aad() -> HighMemoAad {
        HighMemoAad {
            owner_partition: Uuid::new_v4(),
            memo_id: Uuid::new_v4(),
            version: 7,
            schema_version: MEMO_HIGH_SCHEMA_VERSION,
            crypto_suite_id: MEMO_HIGH_SUITE_ID.into(),
        }
    }

    #[test]
    fn secret_data_key_debug_is_redacted() {
        let key = SecretDataKey::new([0xAB; DATA_KEY_BYTES]);
        let debug = format!("{key:?}");

        assert_eq!(debug, "SecretDataKey([REDACTED])");
        assert!(!debug.contains("171"));
        assert_eq!(key.expose(), &[0xAB; DATA_KEY_BYTES]);
    }

    #[test]
    fn generated_data_key_requires_wrapped_material_and_version() {
        let valid = GeneratedDataKey {
            plaintext: SecretDataKey::new([0x01; DATA_KEY_BYTES]),
            wrapped_dek: vec![0x02; 48],
            key_version: "provider-key-v1".into(),
        };
        assert!(valid.validate().is_ok());

        let missing_wrapped = GeneratedDataKey {
            plaintext: SecretDataKey::new([0x01; DATA_KEY_BYTES]),
            wrapped_dek: vec![],
            key_version: "provider-key-v1".into(),
        };
        assert!(missing_wrapped.validate().is_err());

        let missing_version = GeneratedDataKey {
            plaintext: SecretDataKey::new([0x01; DATA_KEY_BYTES]),
            wrapped_dek: vec![0x02; 48],
            key_version: " ".into(),
        };
        assert!(missing_version.validate().is_err());
    }

    #[test]
    fn wrapping_context_contains_only_authenticated_operational_metadata() {
        let aad = aad();
        let context = data_key_encryption_context(&aad).unwrap();

        assert_eq!(context.get("domain").unwrap(), "memo_server:high");
        assert_eq!(
            context.get("owner_partition").unwrap(),
            &aad.owner_partition.to_string()
        );
        assert_eq!(context.get("memo_id").unwrap(), &aad.memo_id.to_string());
        assert_eq!(context.get("version").unwrap(), "7");
        assert_eq!(
            context.get("schema_version").unwrap(),
            &MEMO_HIGH_SCHEMA_VERSION.to_string()
        );
        assert_eq!(
            context.get("crypto_suite_id").unwrap(),
            MEMO_HIGH_SUITE_ID
        );
        assert!(!context.contains_key("title"));
        assert!(!context.contains_key("content"));
        assert!(!context.contains_key("tags"));
    }
}
