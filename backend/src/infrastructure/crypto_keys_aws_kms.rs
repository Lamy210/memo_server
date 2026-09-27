// AWS KMS implementation of the staged MEMO-HIGH-1 data-key provider.
//
// This adapter is deliberately feature-gated and not request-path wired yet.
// It keeps provider locators out of persisted envelopes by mapping
// application-owned key-version aliases to pinned KMS key ARNs.
#![allow(dead_code)]

use std::collections::{BTreeMap, HashSet};

use async_trait::async_trait;
use aws_sdk_kms::{
    primitives::Blob,
    types::{DataKeySpec, EncryptionAlgorithmSpec},
    Client,
};
use zeroize::Zeroizing;

use crate::{
    application::crypto::{memo_key_version_identifier_is_valid, HighMemoAad},
    error::{AppError, AppResult},
};

use super::crypto_keys::{
    data_key_encryption_context, DataKeyProvider, GeneratedDataKey, SecretDataKey, DATA_KEY_BYTES,
};

const MAX_KMS_KEY_ARN_BYTES: usize = 2048;
const MAX_KMS_CIPHERTEXT_BLOB_BYTES: usize = 6144;
const MAX_CONFIGURED_KEY_VERSIONS: usize = 32;

pub(super) struct AwsKmsDataKeyProvider {
    client: Client,
    active_key_version: String,
    key_arns: BTreeMap<String, String>,
}

impl AwsKmsDataKeyProvider {
    pub(super) fn new(
        client: Client,
        active_key_version: String,
        key_arns: BTreeMap<String, String>,
    ) -> AppResult<Self> {
        validate_key_ring(&active_key_version, &key_arns)?;

        Ok(Self {
            client,
            active_key_version,
            key_arns,
        })
    }

    /// Validate every configured read/write key before this provider is made
    /// runtime reachable. Old aliases remain necessary for decrypting envelopes
    /// created before rotation.
    pub(super) async fn verify_key_configuration(&self) -> AppResult<()> {
        for key_arn in self.key_arns.values() {
            let response = self
                .client
                .describe_key()
                .key_id(key_arn)
                .send()
                .await
                .map_err(|_| {
                    AppError::ServiceUnavailable(
                        "AWS KMS MEMO-HIGH-1 DescribeKey preflight failed".into(),
                    )
                })?;
            let metadata = response.key_metadata().ok_or_else(|| {
                AppError::ServiceUnavailable(
                    "AWS KMS MEMO-HIGH-1 DescribeKey omitted key metadata".into(),
                )
            })?;
            let encryption_algorithms: Vec<&str> = metadata
                .encryption_algorithms()
                .iter()
                .map(|algorithm| algorithm.as_str())
                .collect();

            validate_kms_encryption_key_metadata(
                key_arn,
                metadata.arn(),
                metadata.enabled(),
                metadata.key_spec().map(|value| value.as_str()),
                metadata.key_usage().map(|value| value.as_str()),
                metadata.key_state().map(|value| value.as_str()),
                &encryption_algorithms,
            )?;
        }

        Ok(())
    }

    fn key_arn_for_version(&self, key_version: &str) -> AppResult<&str> {
        if !memo_key_version_identifier_is_valid(key_version) {
            return Err(AppError::DatabaseError(
                "Encrypted memo key version is structurally invalid".into(),
            ));
        }

        self.key_arns
            .get(key_version)
            .map(String::as_str)
            .ok_or_else(|| {
                AppError::ServiceUnavailable(format!(
                    "No AWS KMS MEMO-HIGH-1 key is configured for key version {key_version}"
                ))
            })
    }
}

