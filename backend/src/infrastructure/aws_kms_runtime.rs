use std::time::Duration;

use aws_config::timeout::TimeoutConfig;

const AWS_KMS_OPERATION_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) fn aws_kms_timeout_config() -> TimeoutConfig {
    TimeoutConfig::builder()
        .operation_timeout(AWS_KMS_OPERATION_TIMEOUT)
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aws_kms_operation_timeout_is_bounded_without_overriding_attempt_policy() {
        let config = aws_kms_timeout_config();

        assert_eq!(
            config.operation_timeout(),
            Some(AWS_KMS_OPERATION_TIMEOUT)
        );
        assert_eq!(config.operation_attempt_timeout(), None);
    }
}
