use std::{sync::Arc, time::Duration};

use tokio::time::sleep;
use uuid::Uuid;

use crate::{
    error::{AppError, AppResult},
    infrastructure::persistence::{
        elasticsearch::ElasticsearchClient,
        redis::RedisCache,
        scylla::{ProjectionRetry, ScyllaDB, PROJECTION_RETRY_BUCKETS},
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

    pub async fn schedule(&self, user_id: Uuid, memo_id: Uuid) -> AppResult<()> {
        let event = self
            .scylla
            .enqueue_projection_retry(user_id, memo_id)
            .await?;

        if let Err(error) = self.reconcile_event(&event).await {
            log::warn!(
                "Projection reconciliation deferred: event_id={} memo_id={} user_id={} error={error}",
                event.event_id,
                event.memo_id,
                event.user_id
            );
        }

        Ok(())
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
                        "Projection reconciliation retry failed: event_id={} memo_id={} user_id={} error={error}",
                        event.event_id,
                        event.memo_id,
                        event.user_id
                    );
                }
            }
        }

        Ok(())
    }

    async fn reconcile_event(&self, event: &ProjectionRetry) -> AppResult<()> {
        let memo = self.scylla.find_by_id(event.user_id, event.memo_id).await?;

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

        self.scylla.acknowledge_projection_retry(event).await
    }
}

fn cache_key(user_id: Uuid, memo_id: Uuid) -> String {
    format!("memo:{user_id}:{memo_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

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
