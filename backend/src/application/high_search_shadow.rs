use std::{
    collections::HashSet,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

use tokio::{sync::Semaphore, time::timeout};
use uuid::Uuid;

use crate::{
    application::{
        crypto_search_orchestration::HighSearchQueryReader,
        high_search_routing::HighSearchQueryRoute, maintenance::HighSearchQueryGuard,
    },
    error::{AppError, AppResult},
};

const METRICS_LOG_EVERY_COMPLETIONS: u64 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HighSearchShadowStats {
    pub completed: u64,
    pub total_matches: u64,
    pub total_mismatches: u64,
    pub complete_set_observations: u64,
    pub complete_set_matches: u64,
    pub complete_set_mismatches: u64,
    pub page_overlap_intersection: u64,
    pub page_overlap_legacy_only: u64,
    pub page_overlap_protected_only: u64,
    pub failures: u64,
    pub timeouts: u64,
    pub dropped_route_change: u64,
    pub dropped_capacity: u64,
}

#[derive(Default)]
struct HighSearchShadowCounters {
    completed: AtomicU64,
    total_matches: AtomicU64,
    total_mismatches: AtomicU64,
    complete_set_observations: AtomicU64,
    complete_set_matches: AtomicU64,
    complete_set_mismatches: AtomicU64,
    page_overlap_intersection: AtomicU64,
    page_overlap_legacy_only: AtomicU64,
    page_overlap_protected_only: AtomicU64,
    failures: AtomicU64,
    timeouts: AtomicU64,
    dropped_route_change: AtomicU64,
    dropped_capacity: AtomicU64,
}

pub struct HighSearchShadowObservation<'a, I> {
    pub query: &'a str,
    pub tag: Option<&'a str>,
    pub owner_partition: Uuid,
    pub page: usize,
    pub limit: usize,
    pub legacy_memo_ids: I,
    pub legacy_total: usize,
    pub legacy_route_generation: i64,
}

pub struct HighSearchShadowObserver {
    reader: Arc<dyn HighSearchQueryReader>,
    query_guard: Arc<dyn HighSearchQueryGuard>,
    permits: Arc<Semaphore>,
    timeout: Duration,
    counters: Arc<HighSearchShadowCounters>,
}

impl HighSearchShadowObserver {
    pub fn new(
        reader: Arc<dyn HighSearchQueryReader>,
        query_guard: Arc<dyn HighSearchQueryGuard>,
        max_concurrency: usize,
        timeout: Duration,
    ) -> AppResult<Self> {
        if max_concurrency == 0 || timeout.is_zero() {
            return Err(AppError::ValidationError(
                "HIGH search shadow concurrency and timeout must be greater than zero".into(),
            ));
        }

        Ok(Self {
            reader,
            query_guard,
            permits: Arc::new(Semaphore::new(max_concurrency)),
            timeout,
            counters: Arc::new(HighSearchShadowCounters::default()),
        })
    }

