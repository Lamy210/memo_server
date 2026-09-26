use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use tokio::time::sleep;
use uuid::Uuid;

use crate::{
    application::{
        crypto_search_orchestration::HighSearchProjectionSink,
        maintenance::{MemoMutationGuard, MemoMutationPermit},
    },
    error::{AppError, AppResult},
    infrastructure::persistence::ports::{
        MemoAuthoritativeStore, MemoCache, MemoSearchProjection, ProjectionIntent, ProjectionTarget,
    },
};

const CACHE_TTL: Duration = Duration::from_secs(3600);
const POLL_INTERVAL: Duration = Duration::from_secs(2);
const STALE_INTENT_GRACE: Duration = Duration::from_secs(15 * 60);
const MAX_RETRY_BACKOFF: Duration = Duration::from_secs(5 * 60);
const METRICS_LOG_EVERY_PASSES: u64 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectionReconciliationStats {
    pub pending_intents: u64,
    pub reconciled_total: u64,
    pub retry_deferred_total: u64,
    pub stale_dropped_total: u64,
}

#[derive(Default)]
struct ReconciliationCounters {
    pending_intents: AtomicU64,
    reconciled_total: AtomicU64,
    retry_deferred_total: AtomicU64,
    stale_dropped_total: AtomicU64,
    passes: AtomicU64,
}

#[derive(Debug, Clone, Copy)]
struct RetryState {
    first_seen: Instant,
    attempt_count: u32,
    next_attempt_at: Instant,
}

