use async_trait::async_trait;

use crate::{
    application::{
        high_memo_routing::{HighMemoDataRoute, HighMemoDataRouteSnapshot},
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

/// Permit held while one memo request can observe or mutate authoritative memo data.
///
/// The permit carries the authoritative/cache route snapshot read atomically
/// with lease admission. Future MEMO-HIGH-1 request-path routing must hold this
/// permit across cache access, authoritative access, and any cache fill so a
/// maintenance-held route cutover can drain legacy plaintext activity first.
#[async_trait]
pub trait HighMemoAccessPermit: Send + Sync {
    fn route_snapshot(&self) -> HighMemoDataRouteSnapshot;
    async fn release(self: Box<Self>) -> AppResult<()>;
}

/// Application boundary for memo data-path admission.
#[async_trait]
pub trait HighMemoAccessGuard: Send + Sync {
    async fn acquire_access(&self) -> AppResult<Box<dyn HighMemoAccessPermit>>;
}

/// Normal mode before MEMO-HIGH-1 routing is enabled.
#[derive(Default)]
pub struct UnrestrictedHighMemoAccessGuard;

struct UnrestrictedHighMemoAccessPermit;

#[async_trait]
impl HighMemoAccessPermit for UnrestrictedHighMemoAccessPermit {
    fn route_snapshot(&self) -> HighMemoDataRouteSnapshot {
        HighMemoDataRouteSnapshot {
            route: HighMemoDataRoute::LegacyPlaintext,
            generation: 0,
        }
    }

    async fn release(self: Box<Self>) -> AppResult<()> {
        Ok(())
    }
}

#[async_trait]
impl HighMemoAccessGuard for UnrestrictedHighMemoAccessGuard {
    async fn acquire_access(&self) -> AppResult<Box<dyn HighMemoAccessPermit>> {
        Ok(Box::new(UnrestrictedHighMemoAccessPermit))
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
