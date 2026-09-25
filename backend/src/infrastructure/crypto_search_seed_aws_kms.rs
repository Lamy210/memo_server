// AWS KMS implementation of the staged managed HIGH search PRF boundary.
#![allow(dead_code)]

use async_trait::async_trait;
use aws_sdk_kms::{primitives::Blob, types::MacAlgorithmSpec, Client};
use zeroize::Zeroizing;

use crate::error::{AppError, AppResult};

use super::crypto_search_seed_provider::{
    ManagedPrfClientBinding, ManagedSearchSeedPrfClient, MANAGED_PRF_PROVIDER_AWS_KMS,
};

const MAX_KMS_KEY_ARN_BYTES: usize = 2048;
const HMAC_SHA384_BYTES: usize = 48;

pub(super) struct AwsKmsSearchSeedPrfClient {
    client: Client,
    key_arn: String,
}

impl AwsKmsSearchSeedPrfClient {
    pub(super) fn new(client: Client, key_arn: String) -> AppResult<Self> {
        validate_pinned_kms_key_arn(&key_arn)?;

        Ok(Self { client, key_arn })
    }

    pub(super) async fn verify_key_configuration(&self) -> AppResult<()> {
        let response = self
            .client
            .describe_key()
            .key_id(&self.key_arn)
            .send()
            .await
            .map_err(|_| {
                AppError::ServiceUnavailable(
                    "AWS KMS HIGH search DescribeKey preflight failed".into(),
                )
            })?;
        let metadata = response.key_metadata().ok_or_else(|| {
            AppError::ServiceUnavailable(
                "AWS KMS HIGH search DescribeKey omitted key metadata".into(),
            )
        })?;
        let mac_algorithms: Vec<&str> = metadata
            .mac_algorithms()
            .iter()
            .map(|algorithm| algorithm.as_str())
            .collect();

        validate_kms_search_key_metadata(
            &self.key_arn,
            metadata.arn(),
            metadata.enabled(),
            metadata.key_spec().map(|value| value.as_str()),
            metadata.key_usage().map(|value| value.as_str()),
            metadata.key_state().map(|value| value.as_str()),
            &mac_algorithms,
        )
    }
}

#[async_trait]
impl ManagedSearchSeedPrfClient for AwsKmsSearchSeedPrfClient {
    fn binding(&self) -> ManagedPrfClientBinding<'_> {
        ManagedPrfClientBinding {
            provider: MANAGED_PRF_PROVIDER_AWS_KMS,
            immutable_key_reference: &self.key_arn,
        }
    }

    async fn hmac_sha384(&self, message: &[u8]) -> AppResult<Zeroizing<Vec<u8>>> {
        if message.is_empty() {
            return Err(AppError::ServiceUnavailable(
                "AWS KMS HIGH search PRF message must not be empty".into(),
            ));
        }
        if message.len() > 4096 {
            return Err(AppError::ServiceUnavailable(
                "AWS KMS HIGH search PRF message exceeds the GenerateMac limit".into(),
            ));
        }

        let response = self
            .client
            .generate_mac()
            .key_id(&self.key_arn)
            .message(Blob::new(message))
            .mac_algorithm(MacAlgorithmSpec::HmacSha384)
            .send()
            .await
            .map_err(|_| {
                AppError::ServiceUnavailable("AWS KMS HIGH search PRF GenerateMac failed".into())
            })?;

        if response.key_id() != Some(self.key_arn.as_str()) {
            return Err(AppError::ServiceUnavailable(
                "AWS KMS HIGH search PRF response key identity mismatch".into(),
            ));
        }

        if response.mac_algorithm() != Some(&MacAlgorithmSpec::HmacSha384) {
            return Err(AppError::ServiceUnavailable(
                "AWS KMS HIGH search PRF response algorithm mismatch".into(),
            ));
        }

        let mac = response.mac().ok_or_else(|| {
            AppError::ServiceUnavailable(
                "AWS KMS HIGH search PRF response omitted MAC bytes".into(),
            )
        })?;
        if mac.as_ref().len() != HMAC_SHA384_BYTES {
            return Err(AppError::ServiceUnavailable(format!(
                "AWS KMS HIGH search PRF returned {} MAC bytes; expected {HMAC_SHA384_BYTES}",
                mac.as_ref().len()
            )));
        }

        Ok(Zeroizing::new(mac.as_ref().to_vec()))
    }
}