#[async_trait]
impl DataKeyProvider for AwsKmsDataKeyProvider {
    async fn generate_data_key(&self, aad: &HighMemoAad) -> AppResult<GeneratedDataKey> {
        let key_arn = self.key_arn_for_version(&self.active_key_version)?;
        let context = data_key_encryption_context(aad)?
            .into_iter()
            .collect::<std::collections::HashMap<_, _>>();

        let response = self
            .client
            .generate_data_key()
            .key_id(key_arn)
            .key_spec(DataKeySpec::Aes256)
            .set_encryption_context(Some(context))
            .send()
            .await
            .map_err(|_| {
                AppError::ServiceUnavailable(
                    "AWS KMS MEMO-HIGH-1 GenerateDataKey failed".into(),
                )
            })?;

        if response.key_id() != Some(key_arn) {
            return Err(AppError::ServiceUnavailable(
                "AWS KMS MEMO-HIGH-1 GenerateDataKey response key identity mismatch".into(),
            ));
        }

        let plaintext_blob = response.plaintext.ok_or_else(|| {
            AppError::ServiceUnavailable(
                "AWS KMS MEMO-HIGH-1 GenerateDataKey omitted plaintext DEK".into(),
            )
        })?;
        let plaintext = Zeroizing::new(plaintext_blob.into_inner());
        let plaintext = secret_data_key_from_bytes(
            &plaintext,
            "AWS KMS MEMO-HIGH-1 GenerateDataKey returned an invalid plaintext DEK length",
        )?;

        let wrapped_dek = response
            .ciphertext_blob
            .ok_or_else(|| {
                AppError::ServiceUnavailable(
                    "AWS KMS MEMO-HIGH-1 GenerateDataKey omitted wrapped DEK".into(),
                )
            })?
            .into_inner();
        validate_generated_wrapped_dek(&wrapped_dek)?;

        let generated = GeneratedDataKey {
            plaintext,
            wrapped_dek,
            key_version: self.active_key_version.clone(),
        };
        generated.validate()?;
        Ok(generated)
    }

    async fn unwrap_data_key(
        &self,
        wrapped_dek: &[u8],
        key_version: &str,
        aad: &HighMemoAad,
    ) -> AppResult<SecretDataKey> {
        validate_persisted_wrapped_dek(wrapped_dek)?;
        let key_arn = self.key_arn_for_version(key_version)?;
        let context = data_key_encryption_context(aad)?
            .into_iter()
            .collect::<std::collections::HashMap<_, _>>();

        let response = self
            .client
            .decrypt()
            .key_id(key_arn)
            .ciphertext_blob(Blob::new(wrapped_dek))
            .encryption_algorithm(EncryptionAlgorithmSpec::SymmetricDefault)
            .set_encryption_context(Some(context))
            .send()
            .await
            .map_err(|_| {
                AppError::ServiceUnavailable("AWS KMS MEMO-HIGH-1 Decrypt failed".into())
            })?;

        if response.key_id() != Some(key_arn) {
            return Err(AppError::ServiceUnavailable(
                "AWS KMS MEMO-HIGH-1 Decrypt response key identity mismatch".into(),
            ));
        }
        if response.encryption_algorithm() != Some(&EncryptionAlgorithmSpec::SymmetricDefault) {
            return Err(AppError::ServiceUnavailable(
                "AWS KMS MEMO-HIGH-1 Decrypt response algorithm mismatch".into(),
            ));
        }

        let plaintext_blob = response.plaintext.ok_or_else(|| {
            AppError::ServiceUnavailable("AWS KMS MEMO-HIGH-1 Decrypt omitted plaintext DEK".into())
        })?;
        let plaintext = Zeroizing::new(plaintext_blob.into_inner());
        secret_data_key_from_bytes(
            &plaintext,
            "AWS KMS MEMO-HIGH-1 Decrypt returned an invalid plaintext DEK length",
        )
    }
}

fn validate_key_ring(
    active_key_version: &str,
    key_arns: &BTreeMap<String, String>,
) -> AppResult<()> {
    if key_arns.is_empty() || key_arns.len() > MAX_CONFIGURED_KEY_VERSIONS {
        return Err(AppError::ServiceUnavailable(format!(
            "AWS KMS MEMO-HIGH-1 key ring must contain 1..={MAX_CONFIGURED_KEY_VERSIONS} versions"
        )));
    }
    if !memo_key_version_identifier_is_valid(active_key_version) {
        return Err(AppError::ServiceUnavailable(
            "AWS KMS MEMO-HIGH-1 active key version is invalid".into(),
        ));
    }
    if !key_arns.contains_key(active_key_version) {
        return Err(AppError::ServiceUnavailable(
            "AWS KMS MEMO-HIGH-1 active key version is not present in the key ring".into(),
        ));
    }

    let mut unique_arns = HashSet::with_capacity(key_arns.len());
    for (key_version, key_arn) in key_arns {
        if !memo_key_version_identifier_is_valid(key_version) {
            return Err(AppError::ServiceUnavailable(format!(
                "AWS KMS MEMO-HIGH-1 key version {key_version:?} is invalid"
            )));
        }
        validate_pinned_kms_key_arn(key_arn)?;
        if !unique_arns.insert(key_arn.as_str()) {
            return Err(AppError::ServiceUnavailable(
                "AWS KMS MEMO-HIGH-1 key ring must not map multiple aliases to the same KMS key ARN"
                    .into(),
            ));
        }
    }

    Ok(())
}

