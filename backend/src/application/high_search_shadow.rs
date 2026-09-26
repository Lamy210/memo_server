use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

use tokio::{sync::Semaphore, time::timeout};
use uuid::Uuid;

use crate::{
    application::crypto_search_orchestration::HighSearchQueryReader,
    error::{AppError, AppResult},
};

const METRICS_LOG_EVERY_COMPLETIONS: u64 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HighSearchShadowStats {
    pub completed: u64,
    pub total_matches: u64,
    pub total_mismatches: u64,
    pub failures: u64,
    pub timeouts: u64,
    pub dropped_capacity: u64,
}

#[derive(Default)]
struct HighSearchShadowCounters {
    completed: AtomicU64,
    total_matches: AtomicU64,
    total_mismatches: AtomicU64,
    failures: AtomicU64,
    timeouts: AtomicU64,
    dropped_capacity: AtomicU64,
}

pub struct HighSearchShadowObserver {
    reader: Arc<dyn HighSearchQueryReader>,
    permits: Arc<Semaphore>,
    timeout: Duration,
    counters: Arc<HighSearchShadowCounters>,
}

impl HighSearchShadowObserver {
    pub fn new(
        reader: Arc<dyn HighSearchQueryReader>,
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
    pub fn observe(
        &self,
        query: &str,
        tag: Option<&str>,
        owner_partition: Uuid,
        page: usize,
        limit: usize,
        legacy_total: usize,
    ) {
        let Ok(permit) = self.permits.clone().try_acquire_owned() else {
            self.counters
                .dropped_capacity
                .fetch_add(1, Ordering::Relaxed);
            return;
        };

        let reader = self.reader.clone();
        let query = query.to_owned();
        let tag = tag.map(str::to_owned);
        let timeout_duration = self.timeout;
        let counters = self.counters.clone();

        tokio::spawn(async move {
            let _permit = permit;
            match timeout(
                timeout_duration,
                reader.search_memo_ids(
                    owner_partition,
                    &query,
                    tag.as_deref(),
                    page,
                    limit,
                ),
            )
            .await
            {
                Ok(Ok(result)) => {
                    if result.total == legacy_total {
                        counters.total_matches.fetch_add(1, Ordering::Relaxed);
                    } else {
                        counters
                            .total_mismatches
                            .fetch_add(1, Ordering::Relaxed);
                    }
                }
                Ok(Err(_)) => {
                    counters.failures.fetch_add(1, Ordering::Relaxed);
                }
                Err(_) => {
                    counters.timeouts.fetch_add(1, Ordering::Relaxed);
                }
            }

            let completed = counters.completed.fetch_add(1, Ordering::Relaxed) + 1;
            if completed.is_multiple_of(METRICS_LOG_EVERY_COMPLETIONS) {
                let stats = snapshot(&counters);
                log::info!(
                    "HIGH search shadow aggregate: completed={} total_matches={} total_mismatches={} failures={} timeouts={} dropped_capacity={}",
                    stats.completed,
                    stats.total_matches,
                    stats.total_mismatches,
                    stats.failures,
                    stats.timeouts,
                    stats.dropped_capacity
                );
            }
        });
    }

    pub fn stats(&self) -> HighSearchShadowStats {
        snapshot(&self.counters)
    }
}

fn snapshot(counters: &HighSearchShadowCounters) -> HighSearchShadowStats {
    HighSearchShadowStats {
        completed: counters.completed.load(Ordering::Relaxed),
        total_matches: counters.total_matches.load(Ordering::Relaxed),
        total_mismatches: counters.total_mismatches.load(Ordering::Relaxed),
        failures: counters.failures.load(Ordering::Relaxed),
        timeouts: counters.timeouts.load(Ordering::Relaxed),
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

            self.result
                .lock()
                .unwrap()
                .take()
                .unwrap_or_else(|| {
                    Ok(HighSearchProjectionPage {
                        memo_ids: Vec::new(),
                        total: 0,
                    })
                })
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

        assert!(HighSearchShadowObserver::new(
            reader.clone(),
            0,
            Duration::from_millis(10),
        )
        .is_err());
        assert!(HighSearchShadowObserver::new(reader, 1, Duration::ZERO).is_err());
    }

    #[tokio::test]
    async fn shadow_records_total_match_without_exposing_request_data() {
        let reader = Arc::new(FakeReader {
            result: Mutex::new(Some(Ok(HighSearchProjectionPage {
                memo_ids: vec![Uuid::new_v4()],
                total: 7,
            }))),
            block: None,
        });
        let observer =
            HighSearchShadowObserver::new(reader, 1, Duration::from_millis(50)).unwrap();

        observer.observe("private query", Some("private-tag"), Uuid::new_v4(), 1, 20, 7);
        wait_for_completion(&observer).await;

        assert_eq!(
            observer.stats(),
            HighSearchShadowStats {
                completed: 1,
                total_matches: 1,
                total_mismatches: 0,
                failures: 0,
                timeouts: 0,
                dropped_capacity: 0,
            }
        );
    }

    #[tokio::test]
    async fn saturated_shadow_drops_observation_without_queueing() {
        let block = Arc::new(Notify::new());
        let reader = Arc::new(FakeReader {
            result: Mutex::new(None),
            block: Some(block.clone()),
        });
        let observer =
            HighSearchShadowObserver::new(reader, 1, Duration::from_secs(1)).unwrap();

        observer.observe("first", None, Uuid::new_v4(), 1, 20, 0);
        observer.observe("second", None, Uuid::new_v4(), 1, 20, 0);

        assert_eq!(observer.stats().dropped_capacity, 1);
        block.notify_one();
        wait_for_completion(&observer).await;
    }
}
