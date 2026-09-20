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
    error::{AppError, AppResult},
    infrastructure::persistence::ports::{
        MemoAuthoritativeStore, MemoCache, MemoSearchProjection, ProjectionIntent,
        PROJECTION_DELETE_TARGET, PROJECTION_RETRY_BUCKETS,
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
    retry_states: Mutex<HashMap<Uuid, RetryState>>,
    counters: ReconciliationCounters,
}

impl ProjectionReconciler {
    pub fn new(
        authoritative_store: Arc<dyn MemoAuthoritativeStore>,
        cache: Arc<dyn MemoCache>,
        search_projection: Arc<dyn MemoSearchProjection>,
    ) -> Self {
        Self {
            authoritative_store,
            cache,
            search_projection,
            retry_states: Mutex::new(HashMap::new()),
            counters: ReconciliationCounters::default(),
        }
    }

    pub async fn prepare(
        &self,
        user_id: Uuid,
        memo_id: Uuid,
        target_version: i32,
    ) -> AppResult<ProjectionIntent> {
        let event = self
            .scylla
            .enqueue_projection_intent(user_id, memo_id, target_version)
            .await?;
        self.track_event(event.event_id, Instant::now());
        Ok(event)
    }

    pub async fn reconcile_now(&self, event: &ProjectionIntent) {
        let now = Instant::now();
        match self.reconcile_event(event).await {
            Ok(ReconcileOutcome::Completed) => self.record_completed(event.event_id),
            Ok(ReconcileOutcome::WaitingForTarget) => {}
            Err(error) => {
                let retry_after = self.defer_after_failure(event.event_id, now);
                log::warn!(
                    "Projection reconciliation deferred: event_id={} memo_id={} user_id={} target_version={} retry_after_seconds={} error={error}",
                    event.event_id,
                    event.memo_id,
                    event.user_id,
                    event.target_version,
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
        let mut pending_intents = 0_u64;
        let mut seen_event_ids = HashSet::new();

        for bucket in 0..PROJECTION_RETRY_BUCKETS {
            let events = self.authoritative_store.list_projection_intents(bucket).await?;
            pending_intents = pending_intents.saturating_add(events.len() as u64);

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
                            self.authoritative_store.acknowledge_projection_intent(&event).await?;
                            self.clear_retry_state(event.event_id);
                            self.counters
                                .stale_dropped_total
                                .fetch_add(1, Ordering::Relaxed);
                            log::warn!(
                                "Dropped stale projection intent whose primary target was never reached: event_id={} memo_id={} user_id={} target_version={}",
                                event.event_id,
                                event.memo_id,
                                event.user_id,
                                event.target_version
                            );
                        }
                    }
                    Err(error) => {
                        let retry_after = self.defer_after_failure(event.event_id, now);
                        log::warn!(
                            "Projection reconciliation retry failed: event_id={} memo_id={} user_id={} target_version={} retry_after_seconds={} error={error}",
                            event.event_id,
                            event.memo_id,
                            event.user_id,
                            event.target_version,
                            retry_after.as_secs()
                        );
                    }
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
        let memo = self.authoritative_store.find_by_id(event.user_id, event.memo_id).await?;

        if !target_reached(event, memo.as_ref()) {
            return Ok(ReconcileOutcome::WaitingForTarget);
        }

        let mut failures = Vec::new();
        let cache_key = cache_key(event.user_id, event.memo_id);

        match memo.as_ref() {
            Some(memo) => {
                if let Err(error) = self.search_projection.index_memo(memo).await {
                    failures.push(format!("search_projection={error}"));
                }
                if let Err(error) = self.cache.set_memo(&cache_key, memo, Some(CACHE_TTL)).await {
                    failures.push(format!("cache={error}"));
                }
            }
            None => {
                if let Err(error) = self.search_projection.delete_memo(event.memo_id).await {
                    failures.push(format!("search_projection={error}"));
                }
                if let Err(error) = self.cache.delete(&cache_key).await {
                    failures.push(format!("cache={error}"));
                }
            }
        }

        if !failures.is_empty() {
            return Err(AppError::DatabaseError(failures.join("; ")));
        }

        let current = self.authoritative_store.find_by_id(event.user_id, event.memo_id).await?;
        if projection_state(memo.as_ref()) != projection_state(current.as_ref()) {
            let target_version = current
                .as_ref()
                .map(|memo| memo.version)
                .unwrap_or(PROJECTION_DELETE_TARGET);
            self.authoritative_store
                .enqueue_projection_intent(event.user_id, event.memo_id, target_version)
                .await?;
        }

        self.authoritative_store.acknowledge_projection_intent(event).await?;
        Ok(ReconcileOutcome::Completed)
    }

    pub async fn cancel(&self, event: &ProjectionIntent) {
        match self.authoritative_store.acknowledge_projection_intent(event).await {
            Ok(()) => self.clear_retry_state(event.event_id),
            Err(error) => {
                log::warn!(
                    "Failed to cancel unused projection intent: event_id={} memo_id={} user_id={} error={error}",
                    event.event_id,
                    event.memo_id,
                    event.user_id
                );
            }
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
    if event.target_version == PROJECTION_DELETE_TARGET {
        return memo.is_none();
    }

    memo.is_some_and(|memo| memo.version >= event.target_version)
}

fn cache_key(user_id: Uuid, memo_id: Uuid) -> String {
    format!("memo:{user_id}:{memo_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn retry(user_id: Uuid, memo_id: Uuid, target_version: i32) -> ProjectionIntent {
        ProjectionIntent::new(user_id, memo_id, target_version)
    }

    #[test]
    fn present_target_waits_until_scylla_reaches_expected_version() {
        let user_id = Uuid::new_v4();
        let memo_id = Uuid::new_v4();
        let event = retry(user_id, memo_id, 2);
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
        let event = retry(user_id, memo_id, PROJECTION_DELETE_TARGET);
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