fn wrapped_dek_length_is_valid(wrapped_dek: &[u8]) -> bool {
    !wrapped_dek.is_empty() && wrapped_dek.len() <= MAX_KMS_CIPHERTEXT_BLOB_BYTES
}

fn validate_generated_wrapped_dek(wrapped_dek: &[u8]) -> AppResult<()> {
    if !wrapped_dek_length_is_valid(wrapped_dek) {
        return Err(AppError::ServiceUnavailable(format!(
            "AWS KMS MEMO-HIGH-1 GenerateDataKey returned a wrapped DEK outside 1..={MAX_KMS_CIPHERTEXT_BLOB_BYTES} bytes"
        )));
    }
    Ok(())
}

fn validate_persisted_wrapped_dek(wrapped_dek: &[u8]) -> AppResult<()> {
    if !wrapped_dek_length_is_valid(wrapped_dek) {
        return Err(AppError::DatabaseError(format!(
            "AWS KMS MEMO-HIGH-1 persisted wrapped DEK must be 1..={MAX_KMS_CIPHERTEXT_BLOB_BYTES} bytes"
        )));
    }
    Ok(())
}

fn secret_data_key_from_bytes(bytes: &[u8], error_message: &str) -> AppResult<SecretDataKey> {
    if bytes.len() != DATA_KEY_BYTES {
        return Err(AppError::ServiceUnavailable(error_message.into()));
    }

    let mut key = [0u8; DATA_KEY_BYTES];
    key.copy_from_slice(bytes);
    Ok(SecretDataKey::new(key))
}

fn validate_kms_encryption_key_metadata(
    expected_arn: &str,
    actual_arn: Option<&str>,
    enabled: bool,
    key_spec: Option<&str>,
    key_usage: Option<&str>,
    key_state: Option<&str>,
    encryption_algorithms: &[&str],
) -> AppResult<()> {
    if actual_arn != Some(expected_arn) {
        return Err(AppError::ServiceUnavailable(
            "AWS KMS MEMO-HIGH-1 DescribeKey identity mismatch".into(),
        ));
    }
    if !enabled || key_state != Some("Enabled") {
        return Err(AppError::ServiceUnavailable(
            "AWS KMS MEMO-HIGH-1 key is not enabled".into(),
        ));
    }
    if key_spec != Some("SYMMETRIC_DEFAULT") {
        return Err(AppError::ServiceUnavailable(
            "AWS KMS MEMO-HIGH-1 key must use KeySpec SYMMETRIC_DEFAULT".into(),
        ));
    }
    if key_usage != Some("ENCRYPT_DECRYPT") {
        return Err(AppError::ServiceUnavailable(
            "AWS KMS MEMO-HIGH-1 key must use ENCRYPT_DECRYPT".into(),
        ));
    }
    if !encryption_algorithms.contains(&"SYMMETRIC_DEFAULT") {
        return Err(AppError::ServiceUnavailable(
            "AWS KMS MEMO-HIGH-1 key does not advertise SYMMETRIC_DEFAULT encryption".into(),
        ));
    }

    Ok(())
}

