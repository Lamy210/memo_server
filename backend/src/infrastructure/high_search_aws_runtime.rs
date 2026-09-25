// Startup-only composition for the staged SEARCH-HIGH-1 runtime.
#![allow(dead_code)]

use std::{
    sync::{Arc, Weak},
    time::Duration,
};

use tokio::{task::JoinHandle, time::sleep};

use crate::{
    config::HighSearchConfig,
    error::{AppError, AppResult},
};

use super::high_search_runtime::HighSearchRuntimeStack;

#[cfg(feature = "aws-kms-search")]
use super::{
    crypto_search_seed_aws_kms::AwsKmsSearchSeedPrfClient,
    crypto_search_seed_provider::ManagedSearchSeedPrfClient,
};

/// Owns the staged HIGH search runtime for the application lifetime.
///
/// The runtime is not installed into request handling yet. Keeping it here
/// proves production provider/startup composition and owns cache maintenance
/// without widening route-level access prematurely.
pub(crate) struct HighSearchRuntimeHandle {
    stack: Option<Arc<HighSearchRuntimeStack>>,
    cache_sweeper: Option<JoinHandle<()>>,
}

impl HighSearchRuntimeHandle {
    pub(crate) async fn build(config: &HighSearchConfig, search_uri: &str) -> AppResult<Self> {
        match config {
            HighSearchConfig::Disabled => Ok(Self {
                stack: None,
                cache_sweeper: None,
            }),
            HighSearchConfig::AwsKms { .. } => {
                let prf_client = build_aws_kms_prf_client(config).await?;
                let stack = HighSearchRuntimeStack::build_managed_prf(
                    config,
                    search_uri,
                    Some(prf_client),
                )?
                .ok_or_else(|| {
                    AppError::ServiceUnavailable(
                        "HIGH search startup composition unexpectedly returned no runtime".into(),
                    )
                })?;
                let stack = Arc::new(stack);
                let cache_sweeper = spawn_cache_sweeper(&stack);

                Ok(Self {
                    stack: Some(stack),
                    cache_sweeper: Some(cache_sweeper),
                })
            }
        }
    }

    pub(crate) fn stack(&self) -> Option<Arc<HighSearchRuntimeStack>> {
        self.stack.clone()
    }
}

impl Drop for HighSearchRuntimeHandle {
    fn drop(&mut self) {
        if let Some(task) = self.cache_sweeper.take() {
            task.abort();
        }
    }
}

fn spawn_cache_sweeper(stack: &Arc<HighSearchRuntimeStack>) -> JoinHandle<()> {
    let interval = stack.cache_sweep_interval();
    let stack = Arc::downgrade(stack);

    tokio::spawn(async move {
        run_cache_sweeper(stack, interval).await;
    })
}

async fn run_cache_sweeper(stack: Weak<HighSearchRuntimeStack>, interval: Duration) {
    loop {
        sleep(interval).await;
        let Some(stack) = stack.upgrade() else {
            break;
        };
        stack.purge_expired_keys().await;
    }
}

#[cfg(feature = "aws-kms-search")]
async fn build_aws_kms_prf_client(
    config: &HighSearchConfig,
) -> AppResult<Arc<dyn ManagedSearchSeedPrfClient>> {
    use aws_config::BehaviorVersion;
    use aws_sdk_kms::config::Region;

    let HighSearchConfig::AwsKms {
        key_arn, region, ..
    } = config
    else {
        return Err(AppError::ServiceUnavailable(
            "AWS KMS HIGH search client requested while HIGH search is disabled".into(),
        ));
    };

    // Region is intentionally explicit rather than inherited from the default
    // chain. AppConfig has already verified that it matches the pinned KMS ARN.
    // Credentials still use the AWS standard provider chain and remain
    // refreshable provider state rather than application secrets.
    let sdk_config = aws_config::defaults(BehaviorVersion::latest())
        .region(Region::new(region.clone()))
        .load()
        .await;

    if sdk_config.region().map(|value| value.as_ref()) != Some(region.as_str()) {
        return Err(AppError::ServiceUnavailable(
            "AWS HIGH search SDK configuration did not retain the configured KMS Region".into(),
        ));
    }

    let kms = aws_sdk_kms::Client::new(&sdk_config);
    let client = AwsKmsSearchSeedPrfClient::new(kms, key_arn.clone())?;
    Ok(Arc::new(client))
}

#[cfg(not(feature = "aws-kms-search"))]
async fn build_aws_kms_prf_client(
    _config: &HighSearchConfig,
) -> AppResult<Arc<dyn super::crypto_search_seed_provider::ManagedSearchSeedPrfClient>> {
    Err(AppError::ServiceUnavailable(
        "HIGH search AWS KMS startup requires feature `aws-kms-search`".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn disabled_high_search_does_not_require_aws_runtime() {
        let handle =
            HighSearchRuntimeHandle::build(&HighSearchConfig::Disabled, "not-a-search-uri")
                .await
                .unwrap();

        assert!(handle.stack().is_none());
        assert!(handle.cache_sweeper.is_none());
    }

    #[tokio::test]
    async fn sweeper_stops_when_runtime_is_gone() {
        // This test covers the weak-reference termination path without
        // constructing provider/network dependencies.
        let weak: Weak<HighSearchRuntimeStack> = Weak::new();
        tokio::time::timeout(
            Duration::from_millis(50),
            run_cache_sweeper(weak, Duration::from_millis(1)),
        )
        .await
        .unwrap();
    }
}
