use std::sync::Arc;

use async_trait::async_trait;

use crate::{
    application::{
        crypto_migration_batch::validate_page_size,
        crypto_search_reindex::{HighSearchReindexRunner, HighSearchReindexStats},
    },
    error::{AppError, AppResult},
};

/// Live permit for an enforced operator-controlled offline window.
///
/// A production implementation must hold the maintenance/write-freeze barrier
/// for the lifetime of this value. Rotation orchestration explicitly releases
/// it only after cutover or fail-closed cleanup has completed.
#[async_trait]
pub trait HighSearchOfflineWindowPermit: Send + Sync {
    async fn assert_still_enforced(&self) -> AppResult<()>;
    async fn release(self: Box<Self>) -> AppResult<()>;
}

/// Acquires an enforced maintenance/write-freeze window.
///
/// This is deliberately not a boolean assertion. The returned permit must keep
/// the underlying exclusion active while it is held so memo mutations and
/// protected-search queries cannot resume in the middle of rotation/reindex.
#[async_trait]
pub trait HighSearchOfflineWindowGuard: Send + Sync {
    async fn acquire_offline_window(&self) -> AppResult<Box<dyn HighSearchOfflineWindowPermit>>;
}

/// Minimal cache-control port needed by key rotation orchestration.
#[async_trait]
pub trait HighSearchKeyCacheControl: Send + Sync {
    async fn clear_cached_keys(&self) -> AppResult<()>;
}

/// Fail-closed orchestration for a HIGH search key-generation rotation.
///
/// The service does not change provider configuration itself. It assumes the
/// runtime has already been composed for the target key generation while
/// writes and protected-search queries are held offline.
#[must_use = "keep the HIGH search rotation permit alive until cutover is complete, then call finish_after_cutover or abort"]
pub struct HighSearchRotationReady {
    stats: HighSearchReindexStats,
    permit: Box<dyn HighSearchOfflineWindowPermit>,
    cache: Arc<dyn HighSearchKeyCacheControl>,
}

impl HighSearchRotationReady {
    pub fn stats(&self) -> HighSearchReindexStats {
        self.stats
    }

    /// Confirm that the offline-window lease survived the caller-owned cutover
    /// and only then allow this permit to be dropped.
    pub async fn finish_after_cutover(self) -> AppResult<HighSearchReindexStats> {
        let Self {
            stats,
            permit,
            cache,
        } = self;

        if let Err(primary) = permit.assert_still_enforced().await {
            let cleanup = cache.clear_cached_keys().await;
            let release = permit.release().await;
            return rotation_failure(
                "HIGH search cutover permit failed",
                primary,
                cleanup,
                release,
            );
        }

        permit.release().await?;
        Ok(stats)
    }

    /// Abort a prepared rotation while the offline permit is still held.
    pub async fn abort(self) -> AppResult<()> {
        let Self {
            permit, cache, ..
        } = self;
        let cleanup = cache.clear_cached_keys().await;
        let release = permit.release().await;

        match (cleanup, release) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(primary), Ok(())) => Err(primary),
            (Ok(()), Err(release)) => Err(release),
            (Err(primary), Err(release)) => Err(AppError::ServiceUnavailable(format!(
                "HIGH search abort cleanup failed and maintenance release also failed; primary={primary}; release={release}"
            ))),
        }
    }
}

pub struct HighSearchRotationService {
    guard: Arc<dyn HighSearchOfflineWindowGuard>,
    cache: Arc<dyn HighSearchKeyCacheControl>,
    reindex: Arc<dyn HighSearchReindexRunner>,
}

impl HighSearchRotationService {
    pub fn new(
        guard: Arc<dyn HighSearchOfflineWindowGuard>,
        cache: Arc<dyn HighSearchKeyCacheControl>,
        reindex: Arc<dyn HighSearchReindexRunner>,
    ) -> Self {
        Self {
            guard,
            cache,
            reindex,
        }
    }