    /// Schedule one best-effort protected-search observation without delaying
    /// the user-visible legacy search response.
    ///
    /// Query plaintext and result IDs are deliberately never logged. When the
    /// bounded worker pool is saturated the observation is dropped rather than
    /// adding unbounded tasks or backpressure to the request path.
    pub fn observe<I>(&self, observation: HighSearchShadowObservation<'_, I>)
    where
        I: IntoIterator<Item = Uuid>,
    {
        let Ok(permit) = self.permits.clone().try_acquire_owned() else {
            self.counters
                .dropped_capacity
                .fetch_add(1, Ordering::Relaxed);
            return;
        };

        let HighSearchShadowObservation {
            query,
            tag,
            owner_partition,
            page,
            limit,
            legacy_memo_ids,
            legacy_total,
            legacy_route_generation,
        } = observation;
        let reader = self.reader.clone();
        let query_guard = self.query_guard.clone();
        let query = query.to_owned();
        let tag = tag.map(str::to_owned);
        let legacy_memo_ids = legacy_memo_ids.into_iter().collect::<Vec<_>>();
        let timeout_duration = self.timeout;
        let counters = self.counters.clone();

        tokio::spawn(async move {
            match query_guard.acquire_query().await {
                Err(_) => {
                    // Maintenance is expected to reject new protected reads.
                    // Infrastructure failures are also intentionally reduced to
                    // the aggregate shadow failure counter.
                    counters.failures.fetch_add(1, Ordering::Relaxed);
                }
                Ok(query_permit) => {
                    let shadow_snapshot = query_permit.route_snapshot();
                    if shadow_snapshot.route != HighSearchQueryRoute::Legacy
                        || shadow_snapshot.generation != legacy_route_generation
                    {
                        if query_permit.release().await.is_err() {
                            counters.failures.fetch_add(1, Ordering::Relaxed);
                        } else {
                            // Do not compare a legacy result captured under one
                            // route generation with protected results admitted
                            // after a cutover/rollback.
                            counters
                                .dropped_route_change
                                .fetch_add(1, Ordering::Relaxed);
                        }
                    } else {
                        let query_result = timeout(
                            timeout_duration,
                            reader.search_memo_ids(
                                owner_partition,
                                &query,
                                tag.as_deref(),
                                page,
                                limit,
                            ),
                        )
                        .await;
                        let release_result = query_permit.release().await;

                        match (query_result, release_result) {
                            (_, Err(_)) => {
                                // A missing/stuck read lease must remain visible as
                                // a failed observation; maintenance recovery is
                                // fail-closed and will see the persisted lease.
                                counters.failures.fetch_add(1, Ordering::Relaxed);
                            }
                            (Ok(Ok(result)), Ok(())) => {
                                record_comparison(
                                    &counters,
                                    &legacy_memo_ids,
                                    legacy_total,
                                    &result.memo_ids,
                                    result.total,
                                    page,
                                    limit,
                                );
                            }
                            (Ok(Err(_)), Ok(())) => {
                                counters.failures.fetch_add(1, Ordering::Relaxed);
                            }
                            (Err(_), Ok(())) => {
                                counters.timeouts.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                }
            }

            // Publish completion only after capacity is available again. This
            // makes the completed counter a reliable admission boundary for
            // tests and operational sampling.
            drop(permit);
            let completed = counters.completed.fetch_add(1, Ordering::Relaxed) + 1;
            if completed.is_multiple_of(METRICS_LOG_EVERY_COMPLETIONS) {
                let stats = snapshot(&counters);
                log::info!(
                    "HIGH search shadow aggregate: completed={} total_matches={} total_mismatches={} complete_set_observations={} complete_set_matches={} complete_set_mismatches={} page_overlap_intersection={} page_overlap_legacy_only={} page_overlap_protected_only={} failures={} timeouts={} dropped_route_change={} dropped_capacity={}",
                    stats.completed,
                    stats.total_matches,
                    stats.total_mismatches,
                    stats.complete_set_observations,
                    stats.complete_set_matches,
                    stats.complete_set_mismatches,
                    stats.page_overlap_intersection,
                    stats.page_overlap_legacy_only,
                    stats.page_overlap_protected_only,
                    stats.failures,
                    stats.timeouts,
                    stats.dropped_route_change,
                    stats.dropped_capacity
                );
            }
        });
    }

    pub fn stats(&self) -> HighSearchShadowStats {
        snapshot(&self.counters)
    }
}

fn record_comparison(
    counters: &HighSearchShadowCounters,
    legacy_memo_ids: &[Uuid],
    legacy_total: usize,
    protected_memo_ids: &[Uuid],
    protected_total: usize,
    page: usize,
    limit: usize,
) {
    if protected_total == legacy_total {
        counters.total_matches.fetch_add(1, Ordering::Relaxed);
    } else {
        counters.total_mismatches.fetch_add(1, Ordering::Relaxed);
    }

    let legacy_set = legacy_memo_ids.iter().copied().collect::<HashSet<_>>();
    let protected_set = protected_memo_ids.iter().copied().collect::<HashSet<_>>();
    let intersection = legacy_set.intersection(&protected_set).count();
    let legacy_only = legacy_set.len().saturating_sub(intersection);
    let protected_only = protected_set.len().saturating_sub(intersection);

    counters
        .page_overlap_intersection
        .fetch_add(intersection as u64, Ordering::Relaxed);
    counters
        .page_overlap_legacy_only
        .fetch_add(legacy_only as u64, Ordering::Relaxed);
    counters
        .page_overlap_protected_only
        .fetch_add(protected_only as u64, Ordering::Relaxed);

    let complete_legacy = page == 1 && legacy_total <= limit && legacy_set.len() == legacy_total;
    let complete_protected =
        page == 1 && protected_total <= limit && protected_set.len() == protected_total;
    if complete_legacy && complete_protected {
        counters
            .complete_set_observations
            .fetch_add(1, Ordering::Relaxed);
        if legacy_set == protected_set {
            counters
                .complete_set_matches
                .fetch_add(1, Ordering::Relaxed);
        } else {
            counters
                .complete_set_mismatches
                .fetch_add(1, Ordering::Relaxed);
        }
    }
}

fn snapshot(counters: &HighSearchShadowCounters) -> HighSearchShadowStats {
    HighSearchShadowStats {
        completed: counters.completed.load(Ordering::Relaxed),
        total_matches: counters.total_matches.load(Ordering::Relaxed),
        total_mismatches: counters.total_mismatches.load(Ordering::Relaxed),
        complete_set_observations: counters.complete_set_observations.load(Ordering::Relaxed),
        complete_set_matches: counters.complete_set_matches.load(Ordering::Relaxed),
        complete_set_mismatches: counters.complete_set_mismatches.load(Ordering::Relaxed),
        page_overlap_intersection: counters.page_overlap_intersection.load(Ordering::Relaxed),
        page_overlap_legacy_only: counters.page_overlap_legacy_only.load(Ordering::Relaxed),
        page_overlap_protected_only: counters.page_overlap_protected_only.load(Ordering::Relaxed),
        failures: counters.failures.load(Ordering::Relaxed),
        timeouts: counters.timeouts.load(Ordering::Relaxed),
        dropped_route_change: counters.dropped_route_change.load(Ordering::Relaxed),
        dropped_capacity: counters.dropped_capacity.load(Ordering::Relaxed),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use tokio::sync::Notify;

    use super::*;
    use crate::application::crypto_search_projection::HighSearchProjectionPage;

    struct FakeReader {
        result: Mutex<Option<AppResult<HighSearchProjectionPage>>>,
        block: Option<Arc<Notify>>,
    }

    #[async_trait]
    impl HighSearchQueryReader for FakeReader {
        async fn search_memo_ids(
            &self,
            _owner_partition: Uuid,
            _query: &str,
            _tag: Option<&str>,
            _page: usize,
            _limit: usize,
        ) -> AppResult<HighSearchProjectionPage> {
            if let Some(block) = self.block.as_ref() {
                block.notified().await;
            }

            self.result.lock().unwrap().take().unwrap_or_else(|| {
                Ok(HighSearchProjectionPage {
                    memo_ids: Vec::new(),
                    total: 0,
                })
            })
        }
    }

    fn observer(
        reader: Arc<FakeReader>,
        max_concurrency: usize,
        timeout: Duration,
    ) -> AppResult<HighSearchShadowObserver> {
        HighSearchShadowObserver::new(
            reader,
            Arc::new(crate::application::maintenance::UnrestrictedHighSearchQueryGuard),
            max_concurrency,
            timeout,
        )
    }

    struct FakeQueryGuard {
        events: Arc<Mutex<Vec<&'static str>>>,
        reject: bool,
        fail_release: bool,
    }

    struct FakeQueryPermit {
        events: Arc<Mutex<Vec<&'static str>>>,
        fail_release: bool,
    }

    #[async_trait]
    impl HighSearchQueryGuard for FakeQueryGuard {
        async fn acquire_query(
            &self,
        ) -> AppResult<Box<dyn crate::application::maintenance::HighSearchQueryPermit>> {
            self.events.lock().unwrap().push("query-acquire");
            if self.reject {
                return Err(AppError::ServiceUnavailable(
                    "protected query maintenance active".into(),
                ));
            }
            Ok(Box::new(FakeQueryPermit {
                events: self.events.clone(),
                fail_release: self.fail_release,
            }))
        }
    }

    #[async_trait]
    impl crate::application::maintenance::HighSearchQueryPermit for FakeQueryPermit {
        fn route_snapshot(
            &self,
        ) -> crate::application::high_search_routing::HighSearchQueryRouteSnapshot {
            crate::application::high_search_routing::HighSearchQueryRouteSnapshot {
                route: crate::application::high_search_routing::HighSearchQueryRoute::Legacy,
                generation: 0,
            }
        }

        async fn release(self: Box<Self>) -> AppResult<()> {
            self.events.lock().unwrap().push("query-release");
            if self.fail_release {
                Err(AppError::ServiceUnavailable(
                    "protected query lease release failed".into(),
                ))
            } else {
                Ok(())
            }
        }
    }

    async fn wait_for_completion(observer: &HighSearchShadowObserver) {
        tokio::time::timeout(Duration::from_millis(100), async {
            while observer.stats().completed == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[test]
    fn shadow_requires_positive_bounds() {
        let reader = Arc::new(FakeReader {
            result: Mutex::new(None),
            block: None,
        });

        assert!(observer(reader.clone(), 0, Duration::from_millis(10)).is_err());
        assert!(observer(reader, 1, Duration::ZERO).is_err());
    }

    #[tokio::test]
    async fn protected_query_is_wrapped_by_maintenance_lease() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let reader = Arc::new(FakeReader {
            result: Mutex::new(Some(Ok(HighSearchProjectionPage {
                memo_ids: Vec::new(),
                total: 0,
            }))),
            block: None,
        });
        let observer = HighSearchShadowObserver::new(
            reader,
            Arc::new(FakeQueryGuard {
                events: events.clone(),
                reject: false,
                fail_release: false,
            }),
            1,
            Duration::from_millis(50),
        )
        .unwrap();

        observer.observe(HighSearchShadowObservation {
            query: "private query",
            tag: None,
            owner_partition: Uuid::new_v4(),
            page: 1,
            limit: 20,
            legacy_memo_ids: Vec::<Uuid>::new(),
            legacy_total: 0,
            legacy_route_generation: 0,
        });
        wait_for_completion(&observer).await;

        assert_eq!(
            *events.lock().unwrap(),
            vec!["query-acquire", "query-release"]
        );
        assert_eq!(observer.stats().total_matches, 1);
        assert_eq!(observer.stats().failures, 0);
    }

    #[tokio::test]
    async fn route_generation_change_drops_shadow_before_reader_call() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let reader = Arc::new(FakeReader {
            result: Mutex::new(Some(Ok(HighSearchProjectionPage {
                memo_ids: vec![Uuid::new_v4()],
                total: 1,
            }))),
            block: None,
        });
        let reader_probe = reader.clone();
        let observer = HighSearchShadowObserver::new(
            reader,
            Arc::new(FakeQueryGuard {
                events: events.clone(),
                reject: false,
                fail_release: false,
            }),
            1,
            Duration::from_millis(50),
        )
        .unwrap();

        observer.observe(HighSearchShadowObservation {
            query: "private query",
            tag: None,
            owner_partition: Uuid::new_v4(),
            page: 1,
            limit: 20,
            legacy_memo_ids: Vec::<Uuid>::new(),
            legacy_total: 0,
            legacy_route_generation: 1,
        });
        wait_for_completion(&observer).await;

        assert_eq!(
            *events.lock().unwrap(),
            vec!["query-acquire", "query-release"]
        );
        assert!(reader_probe.result.lock().unwrap().is_some());
        assert_eq!(observer.stats().dropped_route_change, 1);
        assert_eq!(observer.stats().failures, 0);
    }

    #[tokio::test]
    async fn maintenance_rejection_skips_protected_reader() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let reader = Arc::new(FakeReader {
            result: Mutex::new(Some(Ok(HighSearchProjectionPage {
                memo_ids: vec![Uuid::new_v4()],
                total: 1,
            }))),
            block: None,
        });
        let reader_probe = reader.clone();
        let observer = HighSearchShadowObserver::new(
            reader,
            Arc::new(FakeQueryGuard {
                events: events.clone(),
                reject: true,
                fail_release: false,
            }),
            1,
            Duration::from_millis(50),
        )
        .unwrap();

        observer.observe(HighSearchShadowObservation {
            query: "private query",
            tag: None,
            owner_partition: Uuid::new_v4(),
            page: 1,
            limit: 20,
            legacy_memo_ids: Vec::<Uuid>::new(),
            legacy_total: 0,
            legacy_route_generation: 0,
        });
        wait_for_completion(&observer).await;

        assert_eq!(*events.lock().unwrap(), vec!["query-acquire"]);
        assert!(reader_probe.result.lock().unwrap().is_some());
        assert_eq!(observer.stats().failures, 1);
        assert_eq!(observer.stats().total_matches, 0);
    }

    #[tokio::test]
    async fn query_lease_release_failure_is_counted_as_shadow_failure() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let reader = Arc::new(FakeReader {
            result: Mutex::new(Some(Ok(HighSearchProjectionPage {
                memo_ids: Vec::new(),
                total: 0,
            }))),
            block: None,
        });
        let observer = HighSearchShadowObserver::new(
            reader,
            Arc::new(FakeQueryGuard {
                events: events.clone(),
                reject: false,
                fail_release: true,
            }),
            1,
            Duration::from_millis(50),
        )
        .unwrap();

        observer.observe(HighSearchShadowObservation {
            query: "private query",
            tag: None,
            owner_partition: Uuid::new_v4(),
            page: 1,
            limit: 20,
            legacy_memo_ids: Vec::<Uuid>::new(),
            legacy_total: 0,
            legacy_route_generation: 0,
        });
        wait_for_completion(&observer).await;

        assert_eq!(
            *events.lock().unwrap(),
            vec!["query-acquire", "query-release"]
        );
        assert_eq!(observer.stats().failures, 1);
        assert_eq!(observer.stats().total_matches, 0);
    }

    #[tokio::test]
    async fn shadow_records_complete_set_match_without_exposing_request_data() {
        let memo_id = Uuid::new_v4();
        let reader = Arc::new(FakeReader {
            result: Mutex::new(Some(Ok(HighSearchProjectionPage {
                memo_ids: vec![memo_id],
                total: 1,
            }))),
            block: None,
        });
        let observer = observer(reader, 1, Duration::from_millis(50)).unwrap();

        observer.observe(HighSearchShadowObservation {
            query: "private query",
            tag: Some("private-tag"),
            owner_partition: Uuid::new_v4(),
            page: 1,
            limit: 20,
            legacy_memo_ids: vec![memo_id],
            legacy_total: 1,
            legacy_route_generation: 0,
        });
        wait_for_completion(&observer).await;

        assert_eq!(
            observer.stats(),
            HighSearchShadowStats {
                completed: 1,
                total_matches: 1,
                total_mismatches: 0,
                complete_set_observations: 1,
                complete_set_matches: 1,
                complete_set_mismatches: 0,
                page_overlap_intersection: 1,
                page_overlap_legacy_only: 0,
                page_overlap_protected_only: 0,
                failures: 0,
                timeouts: 0,
                dropped_route_change: 0,
                dropped_capacity: 0,
            }
        );
    }

    #[tokio::test]
    async fn shadow_records_page_overlap_without_logging_or_persisting_ids() {
        let shared = Uuid::new_v4();
        let legacy_only = Uuid::new_v4();
        let protected_only = Uuid::new_v4();
        let reader = Arc::new(FakeReader {
            result: Mutex::new(Some(Ok(HighSearchProjectionPage {
                memo_ids: vec![shared, protected_only],
                total: 9,
            }))),
            block: None,
        });
        let observer = observer(reader, 1, Duration::from_millis(50)).unwrap();

        observer.observe(HighSearchShadowObservation {
            query: "private query",
            tag: None,
            owner_partition: Uuid::new_v4(),
            page: 2,
            limit: 2,
            legacy_memo_ids: vec![shared, legacy_only],
            legacy_total: 8,
            legacy_route_generation: 0,
        });
        wait_for_completion(&observer).await;

        let stats = observer.stats();
        assert_eq!(stats.total_mismatches, 1);
        assert_eq!(stats.complete_set_observations, 0);
        assert_eq!(stats.page_overlap_intersection, 1);
        assert_eq!(stats.page_overlap_legacy_only, 1);
        assert_eq!(stats.page_overlap_protected_only, 1);
    }

    #[tokio::test]
    async fn shadow_failure_is_counted_without_affecting_other_outcomes() {
        let reader = Arc::new(FakeReader {
            result: Mutex::new(Some(Err(AppError::DatabaseError(
                "protected search failed".into(),
            )))),
            block: None,
        });
        let observer = observer(reader, 1, Duration::from_millis(50)).unwrap();

        observer.observe(HighSearchShadowObservation {
            query: "private query",
            tag: None,
            owner_partition: Uuid::new_v4(),
            page: 1,
            limit: 20,
            legacy_memo_ids: Vec::<Uuid>::new(),
            legacy_total: 3,
            legacy_route_generation: 0,
        });
        wait_for_completion(&observer).await;

        assert_eq!(
            observer.stats(),
            HighSearchShadowStats {
                completed: 1,
                total_matches: 0,
                total_mismatches: 0,
                complete_set_observations: 0,
                complete_set_matches: 0,
                complete_set_mismatches: 0,
                page_overlap_intersection: 0,
                page_overlap_legacy_only: 0,
                page_overlap_protected_only: 0,
                failures: 1,
                timeouts: 0,
                dropped_route_change: 0,
                dropped_capacity: 0,
            }
        );
    }

    #[tokio::test]
    async fn shadow_timeout_is_counted_and_releases_capacity() {
        let block = Arc::new(Notify::new());
        let reader = Arc::new(FakeReader {
            result: Mutex::new(None),
            block: Some(block),
        });
        let observer = observer(reader, 1, Duration::from_millis(1)).unwrap();

        observer.observe(HighSearchShadowObservation {
            query: "private query",
            tag: None,
            owner_partition: Uuid::new_v4(),
            page: 1,
            limit: 20,
            legacy_memo_ids: Vec::<Uuid>::new(),
            legacy_total: 0,
            legacy_route_generation: 0,
        });
        wait_for_completion(&observer).await;

        assert_eq!(observer.stats().timeouts, 1);
        assert_eq!(observer.stats().dropped_capacity, 0);

        // The timed-out task has dropped its owned semaphore permit, so the
        // next observation can be admitted rather than being permanently stuck.
        observer.observe(HighSearchShadowObservation {
            query: "next query",
            tag: None,
            owner_partition: Uuid::new_v4(),
            page: 1,
            limit: 20,
            legacy_memo_ids: Vec::<Uuid>::new(),
            legacy_total: 0,
            legacy_route_generation: 0,
        });
        assert_eq!(observer.stats().dropped_capacity, 0);
    }

    #[tokio::test]
    async fn saturated_shadow_drops_observation_without_queueing() {
        let block = Arc::new(Notify::new());
        let reader = Arc::new(FakeReader {
            result: Mutex::new(None),
            block: Some(block.clone()),
        });
        let observer = observer(reader, 1, Duration::from_secs(1)).unwrap();

        observer.observe(HighSearchShadowObservation {
            query: "first",
            tag: None,
            owner_partition: Uuid::new_v4(),
            page: 1,
            limit: 20,
            legacy_memo_ids: Vec::<Uuid>::new(),
            legacy_total: 0,
            legacy_route_generation: 0,
        });
        observer.observe(HighSearchShadowObservation {
            query: "second",
            tag: None,
            owner_partition: Uuid::new_v4(),
            page: 1,
            limit: 20,
            legacy_memo_ids: Vec::<Uuid>::new(),
            legacy_total: 0,
            legacy_route_generation: 0,
        });

        assert_eq!(observer.stats().dropped_capacity, 1);
        block.notify_one();
        wait_for_completion(&observer).await;
    }
}
