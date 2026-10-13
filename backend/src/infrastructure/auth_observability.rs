use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::{AppError, AppResult};

const METRICS_LOG_EVERY_ATTEMPTS: u64 = 100;
static AUTH_OBSERVABILITY: AuthObservability = AuthObservability::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AuthVerificationOutcome {
    Success,
    Unauthorized,
    DependencyUnavailable,
    InternalError,
}

impl AuthVerificationOutcome {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Unauthorized => "unauthorized",
            Self::DependencyUnavailable => "dependency_unavailable",
            Self::InternalError => "internal_error",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AuthVerificationStats {
    pub(crate) total: u64,
    pub(crate) success: u64,
    pub(crate) unauthorized: u64,
    pub(crate) dependency_unavailable: u64,
    pub(crate) internal_error: u64,
}

pub(crate) struct AuthObservability {
    total: AtomicU64,
    success: AtomicU64,
    unauthorized: AtomicU64,
    dependency_unavailable: AtomicU64,
    internal_error: AtomicU64,
}

impl AuthObservability {
    pub(crate) const fn new() -> Self {
        Self {
            total: AtomicU64::new(0),
            success: AtomicU64::new(0),
            unauthorized: AtomicU64::new(0),
            dependency_unavailable: AtomicU64::new(0),
            internal_error: AtomicU64::new(0),
        }
    }

    pub(crate) fn record<T>(&self, result: &AppResult<T>) {
        let outcome = classify_authentication_result(result);
        match outcome {
            AuthVerificationOutcome::Success => {
                self.success.fetch_add(1, Ordering::Relaxed);
            }
            AuthVerificationOutcome::Unauthorized => {
                self.unauthorized.fetch_add(1, Ordering::Relaxed);
            }
            AuthVerificationOutcome::DependencyUnavailable => {
                self.dependency_unavailable.fetch_add(1, Ordering::Relaxed);
            }
            AuthVerificationOutcome::InternalError => {
                self.internal_error.fetch_add(1, Ordering::Relaxed);
            }
        }

        let total = self.total.fetch_add(1, Ordering::Relaxed) + 1;
        if total.is_multiple_of(METRICS_LOG_EVERY_ATTEMPTS) {
            let stats = self.snapshot();
            log::info!(
                "auth verification aggregate: last_result={} total={} success={} unauthorized={} dependency_unavailable={} internal_error={}",
                outcome.label(),
                stats.total,
                stats.success,
                stats.unauthorized,
                stats.dependency_unavailable,
                stats.internal_error
            );
        }
    }

    pub(crate) fn record_and_return<T>(&self, result: AppResult<T>) -> AppResult<T> {
        self.record(&result);
        result
    }

    pub(crate) fn snapshot(&self) -> AuthVerificationStats {
        AuthVerificationStats {
            total: self.total.load(Ordering::Relaxed),
            success: self.success.load(Ordering::Relaxed),
            unauthorized: self.unauthorized.load(Ordering::Relaxed),
            dependency_unavailable: self.dependency_unavailable.load(Ordering::Relaxed),
            internal_error: self.internal_error.load(Ordering::Relaxed),
        }
    }
}

pub(crate) fn observe_authentication_result<T>(result: AppResult<T>) -> AppResult<T> {
    AUTH_OBSERVABILITY.record_and_return(result)
}

pub(crate) fn classify_authentication_result<T>(result: &AppResult<T>) -> AuthVerificationOutcome {
    match result {
        Ok(_) => AuthVerificationOutcome::Success,
        Err(AppError::Unauthorized(_)) => AuthVerificationOutcome::Unauthorized,
        Err(AppError::ServiceUnavailable(_)) => AuthVerificationOutcome::DependencyUnavailable,
        Err(_) => AuthVerificationOutcome::InternalError,
    }
}