    pub async fn rotate_and_reindex(&self, page_size: usize) -> AppResult<HighSearchRotationReady> {
        // Reject invalid operator input before touching key state.
        validate_page_size(page_size)?;
        let permit = self.guard.acquire_offline_window().await?;

        // Discard keys from the previous runtime generation before any new
        // projection work. The newly composed provider will repopulate only the
        // target generation.
        if let Err(error) = self.cache.clear_cached_keys().await {
            return self.fail_closed_after_error(error, permit).await;
        }

        let stats = match self.reindex.reindex_all(page_size).await {
            Ok(stats) => stats,
            Err(error) => return self.fail_closed_after_error(error, permit).await,
        };

        // The live permit remains owned by this scope for the complete reindex
        // window. Re-check its backing lease before success can be returned.
        if let Err(error) = permit.assert_still_enforced().await {
            return self.fail_closed_after_error(error, permit).await;
        }

        Ok(HighSearchRotationReady {
            stats,
            permit,
            cache: self.cache.clone(),
        })
    }

    async fn fail_closed_after_error<T>(
        &self,
        primary: AppError,
        permit: Box<dyn HighSearchOfflineWindowPermit>,
    ) -> AppResult<T> {
        let cleanup = self.cache.clear_cached_keys().await;
        let release = permit.release().await;
        rotation_failure("HIGH search rotation failed", primary, cleanup, release)
    }
}

