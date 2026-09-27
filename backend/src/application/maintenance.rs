use async_trait::async_trait;

use crate::{
    application::{
        high_memo_routing::{HighMemoAuthoritativeRoute, HighMemoAuthoritativeRouteSnapshot},
        high_search_routing::{HighSearchQueryRoute, HighSearchQueryRouteSnapshot},
    },
    error::AppResult,
};

/// Permit held for the complete lifetime of one memo mutation.
///
/// Implementations may use this to participate in a distributed maintenance
/// barrier. The permit must not be released until the guarded mutation or
/// reconciliation work has completed. Foreground memo mutations and background
/// projection reconciliation intentionally share this boundary.
#[async_trait]
pub trait MemoMutationPermit: Send + Sync {
    /// Authoritative-store route captured atomically with writer-lease
    /// admission. Mutating request paths must use this snapshot rather than
    /// performing a separate route read.
    fn memo_route_snapshot(&self) -> HighMemoAuthoritativeRouteSnapshot;
    async fn release(self: Box<Self>) -> AppResult<()>;
}

/// Application boundary that rejects new memo mutations while a maintenance
/// window is active.
#[async_trait]
pub trait MemoMutationGuard: Send + Sync {
    async fn acquire_mutation(&self) -> AppResult<Box<dyn MemoMutationPermit>>;
}

/// Normal mode for deployments where HIGH search maintenance coordination is
/// not enabled.
#[derive(Default)]
pub struct UnrestrictedMemoMutationGuard;

struct UnrestrictedMemoMutationPermit;

#[async_trait]
impl MemoMutationPermit for UnrestrictedMemoMutationPermit {
    fn memo_route_snapshot(&self) -> HighMemoAuthoritativeRouteSnapshot {
        HighMemoAuthoritativeRouteSnapshot {
            route: HighMemoAuthoritativeRoute::Plaintext,
            generation: 0,
        }
    }

    async fn release(self: Box<Self>) -> AppResult<()> {
        Ok(())
    }
}

#[async_trait]
impl MemoMutationGuard for UnrestrictedMemoMutationGuard {
    async fn acquire_mutation(&self) -> AppResult<Box<dyn MemoMutationPermit>> {
        Ok(Box::new(UnrestrictedMemoMutationPermit))
    }
}

/// Permit held while one HIGH-search-routed query is in flight.
///
/// The permit carries the route snapshot read in the same admission transaction
/// that registers the query lease. User-visible routing uses this snapshot
/// rather than reading route state and acquiring a lease separately; this keeps
/// maintenance cutover atomic across replicas.
#[async_trait]
pub trait HighSearchQueryPermit: Send + Sync {
    fn route_snapshot(&self) -> HighSearchQueryRouteSnapshot;
    async fn release(self: Box<Self>) -> AppResult<()>;
}

/// Application boundary that rejects new HIGH-search-routed queries while a
/// maintenance window is active and tracks admitted queries until release.
#[async_trait]
pub trait HighSearchQueryGuard: Send + Sync {
    async fn acquire_query(&self) -> AppResult<Box<dyn HighSearchQueryPermit>>;
}

/// Normal mode used when HIGH search itself is disabled.
#[derive(Default)]
pub struct UnrestrictedHighSearchQueryGuard;

struct UnrestrictedHighSearchQueryPermit;

#[async_trait]
impl HighSearchQueryPermit for UnrestrictedHighSearchQueryPermit {
    fn route_snapshot(&self) -> HighSearchQueryRouteSnapshot {
        HighSearchQueryRouteSnapshot {
            route: HighSearchQueryRoute::Legacy,
            generation: 0,
        }
    }

    async fn release(self: Box<Self>) -> AppResult<()> {
        Ok(())
    }
}

#[async_trait]
impl HighSearchQueryGuard for UnrestrictedHighSearchQueryGuard {
    async fn acquire_query(&self) -> AppResult<Box<dyn HighSearchQueryPermit>> {
        Ok(Box::new(UnrestrictedHighSearchQueryPermit))
    }
}
