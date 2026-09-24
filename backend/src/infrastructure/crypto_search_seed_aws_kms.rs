// AWS KMS implementation of the staged managed HIGH search PRF boundary.
#![allow(dead_code)]

use async_trait::async_trait;
use aws_sdk_kms::{
    primitives::Blob,
    types::MacAlgorithmSpec,
    Client,
};
use zeroize::Zeroizing;

use crate::error::{AppError, AppResult};

use super::crypto_search_seed_provider::ManagedSearchSeedPrfClient;

const MAX_KMS_KEY_ARN_BYTES: usize = 2048;

pub(super) struct AwsKmsSearchSeedPrfClient {
    client: Client,
    key_arn: String,
}

impl AwsKmsSearchSeedPrfClient {
    pub(super) fn new(client: Client, key_arn: String) -> AppResult<Self> {
        validate_pinned_kms_key_arn(&key_arn)?;

        Ok(Self { client, key_arn })
    }
}

#[async_trait]
impl ManagedSearchSeedPrfClient for AwsKmsSearchSeedPrfClient {
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
                AppError::ServiceUnavailable(
                    "AWS KMS HIGH search PRF GenerateMac failed".into(),
                )
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

        Ok(Zeroizing::new(mac.as_ref().to_vec()))
    }
}

fn validate_pinned_kms_key_arn(key_arn: &str) -> AppResult<()> {
    if key_arn.is_empty()
        || key_arn.len() > MAX_KMS_KEY_ARN_BYTES
        || key_arn.trim() != key_arn
        || !key_arn.starts_with("arn:")
        || !key_arn.contains(":kms:")
        || !key_arn.contains(":key/")
        || key_arn.contains(":alias/")
        || key_arn.starts_with("alias/")
    {
        return Err(AppError::ServiceUnavailable(
            "AWS KMS HIGH search PRF requires a pinned KMS key ARN, not an alias".into(),
        ));
    }

    let Some((_, resource)) = key_arn.rsplit_once(":key/") else {
        return Err(AppError::ServiceUnavailable(
            "AWS KMS HIGH search PRF key ARN is malformed".into(),
        ));
    };
    if resource.is_empty() || resource.contains('/') || resource.chars().any(char::is_whitespace) {
        return Err(AppError::ServiceUnavailable(
            "AWS KMS HIGH search PRF key ARN is malformed".into(),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY_ARN: &str =
        "arn:aws:kms:ap-northeast-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab";

    #[test]
    fn accepts_pinned_kms_key_arn() {
        assert!(validate_pinned_kms_key_arn(KEY_ARN).is_ok());
        assert!(validate_pinned_kms_key_arn(
            "arn:aws-us-gov:kms:us-gov-west-1:111122223333:key/mrk-0123456789abcdef0123456789abcdef"
        )
        .is_ok());
    }

    #[test]
    fn rejects_aliases_and_unpinned_identifiers() {
        for invalid in [
            "",
            "alias/memo-search",
            "1234abcd-12ab-34cd-56ef-1234567890ab",
            "arn:aws:kms:ap-northeast-1:111122223333:alias/memo-search",
            " arn:aws:kms:ap-northeast-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab",
        ] {
            assert!(validate_pinned_kms_key_arn(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn rejects_malformed_key_resource() {
        assert!(validate_pinned_kms_key_arn(
            "arn:aws:kms:ap-northeast-1:111122223333:key/"
        )
        .is_err());
        assert!(validate_pinned_kms_key_arn(
            "arn:aws:kms:ap-northeast-1:111122223333:key/key/extra"
        )
        .is_err());
    }
}