fn validate_pinned_kms_key_arn(key_arn: &str) -> AppResult<()> {
    let parts: Vec<&str> = key_arn.splitn(6, ':').collect();
    let partition_valid = parts.get(1).is_some_and(|partition| {
        !partition.is_empty()
            && partition
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            && !partition.starts_with('-')
            && !partition.ends_with('-')
    });
    let region_valid = parts.get(3).is_some_and(|region| {
        !region.is_empty()
            && region
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            && !region.starts_with('-')
            && !region.ends_with('-')
    });
    let account_valid = parts.get(4).is_some_and(|account| {
        account.len() == 12 && account.bytes().all(|byte| byte.is_ascii_digit())
    });
    let resource_valid = parts.get(5).is_some_and(|resource| {
        resource.strip_prefix("key/").is_some_and(|key_id| {
            !key_id.is_empty()
                && !key_id.contains('/')
                && key_id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
    });

    let valid = !key_arn.is_empty()
        && key_arn.len() <= MAX_KMS_KEY_ARN_BYTES
        && key_arn.trim() == key_arn
        && parts.len() == 6
        && parts[0] == "arn"
        && partition_valid
        && parts[2] == "kms"
        && region_valid
        && account_valid
        && resource_valid;

    if !valid {
        return Err(AppError::ServiceUnavailable(
            "AWS KMS MEMO-HIGH-1 requires structurally valid pinned KMS key ARNs, not aliases"
                .into(),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY_ARN_V1: &str =
        "arn:aws:kms:ap-northeast-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab";
    const KEY_ARN_V2: &str =
        "arn:aws:kms:ap-northeast-1:111122223333:key/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

    fn key_ring() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("memo-key-v1".into(), KEY_ARN_V1.into()),
            ("memo-key-v2".into(), KEY_ARN_V2.into()),
        ])
    }

    #[test]
    fn accepts_versioned_key_ring_with_separate_active_alias() {
        assert!(validate_key_ring("memo-key-v2", &key_ring()).is_ok());
    }

    #[test]
    fn rejects_missing_active_alias_duplicate_arn_and_unsafe_aliases() {
        assert!(validate_key_ring("missing", &key_ring()).is_err());

        let duplicated = BTreeMap::from([
            ("memo-key-v1".into(), KEY_ARN_V1.into()),
            ("memo-key-v2".into(), KEY_ARN_V1.into()),
        ]);
        assert!(validate_key_ring("memo-key-v2", &duplicated).is_err());

        let invalid_alias = BTreeMap::from([("provider/key/arn".into(), KEY_ARN_V1.into())]);
        assert!(validate_key_ring("provider/key/arn", &invalid_alias).is_err());
    }

    #[test]
    fn rejects_empty_or_oversized_key_rings() {
        assert!(validate_key_ring("memo-key-v1", &BTreeMap::new()).is_err());

        let oversized = (0..=MAX_CONFIGURED_KEY_VERSIONS)
            .map(|index| {
                (
                    format!("memo-key-{index}"),
                    format!(
                        "arn:aws:kms:ap-northeast-1:111122223333:key/{index:032x}"
                    ),
                )
            })
            .collect();
        assert!(validate_key_ring("memo-key-0", &oversized).is_err());
    }

    #[test]
    fn accepts_pinned_kms_key_arns_and_rejects_aliases() {
        assert!(validate_pinned_kms_key_arn(KEY_ARN_V1).is_ok());
        assert!(validate_pinned_kms_key_arn(
            "arn:aws-us-gov:kms:us-gov-west-1:111122223333:key/mrk-0123456789abcdef0123456789abcdef"
        )
        .is_ok());

        for invalid in [
            "",
            "alias/memo-high",
            "1234abcd-12ab-34cd-56ef-1234567890ab",
            "arn:aws:kms:ap-northeast-1:111122223333:alias/memo-high",
            "arn:aws:kms:AP-NORTHEAST-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab",
        ] {
            assert!(validate_pinned_kms_key_arn(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn accepts_only_enabled_symmetric_encrypt_decrypt_metadata() {
        assert!(validate_kms_encryption_key_metadata(
            KEY_ARN_V1,
            Some(KEY_ARN_V1),
            true,
            Some("SYMMETRIC_DEFAULT"),
            Some("ENCRYPT_DECRYPT"),
            Some("Enabled"),
            &["SYMMETRIC_DEFAULT"],
        )
        .is_ok());

        assert!(validate_kms_encryption_key_metadata(
            KEY_ARN_V1,
            Some(KEY_ARN_V2),
            true,
            Some("SYMMETRIC_DEFAULT"),
            Some("ENCRYPT_DECRYPT"),
            Some("Enabled"),
            &["SYMMETRIC_DEFAULT"],
        )
        .is_err());
        assert!(validate_kms_encryption_key_metadata(
            KEY_ARN_V1,
            Some(KEY_ARN_V1),
            false,
            Some("SYMMETRIC_DEFAULT"),
            Some("ENCRYPT_DECRYPT"),
            Some("Disabled"),
            &["SYMMETRIC_DEFAULT"],
        )
        .is_err());
        assert!(validate_kms_encryption_key_metadata(
            KEY_ARN_V1,
            Some(KEY_ARN_V1),
            true,
            Some("HMAC_384"),
            Some("GENERATE_VERIFY_MAC"),
            Some("Enabled"),
            &["HMAC_SHA_384"],
        )
        .is_err());
    }

    #[test]
    fn wrapped_dek_and_plaintext_lengths_are_bounded() {
        assert!(validate_generated_wrapped_dek(&[0xAA]).is_ok());
        assert!(matches!(
            validate_generated_wrapped_dek(&[]),
            Err(AppError::ServiceUnavailable(_))
        ));
        assert!(matches!(
            validate_persisted_wrapped_dek(&vec![0xAA; MAX_KMS_CIPHERTEXT_BLOB_BYTES + 1]),
            Err(AppError::DatabaseError(_))
        ));

        assert!(secret_data_key_from_bytes(&[0x11; DATA_KEY_BYTES], "bad").is_ok());
        assert!(secret_data_key_from_bytes(&[0x11; DATA_KEY_BYTES - 1], "bad").is_err());
    }
}
