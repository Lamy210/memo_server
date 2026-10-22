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
        high_memo_routing::HighMemoDataRoute,
        high_search_routing::{HighSearchQueryRoute, HighSearchQueryRouteReader},
        maintenance::{
            HighMemoAccessGuard, HighMemoAccessPermit, MemoMutationGuard, MemoMutationPermit,
        },
    },
    error::{AppError, AppResult},
    infrastructure::persistence::ports::{
        MemoAuthoritativeStore, MemoCache, MemoSearchProjection, ProjectionIntent, ProjectionTarget,
    },
};

const CACHE_TTL: Duration = Duration::from_secs(3600);
const INTENT_PAGE_SIZE: usize = 128;
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
    InactiveMemoRoute,
}

pub struct ProjectionReconciler {
    authoritative_store: Arc<dyn MemoAuthoritativeStore>,
    cache: Arc<dyn MemoCache>,
    search_projection: Arc<dyn MemoSearchProjection>,
    high_search_projection: Option<Arc<dyn HighSearchProjectionSink>>,
    mutation_guard: Arc<dyn MemoMutationGuard>,
    memo_route_scope: Option<(Arc<dyn HighMemoAccessGuard>, HighMemoDataRoute)>,
    search_route_reader: Option<Arc<dyn HighSearchQueryRouteReader>>,
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
            memo_route_scope: None,
            search_route_reader: None,
            retry_states: Mutex::new(HashMap::new()),
            counters: ReconciliationCounters::default(),
        }
    }

    /// Scope this reconciler to exactly one admitted MEMO-HIGH-1 data route.
    ///
    /// The access lease is acquired before reading the authoritative source and
    /// remains held through all secondary writes. If the shared route does not
    /// match, the durable intent is left untouched for the route that owns it.
    pub fn with_memo_route(
        mut self,
        access_guard: Arc<dyn HighMemoAccessGuard>,
        required_route: HighMemoDataRoute,
    ) -> Self {
        self.memo_route_scope = Some((access_guard, required_route));
        self
    }

    /// Select which search projection is authoritative for reconciliation.
    ///
    /// The reader is consulted only after the reconciler has acquired the
    /// maintenance writer lease. Search-route cutover also requires that writer
    /// lease set to drain, so the observed route cannot change until this
    /// reconciliation releases its permit.
    pub fn with_search_route(mut self, route_reader: Arc<dyn HighSearchQueryRouteReader>) -> Self {
        self.search_route_reader = Some(route_reader);
        self
    }

    pub async fn reconcile_now(&self, event: &ProjectionIntent) {
        let now = Instant::now();
        self.track_event(event.event_id, now);
        match self.reconcile_event(event).await {
            Ok(ReconcileOutcome::Completed) => self.record_completed(event.event_id),
            Ok(ReconcileOutcome::WaitingForTarget | ReconcileOutcome::InactiveMemoRoute) => {}
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
        let mut pending_intents = 0_u64;
        let mut seen_event_ids = HashSet::new();
        let mut after = None;

        loop {
            // Keep each authoritative-store fetch bounded. A complete pass
            // scans all cursor pages, so events that are deferred or owned by
            // another route cannot starve later entries in the outbox.
            let events = self
                .authoritative_store
                .list_projection_intents_page(after, INTENT_PAGE_SIZE)
                .await?;
            if events.is_empty() {
                break;
            }
            let page_len = events.len();
            after = events.last().map(|event| event.event_id);
            pending_intents = pending_intents.saturating_add(page_len as u64);

            for event in events {
            seen_event_ids.insert(event.event_id);
            let now = Instant::now();
            if !self.is_due(event.event_id, now) {
                continue;
            }

            match self.reconcile_event(&event).await {
                Ok(ReconcileOutcome::Completed) => {
                    self.record_completed(event.event_id);
                    pending_intents = pending_intents.saturating_sub(1);
                }
                Ok(ReconcileOutcome::WaitingForTarget) => {
                    if self.is_stale_unreached(event.event_id, now) {
                        // The authoritative mutation may have become visible since
                        // the first read. Re-run reconciliation before retiring an
                        // abandoned target, so a newly reachable event is projected
                        // rather than acknowledged without touching secondaries.
                        match self.reconcile_event(&event).await {
                            Ok(ReconcileOutcome::Completed) => {
                                self.record_completed(event.event_id);
                                pending_intents = pending_intents.saturating_sub(1);
                            }
                            Ok(ReconcileOutcome::InactiveMemoRoute) => {
                                // Cutover owns the outbox; do not stale-drop it.
                            }
                            Ok(ReconcileOutcome::WaitingForTarget) => {
                                match self
                                    .authoritative_store
                                    .acknowledge_projection_intent(&event)
                                    .await
                                {
                                    Ok(()) => {
                                        self.clear_retry_state(event.event_id);
                                        self.counters
                                            .stale_dropped_total
                                            .fetch_add(1, Ordering::Relaxed);
                                        pending_intents = pending_intents.saturating_sub(1);
                                        log::warn!(
                                            "Dropped stale projection intent whose primary target was never reached: event_id={} memo_id={} user_id={} target={:?}",
                                            event.event_id,
                                            event.memo_id,
                                            event.user_id,
                                            event.target
                                        );
                                    }
                                    Err(error) => {
                                        let retry_after =
                                            self.defer_after_failure(event.event_id, now);
                                        log::warn!(
                                            "Stale projection intent acknowledgement deferred: event_id={} memo_id={} user_id={} target={:?} retry_after_seconds={} error={error}",
                                            event.event_id,
                                            event.memo_id,
                                            event.user_id,
                                            event.target,
                                            retry_after.as_secs()
                                        );
                                    }
                                }
                            }
                            Err(error) => {
                                // A transient read, projection, or ACK failure
                                // during recheck must keep the durable intent.
                                let retry_after = self.defer_after_failure(event.event_id, now);
                                log::warn!(
                                    "Stale projection intent recheck deferred: event_id={} memo_id={} user_id={} target={:?} retry_after_seconds={} error={error}",
                                    event.event_id,
                                    event.memo_id,
                                    event.user_id,
                                    event.target,
                                    retry_after.as_secs()
                                );
                            }
                        }
                    }
                }
                Ok(ReconcileOutcome::InactiveMemoRoute) => {
                    // This outbox belongs to the other authoritative generation.
                    // Do not acknowledge, stale-drop, or touch secondary state.
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

            if page_len < INTENT_PAGE_SIZE {
                break;
            }
        }

        self.prune_retry_states(&seen_event_ids);
        self.counters
            .pending_intents
            .store(pending_intents, Ordering::Relaxed);
        Ok(())
    }

    async fn reconcile_event(&self, event: &ProjectionIntent) -> AppResult<ReconcileOutcome> {
        let access = match self.acquire_memo_route_access().await? {
            Some((permit, required_route)) if permit.route_snapshot().route != required_route => {
                permit.release().await?;
                return Ok(ReconcileOutcome::InactiveMemoRoute);
            }
            access => access,
        };

        let result = self.reconcile_event_before_ack(event).await;
        let outcome = Self::finish_memo_route_access(result, access).await?;
        if outcome != ReconcileOutcome::Completed {
            return Ok(outcome);
        }

        // Ack only after all secondary work and both maintenance/access leases
        // have been released. Any release failure leaves the durable intent for
        // an idempotent retry rather than silently losing reconciliation work.
        self.authoritative_store
            .acknowledge_projection_intent(event)
            .await?;
        Ok(ReconcileOutcome::Completed)
    }

    async fn reconcile_event_before_ack(
        &self,
        event: &ProjectionIntent,
    ) -> AppResult<ReconcileOutcome> {
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
        let result = async {
            let search_route = self.current_search_route().await?;
            self.reconcile_secondary_state(event, memo.as_ref(), search_route)
                .await
        }
        .await;
        Self::finish_guarded_reconciliation(result, permit).await?;

        Ok(ReconcileOutcome::Completed)
    }

    async fn acquire_memo_route_access(
        &self,
    ) -> AppResult<Option<(Box<dyn HighMemoAccessPermit>, HighMemoDataRoute)>> {
        let Some((guard, required_route)) = self.memo_route_scope.as_ref() else {
            return Ok(None);
        };
        let permit = guard.acquire_access().await?;
        Ok(Some((permit, *required_route)))
    }

    async fn finish_memo_route_access<T>(
        result: AppResult<T>,
        access: Option<(Box<dyn HighMemoAccessPermit>, HighMemoDataRoute)>,
    ) -> AppResult<T> {
        let Some((permit, _)) = access else {
            return result;
        };

        match (result, permit.release().await) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(primary), Ok(())) => Err(primary),
            (Ok(_), Err(release)) => Err(release),
            (Err(primary), Err(release)) => Err(AppError::ServiceUnavailable(format!(
                "projection reconciliation failed and memo-access lease release also failed; primary={primary}; release={release}"
            ))),
        }
    }

    async fn current_search_route(&self) -> AppResult<HighSearchQueryRoute> {
        match self.search_route_reader.as_ref() {
            Some(reader) => Ok(reader.current_query_route().await?.route),
            None => Ok(HighSearchQueryRoute::Legacy),
        }
    }

    async fn reconcile_secondary_state(
        &self,
        event: &ProjectionIntent,
        memo: Option<&crate::domain::memo::entity::Memo>,
        search_route: HighSearchQueryRoute,
    ) -> AppResult<()> {
        if search_route == HighSearchQueryRoute::Protected && self.high_search_projection.is_none()
        {
            return Err(AppError::ServiceUnavailable(
                "protected HIGH search route is active but no protected projection sink is available"
                    .into(),
            ));
        }

        let mut failures = Vec::new();

        match memo {
            Some(memo) => {
                if search_route == HighSearchQueryRoute::Legacy {
                    if let Err(error) = self.search_projection.index_memo(memo).await {
                        failures.push(format!("search_projection={error}"));
                    }
                }
                if let Some(high_search_projection) = self.high_search_projection.as_ref() {
                    if let Err(error) = high_search_projection.replace_memo(memo).await {
                        failures.push(format!("high_search_projection={error}"));
                    }
                }
                if let Err(error) = self.cache.delete_memo(memo.user_id, memo.id).await {
                    failures.push(format!("cache_invalidate={error}"));
                } else if let Err(error) = self.cache.set_memo(memo, Some(CACHE_TTL)).await {
                    failures.push(format!("cache_replace={error}"));
                }
            }
            None => {
                if search_route == HighSearchQueryRoute::Legacy {
                    if let Err(error) = self.search_projection.delete_memo(event.memo_id).await {
                        failures.push(format!("search_projection={error}"));
                    }
                }
                if let Some(high_search_projection) = self.high_search_projection.as_ref() {
                    if let Err(error) = high_search_projection
                        .delete_memo(event.user_id, event.memo_id)
                        .await
                    {
                        failures.push(format!("high_search_projection={error}"));
                    }
                }
                if let Err(error) = self.cache.delete_memo(event.user_id, event.memo_id).await {
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
        if projection_changed(memo, current.as_ref()) {
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

/// Detect any change to the authoritative memo while secondary writes were in flight.
/// Version changes are the usual path, but an import/repair or a same-version
/// replacement must not leave a stale cache or search projection indefinitely.
fn projection_changed(
    before: Option<&crate::domain::memo::entity::Memo>,
    after: Option<&crate::domain::memo::entity::Memo>,
) -> bool {
    match (before, after) {
        (None, None) => false,
        (Some(before), Some(after)) => {
            before.id != after.id
                || before.user_id != after.user_id
                || before.version != after.version
                || before.title != after.title
                || before.content != after.content
                || before.tags != after.tags
                || before.created_at != after.created_at
                || before.updated_at != after.updated_at
        }
        _ => true,
    }
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
        after_first_read: Mutex<Option<crate::domain::memo::entity::Memo>>,
        pending: Mutex<Vec<ProjectionIntent>>,
        fail_ack_for: Mutex<Option<Uuid>>,
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
            let snapshot = self.memo.lock().unwrap().clone();
            if let Some(replacement) = self.after_first_read.lock().unwrap().take() {
                *self.memo.lock().unwrap() = Some(replacement);
            }
            Ok(snapshot)
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
            self.events.push("enqueue");
            Ok(ProjectionIntent::new(user_id, memo_id, target))
        }

        async fn list_projection_intents(&self) -> AppResult<Vec<ProjectionIntent>> {
            Ok(self.pending.lock().unwrap().clone())
        }

        async fn acknowledge_projection_intent(&self, event: &ProjectionIntent) -> AppResult<()> {
            self.events.push("ack");
            if self.fail_ack_for.lock().unwrap().as_ref() == Some(&event.event_id) {
                return Err(AppError::DatabaseError(
                    "injected acknowledgement outage".into(),
                ));
            }
            self.pending
                .lock()
                .unwrap()
                .retain(|pending| pending.event_id != event.event_id);
            self.acknowledged.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }

    struct FakeCache {
        events: Arc<TestEvents>,
        fail_set: bool,
    }

    #[async_trait::async_trait]
    impl MemoCache for FakeCache {
        async fn get_memo(
            &self,
            _owner_partition: Uuid,
            _memo_id: Uuid,
        ) -> AppResult<Option<crate::domain::memo::entity::Memo>> {
            Ok(None)
        }

        async fn set_memo(
            &self,
            _memo: &crate::domain::memo::entity::Memo,
            _expiration: Option<Duration>,
        ) -> AppResult<()> {
            self.events.push("cache-set");
            if self.fail_set {
                Err(AppError::DatabaseError("cache set failed".into()))
            } else {
                Ok(())
            }
        }

        async fn delete_memo(&self, _owner_partition: Uuid, _memo_id: Uuid) -> AppResult<()> {
            self.events.push("cache-delete");
            Ok(())
        }

        async fn memo_exists(&self, _owner_partition: Uuid, _memo_id: Uuid) -> AppResult<bool> {
            Ok(false)
        }
    }

    struct FakeLegacyProjection {
        events: Arc<TestEvents>,
    }

    #[async_trait::async_trait]
    impl MemoSearchProjection for FakeLegacyProjection {
        async fn index_memo(&self, _memo: &crate::domain::memo::entity::Memo) -> AppResult<()> {
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
            Ok(
                crate::infrastructure::persistence::ports::MemoSearchHitPage {
                    memo_ids: Vec::new(),
                    total: 0,
                },
            )
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
        async fn replace_memo(&self, _memo: &crate::domain::memo::entity::Memo) -> AppResult<()> {
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

    struct FakeSearchRouteReader {
        route: HighSearchQueryRoute,
        fail: bool,
        events: Arc<TestEvents>,
    }

    #[async_trait::async_trait]
    impl HighSearchQueryRouteReader for FakeSearchRouteReader {
        async fn current_query_route(
            &self,
        ) -> AppResult<crate::application::high_search_routing::HighSearchQueryRouteSnapshot>
        {
            self.events.push("search-route-read");
            if self.fail {
                Err(AppError::ServiceUnavailable(
                    "search route read failed".into(),
                ))
            } else {
                Ok(
                    crate::application::high_search_routing::HighSearchQueryRouteSnapshot {
                        route: self.route,
                        generation: 5,
                    },
                )
            }
        }
    }

    struct FakeMemoAccessGuard {
        route: HighMemoDataRoute,
        fail_release: bool,
        events: Arc<TestEvents>,
    }

    struct FakeMemoAccessPermit {
        route: HighMemoDataRoute,
        fail_release: bool,
        events: Arc<TestEvents>,
    }

    #[async_trait::async_trait]
    impl HighMemoAccessGuard for FakeMemoAccessGuard {
        async fn acquire_access(&self) -> AppResult<Box<dyn HighMemoAccessPermit>> {
            self.events.push("memo-access-acquire");
            Ok(Box::new(FakeMemoAccessPermit {
                route: self.route,
                fail_release: self.fail_release,
                events: self.events.clone(),
            }))
        }
    }

    #[async_trait::async_trait]
    impl HighMemoAccessPermit for FakeMemoAccessPermit {
        fn route_snapshot(
            &self,
        ) -> crate::application::high_memo_routing::HighMemoDataRouteSnapshot {
            crate::application::high_memo_routing::HighMemoDataRouteSnapshot {
                route: self.route,
                generation: 4,
            }
        }

        async fn release(self: Box<Self>) -> AppResult<()> {
            self.events.push("memo-access-release");
            if self.fail_release {
                Err(AppError::ServiceUnavailable(
                    "memo access lease release failed".into(),
                ))
            } else {
                Ok(())
            }
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
            after_first_read: Mutex::new(None),
            pending: Mutex::new(Vec::new()),
            fail_ack_for: Mutex::new(None),
            acknowledged: AtomicU64::new(0),
            events: events.clone(),
        });
        let cache = Arc::new(FakeCache {
            events: events.clone(),
            fail_set: false,
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
            ProjectionReconciler::new(store.clone(), cache, legacy, Some(high.clone()), guard),
            store,
            high,
            events,
        )
    }

    #[tokio::test]
    async fn outbox_scan_reconciles_every_event_across_multiple_pages() {
        let owner = Uuid::new_v4();
        let memo_id = Uuid::new_v4();
        let memo = test_memo(owner, memo_id);
        let (reconciler, store, _, events) = test_reconciler(Some(memo.clone()), false, false);
        let pending = (0..INTENT_PAGE_SIZE + 3)
            .map(|_| ProjectionIntent::new(owner, memo_id, ProjectionTarget::Version(memo.version)))
            .collect::<Vec<_>>();
        *store.pending.lock().unwrap() = pending;

        reconciler.run_once().await.unwrap();

        assert!(store.pending.lock().unwrap().is_empty());
        assert_eq!(
            store.acknowledged.load(Ordering::Relaxed),
            (INTENT_PAGE_SIZE + 3) as u64
        );
        assert_eq!(reconciler.stats().pending_intents, 0);
        assert_eq!(
            reconciler.stats().reconciled_total,
            (INTENT_PAGE_SIZE + 3) as u64
        );
        assert_eq!(
            events.snapshot().iter().filter(|event| **event == "ack").count(),
            INTENT_PAGE_SIZE + 3
        );
    }

    #[tokio::test]
    async fn stale_intent_becoming_reachable_is_projected_before_acknowledgement() {
        let owner = Uuid::new_v4();
        let memo_id = Uuid::new_v4();
        let previous = test_memo(owner, memo_id);
        let mut committed = previous.clone();
        committed.version += 1;
        committed.content = "now visible in authoritative store".into();
        let event =
            ProjectionIntent::new(owner, memo_id, ProjectionTarget::Version(committed.version));
        let (reconciler, store, _, events) = test_reconciler(Some(previous), false, false);
        *store.pending.lock().unwrap() = vec![event.clone()];
        // The initial read sees the earlier version; the recheck sees the
        // committed target. The old implementation discarded this event.
        *store.after_first_read.lock().unwrap() = Some(committed);
        let now = Instant::now();
        reconciler.retry_states().insert(
            event.event_id,
            RetryState {
                first_seen: now - STALE_INTENT_GRACE - Duration::from_secs(1),
                attempt_count: 0,
                next_attempt_at: now,
            },
        );

        reconciler.run_once().await.unwrap();

        assert!(store.pending.lock().unwrap().is_empty());
        assert_eq!(store.acknowledged.load(Ordering::Relaxed), 1);
        assert_eq!(reconciler.stats().reconciled_total, 1);
        assert_eq!(reconciler.stats().stale_dropped_total, 0);
        assert_eq!(reconciler.stats().pending_intents, 0);
        assert_eq!(
            events.snapshot(),
            vec![
                "guard-acquire",
                "legacy-index",
                "high-index",
                "cache-delete",
                "cache-set",
                "guard-release",
                "ack"
            ]
        );
    }

    #[tokio::test]
    async fn stale_intent_recheck_projection_failure_keeps_durable_event() {
        let owner = Uuid::new_v4();
        let memo_id = Uuid::new_v4();
        let previous = test_memo(owner, memo_id);
        let mut committed = previous.clone();
        committed.version += 1;
        let event =
            ProjectionIntent::new(owner, memo_id, ProjectionTarget::Version(committed.version));
        let (reconciler, store, _, events) = test_reconciler(Some(previous), true, false);
        *store.pending.lock().unwrap() = vec![event.clone()];
        *store.after_first_read.lock().unwrap() = Some(committed);
        let now = Instant::now();
        reconciler.retry_states().insert(
            event.event_id,
            RetryState {
                first_seen: now - STALE_INTENT_GRACE - Duration::from_secs(1),
                attempt_count: 0,
                next_attempt_at: now,
            },
        );

        reconciler.run_once().await.unwrap();

        let pending = store.pending.lock().unwrap().clone();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].event_id, event.event_id);
        assert_eq!(store.acknowledged.load(Ordering::Relaxed), 0);
        assert_eq!(reconciler.stats().stale_dropped_total, 0);
        assert_eq!(reconciler.stats().retry_deferred_total, 1);
        assert_eq!(reconciler.stats().pending_intents, 1);
        assert!(events.snapshot().contains(&"high-index"));
        assert!(!events.snapshot().contains(&"ack"));
    }

    #[tokio::test]
    async fn failed_stale_ack_defers_only_that_event_and_allows_healthy_reconciliation() {
        let owner = Uuid::new_v4();
        let memo_id = Uuid::new_v4();
        let memo = test_memo(owner, memo_id);
        let stale =
            ProjectionIntent::new(owner, memo_id, ProjectionTarget::Version(memo.version + 1));
        let healthy =
            ProjectionIntent::new(owner, memo_id, ProjectionTarget::Version(memo.version));
        let (reconciler, store, _, events) = test_reconciler(Some(memo), false, false);
        *store.pending.lock().unwrap() = vec![stale.clone(), healthy.clone()];
        *store.fail_ack_for.lock().unwrap() = Some(stale.event_id);

        // A known stale target must already have aged past the grace interval.
        let now = Instant::now();
        reconciler.retry_states().insert(
            stale.event_id,
            RetryState {
                first_seen: now - STALE_INTENT_GRACE - Duration::from_secs(1),
                attempt_count: 0,
                next_attempt_at: now,
            },
        );

        // The prior implementation returned early here, never processing healthy.
        reconciler.run_once().await.unwrap();
        let remaining = store.pending.lock().unwrap().clone();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].event_id, stale.event_id);
        assert_eq!(store.acknowledged.load(Ordering::Relaxed), 1);
        assert!(events.snapshot().contains(&"cache-set"));

        let counters = reconciler.stats();
        assert_eq!(counters.pending_intents, 1);
        assert_eq!(counters.stale_dropped_total, 0);
        assert_eq!(counters.reconciled_total, 1);
        assert_eq!(counters.retry_deferred_total, 1);

        // A hot-looping stale acknowledgement would overload an unhealthy DB.
        reconciler.run_once().await.unwrap();
        assert_eq!(store.acknowledged.load(Ordering::Relaxed), 1);

        // Retry after the transient failure clears, without losing the intent.
        *store.fail_ack_for.lock().unwrap() = None;
        reconciler
            .retry_states()
            .get_mut(&stale.event_id)
            .unwrap()
            .next_attempt_at = Instant::now() - Duration::from_secs(1);
        reconciler.run_once().await.unwrap();
        assert!(store.pending.lock().unwrap().is_empty());
        assert_eq!(store.acknowledged.load(Ordering::Relaxed), 2);
        assert_eq!(reconciler.stats().stale_dropped_total, 1);
        assert_eq!(reconciler.stats().reconciled_total, 1);
        assert_eq!(reconciler.stats().pending_intents, 0);
    }

    #[tokio::test]
    async fn inactive_memo_route_leaves_outbox_and_secondary_state_untouched() {
        let user_id = Uuid::new_v4();
        let memo_id = Uuid::new_v4();
        let memo = test_memo(user_id, memo_id);
        let event =
            ProjectionIntent::new(user_id, memo_id, ProjectionTarget::Version(memo.version));
        let (reconciler, store, _, events) = test_reconciler(Some(memo), false, false);
        let reconciler = reconciler.with_memo_route(
            Arc::new(FakeMemoAccessGuard {
                route: HighMemoDataRoute::Encrypted,
                fail_release: false,
                events: events.clone(),
            }),
            HighMemoDataRoute::LegacyPlaintext,
        );

        assert_eq!(
            reconciler.reconcile_event(&event).await.unwrap(),
            ReconcileOutcome::InactiveMemoRoute
        );
        assert_eq!(store.acknowledged.load(Ordering::Relaxed), 0);
        assert_eq!(
            events.snapshot(),
            vec!["memo-access-acquire", "memo-access-release"]
        );
    }

    #[tokio::test]
    async fn waiting_target_releases_access_without_acknowledging_intent() {
        let user_id = Uuid::new_v4();
        let memo_id = Uuid::new_v4();
        let memo = test_memo(user_id, memo_id);
        let event = ProjectionIntent::new(
            user_id,
            memo_id,
            ProjectionTarget::Version(memo.version + 1),
        );
        let (reconciler, store, _, events) = test_reconciler(Some(memo), false, false);
        let reconciler = reconciler.with_memo_route(
            Arc::new(FakeMemoAccessGuard {
                route: HighMemoDataRoute::Encrypted,
                fail_release: false,
                events: events.clone(),
            }),
            HighMemoDataRoute::Encrypted,
        );

        assert_eq!(
            reconciler.reconcile_event(&event).await.unwrap(),
            ReconcileOutcome::WaitingForTarget
        );
        assert_eq!(store.acknowledged.load(Ordering::Relaxed), 0);
        assert_eq!(
            events.snapshot(),
            vec!["memo-access-acquire", "memo-access-release"]
        );
    }

    #[tokio::test]
    async fn matching_memo_route_releases_access_before_outbox_ack() {
        let user_id = Uuid::new_v4();
        let memo_id = Uuid::new_v4();
        let memo = test_memo(user_id, memo_id);
        let event =
            ProjectionIntent::new(user_id, memo_id, ProjectionTarget::Version(memo.version));
        let (reconciler, store, _, events) = test_reconciler(Some(memo), false, false);
        let reconciler = reconciler.with_memo_route(
            Arc::new(FakeMemoAccessGuard {
                route: HighMemoDataRoute::Encrypted,
                fail_release: false,
                events: events.clone(),
            }),
            HighMemoDataRoute::Encrypted,
        );

        assert_eq!(
            reconciler.reconcile_event(&event).await.unwrap(),
            ReconcileOutcome::Completed
        );
        assert_eq!(store.acknowledged.load(Ordering::Relaxed), 1);
        assert_eq!(
            events.snapshot(),
            vec![
                "memo-access-acquire",
                "guard-acquire",
                "legacy-index",
                "high-index",
                "cache-delete",
                "cache-set",
                "guard-release",
                "memo-access-release",
                "ack"
            ]
        );
    }

    #[tokio::test]
    async fn memo_access_release_failure_keeps_outbox_unacknowledged() {
        let user_id = Uuid::new_v4();
        let memo_id = Uuid::new_v4();
        let memo = test_memo(user_id, memo_id);
        let event =
            ProjectionIntent::new(user_id, memo_id, ProjectionTarget::Version(memo.version));
        let (reconciler, store, _, events) = test_reconciler(Some(memo), false, false);
        let reconciler = reconciler.with_memo_route(
            Arc::new(FakeMemoAccessGuard {
                route: HighMemoDataRoute::Encrypted,
                fail_release: true,
                events: events.clone(),
            }),
            HighMemoDataRoute::Encrypted,
        );

        assert!(matches!(
            reconciler.reconcile_event(&event).await,
            Err(AppError::ServiceUnavailable(_))
        ));
        assert_eq!(store.acknowledged.load(Ordering::Relaxed), 0);
        assert_eq!(
            events.snapshot().last().copied(),
            Some("memo-access-release")
        );
    }

    #[tokio::test]
    async fn protected_search_route_never_writes_legacy_plaintext_projection() {
        let user_id = Uuid::new_v4();
        let memo_id = Uuid::new_v4();
        let memo = test_memo(user_id, memo_id);
        let event =
            ProjectionIntent::new(user_id, memo_id, ProjectionTarget::Version(memo.version));
        let (reconciler, store, _, events) = test_reconciler(Some(memo), false, false);
        let reconciler = reconciler.with_search_route(Arc::new(FakeSearchRouteReader {
            route: HighSearchQueryRoute::Protected,
            fail: false,
            events: events.clone(),
        }));

        assert_eq!(
            reconciler.reconcile_event(&event).await.unwrap(),
            ReconcileOutcome::Completed
        );
        assert_eq!(store.acknowledged.load(Ordering::Relaxed), 1);
        assert_eq!(
            events.snapshot(),
            vec![
                "guard-acquire",
                "search-route-read",
                "high-index",
                "cache-delete",
                "cache-set",
                "guard-release",
                "ack"
            ]
        );
    }

    #[tokio::test]
    async fn protected_search_route_delete_skips_legacy_plaintext_projection() {
        let user_id = Uuid::new_v4();
        let memo_id = Uuid::new_v4();
        let event = ProjectionIntent::new(user_id, memo_id, ProjectionTarget::Deleted);
        let (reconciler, store, high, events) = test_reconciler(None, false, false);
        let reconciler = reconciler.with_search_route(Arc::new(FakeSearchRouteReader {
            route: HighSearchQueryRoute::Protected,
            fail: false,
            events: events.clone(),
        }));

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
                "search-route-read",
                "high-delete",
                "cache-delete",
                "guard-release",
                "ack"
            ]
        );
    }

    #[tokio::test]
    async fn search_route_read_failure_touches_no_secondary_state_and_keeps_outbox() {
        let user_id = Uuid::new_v4();
        let memo_id = Uuid::new_v4();
        let memo = test_memo(user_id, memo_id);
        let event =
            ProjectionIntent::new(user_id, memo_id, ProjectionTarget::Version(memo.version));
        let (reconciler, store, _, events) = test_reconciler(Some(memo), false, false);
        let reconciler = reconciler.with_search_route(Arc::new(FakeSearchRouteReader {
            route: HighSearchQueryRoute::Protected,
            fail: true,
            events: events.clone(),
        }));

        assert!(reconciler.reconcile_event(&event).await.is_err());
        assert_eq!(store.acknowledged.load(Ordering::Relaxed), 0);
        assert_eq!(
            events.snapshot(),
            vec!["guard-acquire", "search-route-read", "guard-release"]
        );
    }

    #[tokio::test]
    async fn protected_search_route_without_high_projection_fails_closed() {
        let user_id = Uuid::new_v4();
        let memo_id = Uuid::new_v4();
        let memo = test_memo(user_id, memo_id);
        let event =
            ProjectionIntent::new(user_id, memo_id, ProjectionTarget::Version(memo.version));
        let (reconciler, store, _, events) = test_reconciler(Some(memo), false, false);
        let reconciler = ProjectionReconciler {
            high_search_projection: None,
            ..reconciler
        }
        .with_search_route(Arc::new(FakeSearchRouteReader {
            route: HighSearchQueryRoute::Protected,
            fail: false,
            events: events.clone(),
        }));

        assert!(matches!(
            reconciler.reconcile_event(&event).await,
            Err(AppError::ServiceUnavailable(_))
        ));
        assert_eq!(store.acknowledged.load(Ordering::Relaxed), 0);
        assert_eq!(
            events.snapshot(),
            vec!["guard-acquire", "search-route-read", "guard-release"]
        );
    }

    /// Simulate an out-of-band repair replacing memo contents while an older
    /// snapshot is being written to secondary stores without incrementing version.
    /// The correction must be durable before acknowledging the old intent.
    struct SourceChangingLegacyProjection {
        store: Arc<FakeAuthoritativeStore>,
        events: Arc<TestEvents>,
    }

    #[async_trait::async_trait]
    impl MemoSearchProjection for SourceChangingLegacyProjection {
        async fn index_memo(&self, memo: &crate::domain::memo::entity::Memo) -> AppResult<()> {
            self.events.push("legacy-index");
            let mut repaired = memo.clone();
            repaired.content = "repaired authoritative content".into();
            *self.store.memo.lock().unwrap() = Some(repaired);
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
            Ok(
                crate::infrastructure::persistence::ports::MemoSearchHitPage {
                    memo_ids: Vec::new(),
                    total: 0,
                },
            )
        }

        async fn delete_memo(&self, _id: Uuid) -> AppResult<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn same_version_source_drift_enqueues_correction_before_ack() {
        let owner = Uuid::new_v4();
        let memo_id = Uuid::new_v4();
        let memo = test_memo(owner, memo_id);
        let event = ProjectionIntent::new(owner, memo_id, ProjectionTarget::Version(memo.version));
        let (reconciler, store, _, events) = test_reconciler(Some(memo), false, false);
        let reconciler = ProjectionReconciler {
            search_projection: Arc::new(SourceChangingLegacyProjection {
                store: store.clone(),
                events: events.clone(),
            }),
            ..reconciler
        };

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
                "cache-delete",
                "cache-set",
                "enqueue",
                "guard-release",
                "ack"
            ]
        );
    }

    #[tokio::test]
    async fn successful_high_mirror_releases_guard_before_ack() {
        let user_id = Uuid::new_v4();
        let memo_id = Uuid::new_v4();
        let memo = test_memo(user_id, memo_id);
        let event =
            ProjectionIntent::new(user_id, memo_id, ProjectionTarget::Version(memo.version));
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
                "cache-delete",
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
        let event =
            ProjectionIntent::new(user_id, memo_id, ProjectionTarget::Version(memo.version));
        let (reconciler, store, _, events) = test_reconciler(Some(memo), true, false);

        assert!(reconciler.reconcile_event(&event).await.is_err());
        assert_eq!(store.acknowledged.load(Ordering::Relaxed), 0);
        assert_eq!(
            events.snapshot(),
            vec![
                "guard-acquire",
                "legacy-index",
                "high-index",
                "cache-delete",
                "cache-set",
                "guard-release"
            ]
        );
    }

    #[tokio::test]
    async fn cache_replace_failure_keeps_intent_and_invalidates_old_entry_first() {
        let user_id = Uuid::new_v4();
        let memo_id = Uuid::new_v4();
        let memo = test_memo(user_id, memo_id);
        let event =
            ProjectionIntent::new(user_id, memo_id, ProjectionTarget::Version(memo.version));
        let events = Arc::new(TestEvents::default());
        let store = Arc::new(FakeAuthoritativeStore {
            memo: Mutex::new(Some(memo)),
            after_first_read: Mutex::new(None),
            pending: Mutex::new(Vec::new()),
            fail_ack_for: Mutex::new(None),
            acknowledged: AtomicU64::new(0),
            events: events.clone(),
        });
        let cache = Arc::new(FakeCache {
            events: events.clone(),
            fail_set: true,
        });
        let legacy = Arc::new(FakeLegacyProjection {
            events: events.clone(),
        });
        let high = Arc::new(FakeHighProjection {
            events: events.clone(),
            fail_replace: false,
            deleted: Mutex::new(None),
        });
        let guard = Arc::new(FakeMutationGuard {
            events: events.clone(),
            fail_release: false,
        });
        let reconciler = ProjectionReconciler::new(store.clone(), cache, legacy, Some(high), guard);

        assert!(reconciler.reconcile_event(&event).await.is_err());
        assert_eq!(store.acknowledged.load(Ordering::Relaxed), 0);
        assert_eq!(
            events.snapshot(),
            vec![
                "guard-acquire",
                "legacy-index",
                "high-index",
                "cache-delete",
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
        let event =
            ProjectionIntent::new(user_id, memo_id, ProjectionTarget::Version(memo.version));
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
                "cache-delete",
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
    fn projection_change_detection_covers_presence_version_and_same_version_payload() {
        let user_id = Uuid::new_v4();
        let memo = test_memo(user_id, Uuid::new_v4());
        let mut changed = memo.clone();

        assert!(!projection_changed(None, None));
        assert!(projection_changed(Some(&memo), None));
        assert!(projection_changed(None, Some(&memo)));
        assert!(!projection_changed(Some(&memo), Some(&changed)));

        changed.version += 1;
        assert!(projection_changed(Some(&memo), Some(&changed)));
        changed = memo.clone();
        changed.content = "changed without version bump".into();
        assert!(projection_changed(Some(&memo), Some(&changed)));
        changed = memo.clone();
        changed.title = "changed title".into();
        assert!(projection_changed(Some(&memo), Some(&changed)));
        changed = memo.clone();
        changed.tags.push("new".into());
        assert!(projection_changed(Some(&memo), Some(&changed)));
        changed = memo.clone();
        changed.updated_at += chrono::Duration::seconds(1);
        assert!(projection_changed(Some(&memo), Some(&changed)));
        changed = memo.clone();
        changed.id = Uuid::new_v4();
        assert!(projection_changed(Some(&memo), Some(&changed)));
        changed = memo.clone();
        changed.user_id = Uuid::new_v4();
        assert!(projection_changed(Some(&memo), Some(&changed)));
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
