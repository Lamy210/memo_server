use std::fmt;

use async_trait::async_trait;

use crate::error::AppResult;

pub const HIGH_SEARCH_ROUTE_LEGACY: &str = "legacy";
pub const HIGH_SEARCH_ROUTE_PROTECTED: &str = "protected";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HighSearchQueryRoute {
    Legacy,
    Protected,
}

impl HighSearchQueryRoute {
    pub fn as_persisted_str(self) -> &'static str {
        match self {
            Self::Legacy => HIGH_SEARCH_ROUTE_LEGACY,
            Self::Protected => HIGH_SEARCH_ROUTE_PROTECTED,
        }
    }

    pub fn from_persisted_str(value: &str) -> Option<Self> {
        match value {
            HIGH_SEARCH_ROUTE_LEGACY => Some(Self::Legacy),
            HIGH_SEARCH_ROUTE_PROTECTED => Some(Self::Protected),
            _ => None,
        }
    }
}

impl fmt::Display for HighSearchQueryRoute {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_persisted_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HighSearchQueryRouteSnapshot {
    pub route: HighSearchQueryRoute,
    pub generation: i64,
}

/// Read-only route visibility for operator/status surfaces.
///
/// User-visible query handling must not make a route decision from this reader
/// and then acquire a lease separately. The atomic request-path boundary is the
/// route snapshot carried by `HighSearchQueryPermit`.
#[async_trait]
pub trait HighSearchQueryRouteReader: Send + Sync {
    async fn current_query_route(&self) -> AppResult<HighSearchQueryRouteSnapshot>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persisted_route_values_are_explicit_and_round_trip() {
        for route in [
            HighSearchQueryRoute::Legacy,
            HighSearchQueryRoute::Protected,
        ] {
            assert_eq!(
                HighSearchQueryRoute::from_persisted_str(route.as_persisted_str()),
                Some(route)
            );
        }
        assert_eq!(HighSearchQueryRoute::from_persisted_str("unknown"), None);
    }
}
