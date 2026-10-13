use crate::error::{AppError, AppResult};

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

pub(crate) fn classify_authentication_result<T>(result: &AppResult<T>) -> AuthVerificationOutcome {
    match result {
        Ok(_) => AuthVerificationOutcome::Success,
        Err(AppError::Unauthorized(_)) => AuthVerificationOutcome::Unauthorized,
        Err(AppError::ServiceUnavailable(_)) => AuthVerificationOutcome::DependencyUnavailable,
        Err(_) => AuthVerificationOutcome::InternalError,
    }
}
