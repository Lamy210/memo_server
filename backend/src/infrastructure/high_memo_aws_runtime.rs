// Shared composition boundary for staged MEMO-HIGH-1 cryptography.
//
// Building this runtime never changes the persisted memo route. Operator
// migration uses the staging trait while application startup may inject the
// request-path trait behind the still-legacy-by-default route boundary.
#![allow(dead_code)]

use std::sync::Arc;

use crate::{
    application::{
        crypto::HighMemoCryptography, crypto_migration::HighMemoStagingCryptography,
    },
    config::HighMemoCryptoConfig,
    error::{AppError, AppResult},
};

#[cfg(feature = "aws-kms-memo")]
use super::{crypto::RingHighMemoCryptography, crypto_keys_aws_kms::AwsKmsDataKeyProvider};

pub(crate) struct HighMemoStagingRuntimeHandle {
    staging_cryptography: Option<Arc<dyn HighMemoStagingCryptography>>,
    request_cryptography: Option<Arc<dyn HighMemoCryptography>>,
}

impl HighMemoStagingRuntimeHandle {
    pub(crate) async fn build(config: &HighMemoCryptoConfig) -> AppResult<Self> {
        match config {
            HighMemoCryptoConfig::Disabled => Ok(Self {
                staging_cryptography: None,
                request_cryptography: None,
            }),
            HighMemoCryptoConfig::AwsKms { .. } => {
                let (staging_cryptography, request_cryptography) =
                    build_aws_kms_cryptography(config).await?;
                Ok(Self {
                    staging_cryptography: Some(staging_cryptography),
                    request_cryptography: Some(request_cryptography),
                })
            }
        }
    }

    /// Compatibility accessor for migration/operator code.
    pub(crate) fn cryptography(&self) -> Option<Arc<dyn HighMemoStagingCryptography>> {
        self.staging_cryptography.clone()
    }

    pub(crate) fn request_cryptography(&self) -> Option<Arc<dyn HighMemoCryptography>> {
        self.request_cryptography.clone()
    }
}

#[cfg(feature = "aws-kms-memo")]
async fn build_aws_kms_cryptography(
    config: &HighMemoCryptoConfig,
) -> AppResult<(
    Arc<dyn HighMemoStagingCryptography>,
    Arc<dyn HighMemoCryptography>,
)> {
    use aws_config::BehaviorVersion;
    use aws_sdk_kms::config::Region;

    let HighMemoCryptoConfig::AwsKms {
        region,
        active_key_version,
        key_versions,
    } = config
    else {
        return Err(AppError::ServiceUnavailable(
            "AWS KMS MEMO-HIGH-1 runtime requested while memo cryptography is disabled".into(),
        ));
    };

    // Region is explicit and config validation requires every pinned KMS ARN to
    // use the same Region. Credentials remain in the standard refreshable AWS
    // provider chain and are never copied into application configuration.
    let sdk_config = aws_config::defaults(BehaviorVersion::latest())
        .region(Region::new(region.clone()))
        .load()
        .await;

    if sdk_config.region().map(|value| value.as_ref()) != Some(region.as_str()) {
        return Err(AppError::ServiceUnavailable(
            "AWS MEMO-HIGH-1 SDK configuration did not retain the configured KMS Region".into(),
        ));
    }

    let kms = aws_sdk_kms::Client::new(&sdk_config);
    let provider =
        AwsKmsDataKeyProvider::new(kms, active_key_version.clone(), key_versions.clone())?;
    provider.verify_key_configuration().await?;

    let cryptography = Arc::new(RingHighMemoCryptography::new(Arc::new(provider)));
    let staging: Arc<dyn HighMemoStagingCryptography> = cryptography.clone();
    let request: Arc<dyn HighMemoCryptography> = cryptography;
    Ok((staging, request))
}

#[cfg(not(feature = "aws-kms-memo"))]
async fn build_aws_kms_cryptography(
    _config: &HighMemoCryptoConfig,
) -> AppResult<(
    Arc<dyn HighMemoStagingCryptography>,
    Arc<dyn HighMemoCryptography>,
)> {
    Err(AppError::ServiceUnavailable(
        "MEMO-HIGH-1 AWS KMS staging runtime requires feature `aws-kms-memo`".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn disabled_memo_crypto_does_not_require_aws_runtime() {
        let handle = HighMemoStagingRuntimeHandle::build(&HighMemoCryptoConfig::Disabled)
            .await
            .unwrap();

        assert!(handle.cryptography().is_none());
        assert!(handle.request_cryptography().is_none());
    }
}
