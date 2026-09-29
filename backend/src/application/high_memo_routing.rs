use std::fmt;

use async_trait::async_trait;

use crate::error::AppResult;

pub const HIGH_MEMO_ROUTE_LEGACY: &str = "legacy_plaintext";
pub const HIGH_MEMO_ROUTE_ENCRYPTED: &str = "encrypted";

pub const HIGH_MEMO_PLAINTEXT_RETIREMENT_AVAILABLE: &str = "available";
pub const HIGH_MEMO_PLAINTEXT_RETIREMENT_IN_PROGRESS: &str = "in_progress";
pub const HIGH_MEMO_PLAINTEXT_RETIREMENT_RETIRED: &str = "retired";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HighMemoPlaintextRetirementState {
    Available,
    InProgress,
    Retired,
}

impl HighMemoPlaintextRetirementState {
    pub fn as_persisted_str(self) -> &'static str {
        match self {
            Self::Available => HIGH_MEMO_PLAINTEXT_RETIREMENT_AVAILABLE,
            Self::InProgress => HIGH_MEMO_PLAINTEXT_RETIREMENT_IN_PROGRESS,
            Self::Retired => HIGH_MEMO_PLAINTEXT_RETIREMENT_RETIRED,
        }
    }

    pub fn from_persisted_str(value: &str) -> Option<Self> {
        match value {
            HIGH_MEMO_PLAINTEXT_RETIREMENT_AVAILABLE => Some(Self::Available),
            HIGH_MEMO_PLAINTEXT_RETIREMENT_IN_PROGRESS => Some(Self::InProgress),
            HIGH_MEMO_PLAINTEXT_RETIREMENT_RETIRED => Some(Self::Retired),
            _ => None,
        }
    }

    pub fn legacy_rollback_allowed(self) -> bool {
        matches!(self, Self::Available)
    }
}

impl fmt::Display for HighMemoPlaintextRetirementState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_persisted_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HighMemoDataRoute {
    LegacyPlaintext,
    Encrypted,
}

impl HighMemoDataRoute {
    pub fn as_persisted_str(self) -> &'static str {
        match self {
            Self::LegacyPlaintext => HIGH_MEMO_ROUTE_LEGACY,
            Self::Encrypted => HIGH_MEMO_ROUTE_ENCRYPTED,
        }
    }

    pub fn from_persisted_str(value: &str) -> Option<Self> {
        match value {
            HIGH_MEMO_ROUTE_LEGACY => Some(Self::LegacyPlaintext),
            HIGH_MEMO_ROUTE_ENCRYPTED => Some(Self::Encrypted),
            _ => None,
        }
    }
}

impl fmt::Display for HighMemoDataRoute {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_persisted_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HighMemoDataRouteSnapshot {
    pub route: HighMemoDataRoute,
    pub generation: i64,
}

/// Read-only visibility for operator/status surfaces.
///
/// Request-path routing must use the snapshot carried by a
/// `HighMemoAccessPermit`; reading state and acquiring a lease separately would
/// reintroduce a multi-replica cutover race.
#[async_trait]
pub trait HighMemoDataRouteReader: Send + Sync {
    async fn current_memo_data_route(&self) -> AppResult<HighMemoDataRouteSnapshot>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plaintext_retirement_values_are_monotonic_and_explicit() {
        for state in [
            HighMemoPlaintextRetirementState::Available,
            HighMemoPlaintextRetirementState::InProgress,
            HighMemoPlaintextRetirementState::Retired,
        ] {
            assert_eq!(
                HighMemoPlaintextRetirementState::from_persisted_str(state.as_persisted_str()),
                Some(state)
            );
        }

        assert!(HighMemoPlaintextRetirementState::Available.legacy_rollback_allowed());
        assert!(!HighMemoPlaintextRetirementState::InProgress.legacy_rollback_allowed());
        assert!(!HighMemoPlaintextRetirementState::Retired.legacy_rollback_allowed());
        assert_eq!(
            HighMemoPlaintextRetirementState::from_persisted_str("unknown"),
            None
        );
    }

    #[test]
    fn persisted_values_are_explicit_and_round_trip() {
        for route in [
            HighMemoDataRoute::LegacyPlaintext,
            HighMemoDataRoute::Encrypted,
        ] {
            assert_eq!(
                HighMemoDataRoute::from_persisted_str(route.as_persisted_str()),
                Some(route)
            );
        }
        assert_eq!(HighMemoDataRoute::from_persisted_str("legacy"), None);
        assert_eq!(HighMemoDataRoute::from_persisted_str("unknown"), None);
    }
}
