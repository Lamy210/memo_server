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
                "Projection reconciliation deferred: memo_id={} user_id={} target_version={} error={error}",
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
                        "Projection reconciliation retry failed: memo_id={} user_id={} target_version={} error={error}",
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

        let _ = self.scylla.acknowledge_projection_retry(event).await?;
        Ok(())
    }
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
    fn cache_key_is_tenant_scoped() {
        let user_id = Uuid::new_v4();
        let memo_id = Uuid::new_v4();

        assert_eq!(
            cache_key(user_id, memo_id),
            format!("memo:{user_id}:{memo_id}")
        );
    }
}
