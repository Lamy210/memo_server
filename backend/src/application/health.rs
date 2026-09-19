use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio::time::timeout;

const DEFAULT_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

#[async_trait]
pub trait HealthProbe: Send + Sync {
    async fn check(&self) -> bool;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ComponentStatus {
    Ok,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReadinessStatus {
    Ready,
    Degraded,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HealthChecks {
    pub scylla: ComponentStatus,
    pub redis: ComponentStatus,
    pub elasticsearch: ComponentStatus,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReadinessResponse {
    pub ready: bool,
    pub status: ReadinessStatus,
    pub checks: HealthChecks,
    pub timestamp: DateTime<Utc>,
}

pub struct HealthService {
    scylla: Arc<dyn HealthProbe>,
    redis: Arc<dyn HealthProbe>,
    elasticsearch: Arc<dyn HealthProbe>,
    probe_timeout: Duration,
}

impl HealthService {
    pub fn new(
        scylla: Arc<dyn HealthProbe>,
        redis: Arc<dyn HealthProbe>,
        elasticsearch: Arc<dyn HealthProbe>,
    ) -> Self {
        Self {
            scylla,
            redis,
            elasticsearch,
            probe_timeout: DEFAULT_PROBE_TIMEOUT,
        }
    }

    pub async fn readiness(&self) -> ReadinessResponse {
        let (scylla, redis, elasticsearch) = tokio::join!(
            check_with_timeout("scylla", &self.scylla, self.probe_timeout),
            check_with_timeout("redis", &self.redis, self.probe_timeout),
            check_with_timeout("elasticsearch", &self.elasticsearch, self.probe_timeout,),
        );

        readiness_from_checks(scylla, redis, elasticsearch)
    }
}

async fn check_with_timeout(
    name: &str,
    probe: &Arc<dyn HealthProbe>,
    probe_timeout: Duration,
) -> ComponentStatus {
    match timeout(probe_timeout, probe.check()).await {
        Ok(true) => ComponentStatus::Ok,
        Ok(false) => ComponentStatus::Down,
        Err(_) => {
            log::warn!("Health probe timed out: component={name}");
            ComponentStatus::Down
        }
    }
}

fn readiness_from_checks(
    scylla: ComponentStatus,
    redis: ComponentStatus,
    elasticsearch: ComponentStatus,
) -> ReadinessResponse {
    let ready = scylla == ComponentStatus::Ok;
    let status = if !ready {
        ReadinessStatus::Unavailable
    } else if redis == ComponentStatus::Ok && elasticsearch == ComponentStatus::Ok {
        ReadinessStatus::Ready
    } else {
        ReadinessStatus::Degraded
    };

    ReadinessResponse {
        ready,
        status,
        checks: HealthChecks {
            scylla,
            redis,
            elasticsearch,
        },
        timestamp: Utc::now(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_dependencies_healthy_is_ready() {
        let result = readiness_from_checks(
            ComponentStatus::Ok,
            ComponentStatus::Ok,
            ComponentStatus::Ok,
        );

        assert!(result.ready);
        assert_eq!(result.status, ReadinessStatus::Ready);
    }

    #[test]
    fn secondary_dependency_failure_is_degraded_but_ready() {
        let result = readiness_from_checks(
            ComponentStatus::Ok,
            ComponentStatus::Down,
            ComponentStatus::Down,
        );

        assert!(result.ready);
        assert_eq!(result.status, ReadinessStatus::Degraded);
    }

    #[test]
    fn primary_dependency_failure_is_unavailable() {
        let result = readiness_from_checks(
            ComponentStatus::Down,
            ComponentStatus::Ok,
            ComponentStatus::Ok,
        );

        assert!(!result.ready);
        assert_eq!(result.status, ReadinessStatus::Unavailable);
    }
}