impl RetryState {
    fn new(now: Instant) -> Self {
        Self {
            first_seen: now,
            attempt_count: 0,
            next_attempt_at: now,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReconcileOutcome {
    Completed,
    WaitingForTarget,
}

pub struct ProjectionReconciler {
    authoritative_store: Arc<dyn MemoAuthoritativeStore>,
    cache: Arc<dyn MemoCache>,
    search_projection: Arc<dyn MemoSearchProjection>,
    high_search_projection: Option<Arc<dyn HighSearchProjectionSink>>,
    mutation_guard: Arc<dyn MemoMutationGuard>,
    retry_states: Mutex<HashMap<Uuid, RetryState>>,
    counters: ReconciliationCounters,
}

impl ProjectionReconciler {
    pub fn new(
        authoritative_store: Arc<dyn MemoAuthoritativeStore>,
        cache: Arc<dyn MemoCache>,
        search_projection: Arc<dyn MemoSearchProjection>,
        high_search_projection: Option<Arc<dyn HighSearchProjectionSink>>,
        mutation_guard: Arc<dyn MemoMutationGuard>,
    ) -> Self {
        Self {
            authoritative_store,
            cache,
            search_projection,
            high_search_projection,
            mutation_guard,
            retry_states: Mutex::new(HashMap::new()),
            counters: ReconciliationCounters::default(),
        }
    }

    pub async fn reconcile_now(&self, event: &ProjectionIntent) {
        let now = Instant::now();
        self.track_event(event.event_id, now);
        match self.reconcile_event(event).await {
            Ok(ReconcileOutcome::Completed) => self.record_completed(event.event_id),
            Ok(ReconcileOutcome::WaitingForTarget) => {}
            Err(error) => {
                let retry_after = self.defer_after_failure(event.event_id, now);
                log::warn!(
                    "Projection reconciliation deferred: event_id={} memo_id={} user_id={} target={:?} retry_after_seconds={} error={error}",
                    event.event_id,
                    event.memo_id,
                    event.user_id,
                    event.target,
                    retry_after.as_secs()
                );
            }
        }
    }

    pub async fn run(self: Arc<Self>) {
        loop {
            if let Err(error) = self.run_once().await {
                log::warn!("Projection reconciliation pass failed: {error}");
            }
            self.emit_metrics_if_due();
            sleep(POLL_INTERVAL).await;
        }
    }

    async fn run_once(&self) -> AppResult<()> {
        let events = self.authoritative_store.list_projection_intents().await?;
        let pending_intents = events.len() as u64;
        let mut seen_event_ids = HashSet::new();

        for event in events {
            seen_event_ids.insert(event.event_id);
            let now = Instant::now();
            if !self.is_due(event.event_id, now) {
                continue;
            }

            match self.reconcile_event(&event).await {
                Ok(ReconcileOutcome::Completed) => self.record_completed(event.event_id),
                Ok(ReconcileOutcome::WaitingForTarget) => {
                    if self.is_stale_unreached(event.event_id, now) {
                        self.authoritative_store
                            .acknowledge_projection_intent(&event)
                            .await?;
                        self.clear_retry_state(event.event_id);
                        self.counters
                            .stale_dropped_total
                            .fetch_add(1, Ordering::Relaxed);
                        log::warn!(
                            "Dropped stale projection intent whose primary target was never reached: event_id={} memo_id={} user_id={} target={:?}",
                            event.event_id,
                            event.memo_id,
                            event.user_id,
                            event.target
                        );
                    }
                }
                Err(error) => {
                    let retry_after = self.defer_after_failure(event.event_id, now);
                    log::warn!(
                        "Projection reconciliation retry failed: event_id={} memo_id={} user_id={} target={:?} retry_after_seconds={} error={error}",
                        event.event_id,
                        event.memo_id,
                        event.user_id,
                        event.target,
                        retry_after.as_secs()
                    );
                }
            }
        }

        self.prune_retry_states(&seen_event_ids);
        self.counters
            .pending_intents
            .store(pending_intents, Ordering::Relaxed);
        Ok(())
    }

    async fn reconcile_event(&self, event: &ProjectionIntent) -> AppResult<ReconcileOutcome> {
        let memo = self
            .authoritative_store
            .find_by_id(event.user_id, event.memo_id)
            .await?;

        if !target_reached(event, memo.as_ref()) {
            return Ok(ReconcileOutcome::WaitingForTarget);
        }

        // Background reconciliation participates in the same distributed
        // maintenance barrier as foreground memo mutations. This prevents an
        // outbox retry from mutating either search projection while a staged
        // HIGH reindex/reset owns the offline window.
        let permit = self.mutation_guard.acquire_mutation().await?;
        let result = self.reconcile_secondary_state(event, memo.as_ref()).await;
        Self::finish_guarded_reconciliation(result, permit).await?;

        // Ack only after the secondary work and lease release both succeed.
        // A release failure therefore leaves the durable intent available for
        // an idempotent retry instead of silently losing reconciliation work.
        self.authoritative_store
            .acknowledge_projection_intent(event)
            .await?;
        Ok(ReconcileOutcome::Completed)
    }

    async fn reconcile_secondary_state(
        &self,
        event: &ProjectionIntent,
        memo: Option<&crate::domain::memo::entity::Memo>,
    ) -> AppResult<()> {
        let mut failures = Vec::new();
        let cache_key = cache_key(event.user_id, event.memo_id);

        match memo {
            Some(memo) => {
                if let Err(error) = self.search_projection.index_memo(memo).await {
                    failures.push(format!("search_projection={error}"));
                }
                if let Some(high_search_projection) = self.high_search_projection.as_ref() {
                    if let Err(error) = high_search_projection.replace_memo(memo).await {
                        failures.push(format!("high_search_projection={error}"));
                    }
                }
                if let Err(error) = self.cache.set_memo(&cache_key, memo, Some(CACHE_TTL)).await {
                    failures.push(format!("cache={error}"));
                }
            }
            None => {
                if let Err(error) = self.search_projection.delete_memo(event.memo_id).await {
                    failures.push(format!("search_projection={error}"));
                }
                if let Some(high_search_projection) = self.high_search_projection.as_ref() {
                    if let Err(error) = high_search_projection
                        .delete_memo(event.user_id, event.memo_id)
                        .await
                    {
                        failures.push(format!("high_search_projection={error}"));
                    }
                }
                if let Err(error) = self.cache.delete(&cache_key).await {
                    failures.push(format!("cache={error}"));
                }
            }
        }

        if !failures.is_empty() {
            return Err(AppError::DatabaseError(failures.join("; ")));
        }

        let current = self
            .authoritative_store
            .find_by_id(event.user_id, event.memo_id)
            .await?;
        if projection_state(memo) != projection_state(current.as_ref()) {
            let target = current
                .as_ref()
                .map(|memo| ProjectionTarget::Version(memo.version))
                .unwrap_or(ProjectionTarget::Deleted);
            self.authoritative_store
                .enqueue_projection_intent(event.user_id, event.memo_id, target)
                .await?;
        }

        Ok(())
    }

    async fn finish_guarded_reconciliation(
        result: AppResult<()>,
        permit: Box<dyn MemoMutationPermit>,
    ) -> AppResult<()> {
        match (result, permit.release().await) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(primary), Ok(())) => Err(primary),
            (Ok(()), Err(release)) => Err(release),
            (Err(primary), Err(release)) => Err(AppError::ServiceUnavailable(format!(
                "projection reconciliation failed and maintenance writer lease release also failed; primary={primary}; release={release}"
            ))),
        }
    }

    pub fn stats(&self) -> ProjectionReconciliationStats {
        ProjectionReconciliationStats {
            pending_intents: self.counters.pending_intents.load(Ordering::Relaxed),
            reconciled_total: self.counters.reconciled_total.load(Ordering::Relaxed),
            retry_deferred_total: self.counters.retry_deferred_total.load(Ordering::Relaxed),
            stale_dropped_total: self.counters.stale_dropped_total.load(Ordering::Relaxed),
        }
    }

    fn track_event(&self, event_id: Uuid, now: Instant) {
        let mut states = self.retry_states();
        states
            .entry(event_id)
            .or_insert_with(|| RetryState::new(now));
    }

    fn is_due(&self, event_id: Uuid, now: Instant) -> bool {
        let mut states = self.retry_states();
        let state = states
            .entry(event_id)
            .or_insert_with(|| RetryState::new(now));
        now >= state.next_attempt_at
    }

    fn is_stale_unreached(&self, event_id: Uuid, now: Instant) -> bool {
        let states = self.retry_states();
        states
            .get(&event_id)
            .is_some_and(|state| now.duration_since(state.first_seen) >= STALE_INTENT_GRACE)
    }

    fn defer_after_failure(&self, event_id: Uuid, now: Instant) -> Duration {
        let mut states = self.retry_states();
        let state = states
            .entry(event_id)
            .or_insert_with(|| RetryState::new(now));
        state.attempt_count = state.attempt_count.saturating_add(1);

        let delay = retry_delay(event_id, state.attempt_count);
        state.next_attempt_at = now + delay;
        self.counters
            .retry_deferred_total
            .fetch_add(1, Ordering::Relaxed);
        delay
    }

    fn record_completed(&self, event_id: Uuid) {
        self.clear_retry_state(event_id);
        self.counters
            .reconciled_total
            .fetch_add(1, Ordering::Relaxed);
    }

    fn clear_retry_state(&self, event_id: Uuid) {
        self.retry_states().remove(&event_id);
    }

    fn prune_retry_states(&self, seen_event_ids: &HashSet<Uuid>) {
        self.retry_states()
            .retain(|event_id, _| seen_event_ids.contains(event_id));
    }

    fn retry_states(&self) -> std::sync::MutexGuard<'_, HashMap<Uuid, RetryState>> {
        self.retry_states
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn emit_metrics_if_due(&self) {
        let pass = self.counters.passes.fetch_add(1, Ordering::Relaxed) + 1;
        if !pass.is_multiple_of(METRICS_LOG_EVERY_PASSES) {
            return;
        }

        let stats = self.stats();
        log::info!(
            "Projection reconciliation metrics: pending_intents={} reconciled_total={} retry_deferred_total={} stale_dropped_total={}",
            stats.pending_intents,
            stats.reconciled_total,
            stats.retry_deferred_total,
            stats.stale_dropped_total
        );
    }
}

fn retry_delay(event_id: Uuid, attempt_count: u32) -> Duration {
    let exponent = attempt_count.saturating_sub(1).min(8);
    let base_seconds = 2_u64
        .saturating_mul(1_u64 << exponent)
        .min(MAX_RETRY_BACKOFF.as_secs());
    let jitter_window = (base_seconds / 4).max(1);
    let jitter = (event_id.as_u128() as u64) % (jitter_window + 1);

    Duration::from_secs(
        base_seconds
            .saturating_add(jitter)
            .min(MAX_RETRY_BACKOFF.as_secs()),
    )
}

fn projection_state(memo: Option<&crate::domain::memo::entity::Memo>) -> Option<i32> {
    memo.map(|memo| memo.version)
}

fn target_reached(
    event: &ProjectionIntent,
    memo: Option<&crate::domain::memo::entity::Memo>,
) -> bool {
    match event.target {
        ProjectionTarget::Version(version) => memo.is_some_and(|memo| memo.version >= version),
        ProjectionTarget::Deleted => memo.is_none(),
    }
}

fn cache_key(user_id: Uuid, memo_id: Uuid) -> String {
    format!("memo:{user_id}:{memo_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct TestEvents(Mutex<Vec<&'static str>>);

    impl TestEvents {
        fn push(&self, event: &'static str) {
            self.0.lock().unwrap().push(event);
        }

        fn snapshot(&self) -> Vec<&'static str> {
            self.0.lock().unwrap().clone()
        }
    }

    struct FakeAuthoritativeStore {
        memo: Mutex<Option<crate::domain::memo::entity::Memo>>,
        acknowledged: AtomicU64,
        events: Arc<TestEvents>,
    }

    #[async_trait::async_trait]
    impl MemoAuthoritativeStore for FakeAuthoritativeStore {
        async fn find_by_id(
            &self,
            _user_id: Uuid,
            _id: Uuid,
        ) -> AppResult<Option<crate::domain::memo::entity::Memo>> {
            Ok(self.memo.lock().unwrap().clone())
        }

        async fn find_all_by_user_id(
            &self,
            _user_id: Uuid,
        ) -> AppResult<Vec<crate::domain::memo::entity::Memo>> {
            Ok(Vec::new())
        }

        async fn find_many_by_ids(
            &self,
            _user_id: Uuid,
            _ids: &[Uuid],
        ) -> AppResult<Vec<crate::domain::memo::entity::Memo>> {
            Ok(Vec::new())
        }

        async fn save_with_projection_intent(
            &self,
            memo: &crate::domain::memo::entity::Memo,
        ) -> AppResult<ProjectionIntent> {
            Ok(ProjectionIntent::new(
                memo.user_id,
                memo.id,
                ProjectionTarget::Version(memo.version),
            ))
        }

        async fn delete_with_projection_intent(
            &self,
            user_id: Uuid,
            id: Uuid,
        ) -> AppResult<ProjectionIntent> {
            Ok(ProjectionIntent::new(
                user_id,
                id,
                ProjectionTarget::Deleted,
            ))
        }

        async fn exists(&self, _user_id: Uuid, _id: Uuid) -> AppResult<bool> {
            Ok(self.memo.lock().unwrap().is_some())
        }

        async fn enqueue_projection_intent(
            &self,
            user_id: Uuid,
            memo_id: Uuid,
            target: ProjectionTarget,
        ) -> AppResult<ProjectionIntent> {
            Ok(ProjectionIntent::new(user_id, memo_id, target))
        }

        async fn list_projection_intents(&self) -> AppResult<Vec<ProjectionIntent>> {
            Ok(Vec::new())
        }

        async fn acknowledge_projection_intent(
            &self,
            _event: &ProjectionIntent,
        ) -> AppResult<()> {
            self.events.push("ack");
            self.acknowledged.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }

    struct FakeCache {
        events: Arc<TestEvents>,
    }

    #[async_trait::async_trait]
    impl MemoCache for FakeCache {
        async fn get_memo(
            &self,
            _key: &str,
        ) -> AppResult<Option<crate::domain::memo::entity::Memo>> {
            Ok(None)
        }

        async fn set_memo(
            &self,
            _key: &str,
            _memo: &crate::domain::memo::entity::Memo,
            _expiration: Option<Duration>,
        ) -> AppResult<()> {
            self.events.push("cache-set");
            Ok(())
        }

        async fn delete(&self, _key: &str) -> AppResult<()> {
            self.events.push("cache-delete");
            Ok(())
        }

        async fn exists(&self, _key: &str) -> AppResult<bool> {
            Ok(false)
        }
    }

    struct FakeLegacyProjection {
        events: Arc<TestEvents>,
    }

    #[async_trait::async_trait]
    impl MemoSearchProjection for FakeLegacyProjection {
        async fn index_memo(
            &self,
            _memo: &crate::domain::memo::entity::Memo,
        ) -> AppResult<()> {
            self.events.push("legacy-index");
            Ok(())
        }

        async fn search_memo_ids(
            &self,
            _query: &str,
            _tag: Option<String>,
            _user_id: Uuid,
            _page: usize,
            _limit: usize,
        ) -> AppResult<crate::infrastructure::persistence::ports::MemoSearchHitPage> {
            Ok(crate::infrastructure::persistence::ports::MemoSearchHitPage {
                memo_ids: Vec::new(),
                total: 0,
            })
        }

        async fn delete_memo(&self, _id: Uuid) -> AppResult<()> {
            self.events.push("legacy-delete");
            Ok(())
        }
    }

    struct FakeHighProjection {
        events: Arc<TestEvents>,
        fail_replace: bool,
        deleted: Mutex<Option<(Uuid, Uuid)>>,
    }

    #[async_trait::async_trait]
    impl HighSearchProjectionSink for FakeHighProjection {
        async fn replace_memo(
            &self,
            _memo: &crate::domain::memo::entity::Memo,
        ) -> AppResult<()> {
            self.events.push("high-index");
            if self.fail_replace {
                Err(AppError::DatabaseError("HIGH projection failed".into()))
            } else {
                Ok(())
            }
        }

        async fn delete_memo(&self, owner_partition: Uuid, memo_id: Uuid) -> AppResult<()> {
            self.events.push("high-delete");
            *self.deleted.lock().unwrap() = Some((owner_partition, memo_id));
            Ok(())
        }
    }

    struct FakeMutationGuard {
        events: Arc<TestEvents>,
        fail_release: bool,
    }

    struct FakeMutationPermit {
        events: Arc<TestEvents>,
        fail_release: bool,
    }

    #[async_trait::async_trait]
    impl MemoMutationGuard for FakeMutationGuard {
        async fn acquire_mutation(&self) -> AppResult<Box<dyn MemoMutationPermit>> {
            self.events.push("guard-acquire");
            Ok(Box::new(FakeMutationPermit {
                events: self.events.clone(),
                fail_release: self.fail_release,
            }))
        }
    }

    #[async_trait::async_trait]
    impl MemoMutationPermit for FakeMutationPermit {
        async fn release(self: Box<Self>) -> AppResult<()> {
            self.events.push("guard-release");
            if self.fail_release {
                Err(AppError::ServiceUnavailable(
                    "writer lease release failed".into(),
                ))
            } else {
                Ok(())
            }
        }
    }

    fn test_memo(user_id: Uuid, memo_id: Uuid) -> crate::domain::memo::entity::Memo {
        let mut memo = crate::domain::memo::entity::Memo::new(
            "title".into(),
            "content".into(),
            vec!["tag".into()],
            user_id,
        );
        memo.id = memo_id;
        memo
    }

    fn test_reconciler(
        memo: Option<crate::domain::memo::entity::Memo>,
        high_fail_replace: bool,
        guard_fail_release: bool,
    ) -> (
        ProjectionReconciler,
        Arc<FakeAuthoritativeStore>,
        Arc<FakeHighProjection>,
        Arc<TestEvents>,
    ) {
        let events = Arc::new(TestEvents::default());
        let store = Arc::new(FakeAuthoritativeStore {
            memo: Mutex::new(memo),
            acknowledged: AtomicU64::new(0),
            events: events.clone(),
        });
        let cache = Arc::new(FakeCache {
            events: events.clone(),
        });
        let legacy = Arc::new(FakeLegacyProjection {
            events: events.clone(),
        });
        let high = Arc::new(FakeHighProjection {
            events: events.clone(),
            fail_replace: high_fail_replace,
            deleted: Mutex::new(None),
        });
        let guard = Arc::new(FakeMutationGuard {
            events: events.clone(),
            fail_release: guard_fail_release,
        });

        (
            ProjectionReconciler::new(
                store.clone(),
                cache,
                legacy,
                Some(high.clone()),
                guard,
            ),
            store,
            high,
            events,
        )
    }

    #[tokio::test]
    async fn successful_high_mirror_releases_guard_before_ack() {
        let user_id = Uuid::new_v4();
        let memo_id = Uuid::new_v4();
        let memo = test_memo(user_id, memo_id);
        let event = ProjectionIntent::new(
            user_id,
            memo_id,
            ProjectionTarget::Version(memo.version),
        );
        let (reconciler, store, _, events) = test_reconciler(Some(memo), false, false);

        assert_eq!(
            reconciler.reconcile_event(&event).await.unwrap(),
            ReconcileOutcome::Completed
        );
        assert_eq!(store.acknowledged.load(Ordering::Relaxed), 1);
        assert_eq!(
            events.snapshot(),
            vec![
                "guard-acquire",
                "legacy-index",
                "high-index",
                "cache-set",
                "guard-release",
                "ack"
            ]
        );
    }

    #[tokio::test]
    async fn high_mirror_failure_keeps_outbox_intent_unacknowledged() {
        let user_id = Uuid::new_v4();
        let memo_id = Uuid::new_v4();
        let memo = test_memo(user_id, memo_id);
        let event = ProjectionIntent::new(
            user_id,
            memo_id,
            ProjectionTarget::Version(memo.version),
        );
        let (reconciler, store, _, events) = test_reconciler(Some(memo), true, false);

        assert!(reconciler.reconcile_event(&event).await.is_err());
        assert_eq!(store.acknowledged.load(Ordering::Relaxed), 0);
        assert_eq!(
            events.snapshot(),
            vec![
                "guard-acquire",
                "legacy-index",
                "high-index",
                "cache-set",
                "guard-release"
            ]
        );
    }

    #[tokio::test]
    async fn guard_release_failure_keeps_outbox_intent_unacknowledged() {
        let user_id = Uuid::new_v4();
        let memo_id = Uuid::new_v4();
        let memo = test_memo(user_id, memo_id);
        let event = ProjectionIntent::new(
            user_id,
            memo_id,
            ProjectionTarget::Version(memo.version),
        );
        let (reconciler, store, _, events) = test_reconciler(Some(memo), false, true);

        assert!(matches!(
            reconciler.reconcile_event(&event).await,
            Err(AppError::ServiceUnavailable(_))
        ));
        assert_eq!(store.acknowledged.load(Ordering::Relaxed), 0);
        assert_eq!(
            events.snapshot(),
            vec![
                "guard-acquire",
                "legacy-index",
                "high-index",
                "cache-set",
                "guard-release"
            ]
        );
    }

    #[tokio::test]
    async fn delete_mirror_preserves_owner_scope() {
        let user_id = Uuid::new_v4();
        let memo_id = Uuid::new_v4();
        let event = ProjectionIntent::new(user_id, memo_id, ProjectionTarget::Deleted);
        let (reconciler, store, high, events) = test_reconciler(None, false, false);

        assert_eq!(
            reconciler.reconcile_event(&event).await.unwrap(),
            ReconcileOutcome::Completed
        );
        assert_eq!(store.acknowledged.load(Ordering::Relaxed), 1);
        assert_eq!(*high.deleted.lock().unwrap(), Some((user_id, memo_id)));
        assert_eq!(
            events.snapshot(),
            vec![
                "guard-acquire",
                "legacy-delete",
                "high-delete",
                "cache-delete",
                "guard-release",
                "ack"
            ]
        );
    }

    fn retry(user_id: Uuid, memo_id: Uuid, target: ProjectionTarget) -> ProjectionIntent {
        ProjectionIntent::new(user_id, memo_id, target)
    }

    #[test]
    fn present_target_waits_until_scylla_reaches_expected_version() {
        let user_id = Uuid::new_v4();
        let memo_id = Uuid::new_v4();
        let event = retry(user_id, memo_id, ProjectionTarget::Version(2));
        let mut memo = crate::domain::memo::entity::Memo::new(
            "title".into(),
            "content".into(),
            vec![],
            user_id,
        );
        memo.id = memo_id;

        assert!(!target_reached(&event, None));
        assert!(!target_reached(&event, Some(&memo)));

        memo.version = 2;
        assert!(target_reached(&event, Some(&memo)));

        memo.version = 3;
        assert!(target_reached(&event, Some(&memo)));
    }

    #[test]
    fn delete_target_waits_until_scylla_row_is_absent() {
        let user_id = Uuid::new_v4();
        let memo_id = Uuid::new_v4();
        let event = retry(user_id, memo_id, ProjectionTarget::Deleted);
        let mut memo = crate::domain::memo::entity::Memo::new(
            "title".into(),
            "content".into(),
            vec![],
            user_id,
        );
        memo.id = memo_id;

        assert!(!target_reached(&event, Some(&memo)));
        assert!(target_reached(&event, None));
    }

    #[test]
    fn projection_state_changes_when_version_or_presence_changes() {
        let user_id = Uuid::new_v4();
        let mut memo = crate::domain::memo::entity::Memo::new(
            "title".into(),
            "content".into(),
            vec![],
            user_id,
        );

        assert_eq!(projection_state(None), None);
        assert_eq!(projection_state(Some(&memo)), Some(1));

        memo.version = 2;
        assert_eq!(projection_state(Some(&memo)), Some(2));
    }

    #[test]
    fn cache_key_is_tenant_scoped() {
        let user_id = Uuid::new_v4();
        let memo_id = Uuid::new_v4();

        assert_eq!(
            cache_key(user_id, memo_id),
            format!("memo:{user_id}:{memo_id}")
        );
    }

    #[test]
    fn retry_delay_grows_exponentially_and_caps() {
        let event_id = Uuid::nil();

        assert_eq!(retry_delay(event_id, 1), Duration::from_secs(2));
        assert_eq!(retry_delay(event_id, 2), Duration::from_secs(4));
        assert_eq!(retry_delay(event_id, 3), Duration::from_secs(8));
        assert_eq!(
            retry_delay(event_id, 9),
            Duration::from_secs(MAX_RETRY_BACKOFF.as_secs())
        );
    }

    #[test]
    fn retry_delay_is_deterministic_for_an_event() {
        let event_id = Uuid::new_v4();

        assert_eq!(retry_delay(event_id, 4), retry_delay(event_id, 4));
        assert!(retry_delay(event_id, 4) >= Duration::from_secs(16));
        assert!(retry_delay(event_id, 4) <= Duration::from_secs(20));
    }
}