fn rotation_failure<T>(
    context: &str,
    primary: AppError,
    cleanup: AppResult<()>,
    release: AppResult<()>,
) -> AppResult<T> {
    match (cleanup, release) {
        (Ok(()), Ok(())) => Err(primary),
        (Err(cleanup), Ok(())) => Err(AppError::ServiceUnavailable(format!(
            "{context} and cache cleanup also failed; primary={primary}; cleanup={cleanup}"
        ))),
        (Ok(()), Err(release)) => Err(AppError::ServiceUnavailable(format!(
            "{context} and maintenance release also failed; primary={primary}; release={release}"
        ))),
        (Err(cleanup), Err(release)) => Err(AppError::ServiceUnavailable(format!(
            "{context}, cache cleanup failed, and maintenance release failed; primary={primary}; cleanup={cleanup}; release={release}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    };

    use super::*;

    #[derive(Default)]
    struct Events(Mutex<Vec<&'static str>>);

    impl Events {
        fn push(&self, event: &'static str) {
            self.0.lock().unwrap().push(event);
        }

        fn snapshot(&self) -> Vec<&'static str> {
            self.0.lock().unwrap().clone()
        }
    }

    struct FakeGuard {
        acquire_calls: AtomicUsize,
        fail_acquire: bool,
        fail_permit_check: bool,
        events: Arc<Events>,
    }

    struct FakePermit {
        fail_check: bool,
        events: Arc<Events>,
    }

    #[async_trait]
    impl HighSearchOfflineWindowPermit for FakePermit {
        async fn assert_still_enforced(&self) -> AppResult<()> {
            self.events.push("permit-check");
            if self.fail_check {
                return Err(AppError::Conflict(
                    "HIGH search offline window lease was lost".into(),
                ));
            }
            Ok(())
        }

        async fn release(self: Box<Self>) -> AppResult<()> {
            self.events.push("permit-release");
            Ok(())
        }
    }

    #[async_trait]
    impl HighSearchOfflineWindowGuard for FakeGuard {
        async fn acquire_offline_window(
            &self,
        ) -> AppResult<Box<dyn HighSearchOfflineWindowPermit>> {
            self.acquire_calls.fetch_add(1, Ordering::Relaxed);
            self.events.push("guard-acquire");
            if self.fail_acquire {
                return Err(AppError::Conflict(
                    "HIGH search offline window could not be acquired".into(),
                ));
            }
            Ok(Box::new(FakePermit {
                fail_check: self.fail_permit_check,
                events: self.events.clone(),
            }))
        }
    }

    struct FakeCache {
        calls: AtomicUsize,
        fail_on_call: Option<usize>,
        events: Arc<Events>,
    }

    #[async_trait]
    impl HighSearchKeyCacheControl for FakeCache {
        async fn clear_cached_keys(&self) -> AppResult<()> {
            let call = self.calls.fetch_add(1, Ordering::Relaxed) + 1;
            self.events.push("cache");
            if self.fail_on_call == Some(call) {
                return Err(AppError::ServiceUnavailable(
                    "HIGH search cache clear failed".into(),
                ));
            }
            Ok(())
        }
    }

    struct FakeReindex {
        fail: bool,
        events: Arc<Events>,
    }

    #[async_trait]
    impl HighSearchReindexRunner for FakeReindex {
        async fn reindex_all(&self, _page_size: usize) -> AppResult<HighSearchReindexStats> {
            self.events.push("reindex");
            if self.fail {
                return Err(AppError::Conflict("HIGH search reindex failed".into()));
            }
            Ok(HighSearchReindexStats {
                source_count: 2,
                projection_count: 2,
                projected_visited: 2,
                verified_visited: 2,
            })
        }
    }

    fn service(
        guard_fail_acquire: bool,
        permit_fail_check: bool,
        cache_fail_on_call: Option<usize>,
        reindex_fail: bool,
    ) -> (HighSearchRotationService, Arc<Events>) {
        let events = Arc::new(Events::default());
        let guard = Arc::new(FakeGuard {
            acquire_calls: AtomicUsize::new(0),
            fail_acquire: guard_fail_acquire,
            fail_permit_check: permit_fail_check,
            events: events.clone(),
        });
        let cache = Arc::new(FakeCache {
            calls: AtomicUsize::new(0),
            fail_on_call: cache_fail_on_call,
            events: events.clone(),
        });
        let reindex = Arc::new(FakeReindex {
            fail: reindex_fail,
            events: events.clone(),
        });
        (
            HighSearchRotationService::new(guard, cache, reindex),
            events,
        )
    }

    #[tokio::test]
    async fn rotation_requires_offline_window_before_touching_keys() {
        let (service, events) = service(true, false, None, false);

        assert!(matches!(
            service.rotate_and_reindex(100).await,
            Err(AppError::Conflict(_))
        ));
        assert_eq!(events.snapshot(), vec!["guard-acquire"]);
    }

    #[tokio::test]
    async fn rotation_clears_cache_reindexes_and_rechecks_guard() {
        let (service, events) = service(false, false, None, false);

        let ready = service.rotate_and_reindex(100).await.unwrap();

        assert_eq!(ready.stats().source_count, 2);
        assert_eq!(
            events.snapshot(),
            vec!["guard-acquire", "cache", "reindex", "permit-check"]
        );

        let stats = ready.finish_after_cutover().await.unwrap();
        assert_eq!(stats.source_count, 2);
        assert_eq!(
            events.snapshot(),
            vec![
                "guard-acquire",
                "cache",
                "reindex",
                "permit-check",
                "permit-check",
                "permit-release"
            ]
        );
    }

    #[tokio::test]
    async fn reindex_failure_clears_cache_again_and_stays_failed() {
        let (service, events) = service(false, false, None, true);

        assert!(matches!(
            service.rotate_and_reindex(100).await,
            Err(AppError::Conflict(_))
        ));
        assert_eq!(
            events.snapshot(),
            vec!["guard-acquire", "cache", "reindex", "cache", "permit-release"]
        );
    }

    #[tokio::test]
    async fn final_guard_failure_clears_cache_again_and_stays_failed() {
        let (service, events) = service(false, true, None, false);

        assert!(matches!(
            service.rotate_and_reindex(100).await,
            Err(AppError::Conflict(_))
        ));
        assert_eq!(
            events.snapshot(),
            vec![
                "guard-acquire",
                "cache",
                "reindex",
                "permit-check",
                "cache",
                "permit-release"
            ]
        );
    }

    #[tokio::test]
    async fn cleanup_failure_is_promoted_to_service_unavailable() {
        let (service, events) = service(false, false, Some(2), true);

        assert!(matches!(
            service.rotate_and_reindex(100).await,
            Err(AppError::ServiceUnavailable(_))
        ));
        assert_eq!(
            events.snapshot(),
            vec!["guard-acquire", "cache", "reindex", "cache", "permit-release"]
        );
    }

    #[tokio::test]
    async fn prepared_rotation_abort_clears_cache_before_permit_drop() {
        let (service, events) = service(false, false, None, false);

        let ready = service.rotate_and_reindex(100).await.unwrap();
        ready.abort().await.unwrap();

        assert_eq!(
            events.snapshot(),
            vec![
                "guard-acquire",
                "cache",
                "reindex",
                "permit-check",
                "cache",
                "permit-release"
            ]
        );
    }

    #[tokio::test]
    async fn invalid_page_size_fails_before_guard_or_cache() {
        let (service, events) = service(false, false, None, false);

        assert!(matches!(
            service.rotate_and_reindex(0).await,
            Err(AppError::ValidationError(_))
        ));
        assert!(events.snapshot().is_empty());
    }
}
