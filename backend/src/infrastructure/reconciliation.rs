use std::{sync::Arc, time::Duration};

use tokio::time::sleep;
use uuid::Uuid;

use crate::{
    error::{AppError, AppResult},
    infrastructure::persistence::{
        elasticsearch::ElasticsearchClient,
        redis::RedisCache,
        scylla::{ProjectionRetry, ScyllaDB, PROJECTION_DELETE_TARGET, PROJECTION_RETRY_BUCKETS},
    },
};

const CACHE_TTL: Duration = Duration::from_secs(3600);
const POLL_INTERVAL: Duration = Duration::from_secs(2);

pub struct ProjectionReconciler {
    scylla: Arc<ScyllaDB>,
    redis: Arc<RedisCache>,
    elasticsearch: Arc<ElasticsearchClient>,
}

impl ProjectionReconciler {
    pub fn new(
        scylla: Arc<ScyllaDB>,
        redis: Arc<RedisCache>,
        elasticsearch: Arc<ElasticsearchClient>,
    ) -> Self {
        Self {
            scylla,
            redis,
            elasticsearch,
        }
    }

    pub async fn prepare(
        &self,
        user_id: Uuid,
        memo_id: Uuid,
        target_version: i32,
    ) -> AppResult<ProjectionRetry> {
        self.scylla
            .enqueue_projection_retry(user_id, memo_id, target_version)
            .await
    }

    pub async fn reconcile_now(&self, event: &ProjectionRetry) {
        if let Err(error) = self.reconcile_event(event).await {
            log::warn!(
                "Projection reconciliation deferred: event_id={} memo_id={} user_id={} target_version={} error={error}",
                event.event_id,
                event.memo_id,
                event.user_id,
                event.target_version
            );
        }
    }

    pub async fn run(self: Arc<Self>) {
        loop {
            if let Err(error) = self.run_once().await {
                log::warn!("Projection reconciliation pass failed: {error}");
            }
            sleep(POLL_INTERVAL).await;
        }
    }

    async fn run_once(&self) -> AppResult<()> {
        for bucket in 0..PROJECTION_RETRY_BUCKETS {
            let events = self.scylla.list_projection_retries(bucket).await?;
            for event in events {
                if let Err(error) = self.reconcile_event(&event).await {
                    log::warn!(
                        "Projection reconciliation retry failed: event_id={} memo_id={} user_id={} target_version={} error={error}",
                        event.event_id,
                        event.memo_id,
                        event.user_id,
                        event.target_version
                    );
                }
            }
        }

        Ok(())
    }

    async fn reconcile_event(&self, event: &ProjectionRetry) -> AppResult<()> {
        let memo = self.scylla.find_by_id(event.user_id, event.memo_id).await?;

        if !target_reached(event, memo.as_ref()) {
            return Ok(());
        }

        let mut failures = Vec::new();
        let cache_key = cache_key(event.user_id, event.memo_id);

        match memo {
            Some(memo) => {
                if let Err(error) = self.elasticsearch.index_memo(&memo).await {
                    failures.push(format!("elasticsearch={error}"));
                }
                if let Err(error) = self.redis.set(&cache_key, &memo, Some(CACHE_TTL)).await {
                    failures.push(format!("redis={error}"));
                }
            }
            None => {
                if let Err(error) = self.elasticsearch.delete_memo(event.memo_id).await {
                    failures.push(format!("elasticsearch={error}"));
                }
                if let Err(error) = self.redis.delete(&cache_key).await {
                    failures.push(format!("redis={error}"));
                }
            }
        }

        if !failures.is_empty() {
            return Err(AppError::DatabaseError(failures.join("; ")));
        }

        let current = self.scylla.find_by_id(event.user_id, event.memo_id).await?;
        if projection_state(memo.as_ref()) != projection_state(current.as_ref()) {
            let target_version = current
                .as_ref()
                .map(|memo| memo.version)
                .unwrap_or(PROJECTION_DELETE_TARGET);
            self.scylla
                .enqueue_projection_retry(event.user_id, event.memo_id, target_version)
                .await?;
        }

        self.scylla.acknowledge_projection_retry(event).await
    }

    pub async fn cancel(&self, event: &ProjectionRetry) {
        if let Err(error) = self.scylla.acknowledge_projection_retry(event).await {
            log::warn!(
                "Failed to cancel unused projection intent: event_id={} memo_id={} user_id={} error={error}",
                event.event_id,
                event.memo_id,
                event.user_id
            );
        }
    }
}

fn projection_state(memo: Option<&crate::domain::memo::entity::Memo>) -> Option<i32> {
    memo.map(|memo| memo.version)
}

fn target_reached(
    event: &ProjectionRetry,
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

    fn retry(user_id: Uuid, memo_id: Uuid, target_version: i32) -> ProjectionRetry {
        ProjectionRetry {
            bucket: ScyllaDB::projection_retry_bucket(memo_id),
            event_id: Uuid::new_v4(),
            user_id,
            memo_id,
            target_version,
        }
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
}
