use async_trait::async_trait;

use crate::error::AppResult;

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

/// Permit held while one protected HIGH search query is in flight.
///
/// Protected reads participate in the same distributed maintenance barrier as
/// memo writers so projection reset/reindex cannot race a query that is already
/// reading the protected generation.
#[async_trait]
pub trait HighSearchQueryPermit: Send + Sync {
    async fn release(self: Box<Self>) -> AppResult<()>;
}

/// Application boundary that rejects new protected HIGH search queries while a
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
