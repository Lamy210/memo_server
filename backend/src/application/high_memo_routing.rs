use std::fmt;

use async_trait::async_trait;

use crate::error::AppResult;

pub const HIGH_MEMO_ROUTE_PLAINTEXT: &str = "plaintext";
pub const HIGH_MEMO_ROUTE_ENCRYPTED: &str = "encrypted";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HighMemoAuthoritativeRoute {
    Plaintext,
    Encrypted,
}

impl HighMemoAuthoritativeRoute {
    pub fn as_persisted_str(self) -> &'static str {
        match self {
            Self::Plaintext => HIGH_MEMO_ROUTE_PLAINTEXT,
            Self::Encrypted => HIGH_MEMO_ROUTE_ENCRYPTED,
        }
    }

    pub fn from_persisted_str(value: &str) -> Option<Self> {
        match value {
            HIGH_MEMO_ROUTE_PLAINTEXT => Some(Self::Plaintext),
            HIGH_MEMO_ROUTE_ENCRYPTED => Some(Self::Encrypted),
            _ => None,
        }
    }
}

impl fmt::Display for HighMemoAuthoritativeRoute {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_persisted_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HighMemoAuthoritativeRouteSnapshot {
    pub route: HighMemoAuthoritativeRoute,
    pub generation: i64,
}

/// Read-only visibility for operator/status and non-mutating request routing.
///
/// Mutating request paths must route from the snapshot carried by
/// `MemoMutationPermit` so route selection and writer-lease admission share
/// one MongoDB transaction.
#[async_trait]
pub trait HighMemoAuthoritativeRouteReader: Send + Sync {
    async fn current_memo_route(&self) -> AppResult<HighMemoAuthoritativeRouteSnapshot>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persisted_routes_are_explicit_and_round_trip() {
        for route in [
            HighMemoAuthoritativeRoute::Plaintext,
            HighMemoAuthoritativeRoute::Encrypted,
        ] {
            assert_eq!(
                HighMemoAuthoritativeRoute::from_persisted_str(route.as_persisted_str()),
                Some(route)
            );
        }
        assert_eq!(
            HighMemoAuthoritativeRoute::from_persisted_str("unknown"),
            None
        );
    }
}