fn validate_kms_search_key_metadata(
    expected_arn: &str,
    actual_arn: Option<&str>,
    enabled: bool,
    key_spec: Option<&str>,
    key_usage: Option<&str>,
    key_state: Option<&str>,
    mac_algorithms: &[&str],
) -> AppResult<()> {
    if actual_arn != Some(expected_arn) {
        return Err(AppError::ServiceUnavailable(
            "AWS KMS HIGH search DescribeKey identity mismatch".into(),
        ));
    }
    if !enabled || key_state != Some("Enabled") {
        return Err(AppError::ServiceUnavailable(
            "AWS KMS HIGH search key is not enabled".into(),
        ));
    }
    if key_spec != Some("HMAC_384") {
        return Err(AppError::ServiceUnavailable(
            "AWS KMS HIGH search key must use KeySpec HMAC_384".into(),
        ));
    }
    if key_usage != Some("GENERATE_VERIFY_MAC") {
        return Err(AppError::ServiceUnavailable(
            "AWS KMS HIGH search key must use GENERATE_VERIFY_MAC".into(),
        ));
    }
    if !mac_algorithms.contains(&"HMAC_SHA_384") {
        return Err(AppError::ServiceUnavailable(
            "AWS KMS HIGH search key does not advertise HMAC_SHA_384".into(),
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
            "AWS KMS HIGH search PRF requires a structurally valid pinned KMS key ARN, not an alias"
                .into(),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY_ARN: &str =
        "arn:aws:kms:ap-northeast-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab";

    fn valid_metadata() -> AppResult<()> {
        validate_kms_search_key_metadata(
            KEY_ARN,
            Some(KEY_ARN),
            true,
            Some("HMAC_384"),
            Some("GENERATE_VERIFY_MAC"),
            Some("Enabled"),
            &["HMAC_SHA_384"],
        )
    }

    #[test]
    fn accepts_enabled_hmac384_generate_verify_key_metadata() {
        assert!(valid_metadata().is_ok());
    }

    #[test]
    fn rejects_mismatched_or_unusable_kms_key_metadata() {
        assert!(validate_kms_search_key_metadata(
            KEY_ARN,
            Some("arn:aws:kms:ap-northeast-1:111122223333:key/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"),
            true,
            Some("HMAC_384"),
            Some("GENERATE_VERIFY_MAC"),
            Some("Enabled"),
            &["HMAC_SHA_384"],
        )
        .is_err());
        assert!(validate_kms_search_key_metadata(
            KEY_ARN,
            Some(KEY_ARN),
            false,
            Some("HMAC_384"),
            Some("GENERATE_VERIFY_MAC"),
            Some("Disabled"),
            &["HMAC_SHA_384"],
        )
        .is_err());
        assert!(validate_kms_search_key_metadata(
            KEY_ARN,
            Some(KEY_ARN),
            true,
            Some("HMAC_256"),
            Some("GENERATE_VERIFY_MAC"),
            Some("Enabled"),
            &["HMAC_SHA_256"],
        )
        .is_err());
        assert!(validate_kms_search_key_metadata(
            KEY_ARN,
            Some(KEY_ARN),
            true,
            Some("HMAC_384"),
            Some("SIGN_VERIFY"),
            Some("Enabled"),
            &["HMAC_SHA_384"],
        )
        .is_err());
        assert!(validate_kms_search_key_metadata(
            KEY_ARN,
            Some(KEY_ARN),
            true,
            Some("HMAC_384"),
            Some("GENERATE_VERIFY_MAC"),
            Some("Enabled"),
            &[],
        )
        .is_err());
    }

    #[test]
    fn accepts_pinned_kms_key_arn() {
        assert!(validate_pinned_kms_key_arn(KEY_ARN).is_ok());
        assert!(validate_pinned_kms_key_arn(
            "arn:aws-us-gov:kms:us-gov-west-1:111122223333:key/mrk-0123456789abcdef0123456789abcdef"
        )
        .is_ok());
    }

    #[test]
    fn rejects_aliases_unpinned_and_structurally_invalid_identifiers() {
        for invalid in [
            "",
            "alias/memo-search",
            "1234abcd-12ab-34cd-56ef-1234567890ab",
            "arn:aws:kms:ap-northeast-1:111122223333:alias/memo-search",
            " arn:aws:kms:ap-northeast-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab",
            "arn:AWS:kms:ap-northeast-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab",
            "arn:aws:not-kms:ap-northeast-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab",
            "arn:aws:kms:AP-NORTHEAST-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab",
            "arn:aws:kms:ap-northeast-1:not-an-account:key/1234abcd-12ab-34cd-56ef-1234567890ab",
            "arn:aws:kms:ap-northeast-1:111122223333:key/",
            "arn:aws:kms:ap-northeast-1:111122223333:key/key/extra",
            "arn:aws:kms:ap-northeast-1:111122223333:key/key with space",
        ] {
            assert!(validate_pinned_kms_key_arn(invalid).is_err(), "{invalid}");
        }
    }
}
