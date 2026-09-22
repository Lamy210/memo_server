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
    pub authoritative: ComponentStatus,
    pub cache: ComponentStatus,
    pub search: ComponentStatus,

    // Deprecated compatibility aliases. Remove only after downstream
    // monitoring has migrated to the responsibility-oriented names above.
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
    authoritative: Arc<dyn HealthProbe>,
    cache: Arc<dyn HealthProbe>,
    search: Arc<dyn HealthProbe>,
    probe_timeout: Duration,
}

impl HealthService {
    pub fn new(
        authoritative: Arc<dyn HealthProbe>,
        cache: Arc<dyn HealthProbe>,
        search: Arc<dyn HealthProbe>,
    ) -> Self {
        Self {
            authoritative,
            cache,
            search,
            probe_timeout: DEFAULT_PROBE_TIMEOUT,
        }
    }

    pub async fn readiness(&self) -> ReadinessResponse {
        let (authoritative, cache, search) = tokio::join!(
            check_with_timeout("authoritative", &self.authoritative, self.probe_timeout),
            check_with_timeout("cache", &self.cache, self.probe_timeout),
            check_with_timeout("search", &self.search, self.probe_timeout,),
        );

        readiness_from_checks(authoritative, cache, search)
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
    authoritative: ComponentStatus,
    cache: ComponentStatus,
    search: ComponentStatus,
) -> ReadinessResponse {
    let ready = authoritative == ComponentStatus::Ok;
    let status = if !ready {
        ReadinessStatus::Unavailable
    } else if cache == ComponentStatus::Ok && search == ComponentStatus::Ok {
        ReadinessStatus::Ready
    } else {
        ReadinessStatus::Degraded
    };

    ReadinessResponse {
        ready,
        status,
        checks: HealthChecks {
            authoritative,
            cache,
            search,
            scylla: authoritative,
            redis: cache,
            elasticsearch: search,
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
        assert_eq!(result.checks.authoritative, ComponentStatus::Ok);
        assert_eq!(result.checks.cache, ComponentStatus::Ok);
        assert_eq!(result.checks.search, ComponentStatus::Ok);
        assert_eq!(result.checks.scylla, result.checks.authoritative);
        assert_eq!(result.checks.redis, result.checks.cache);
        assert_eq!(result.checks.elasticsearch, result.checks.search);
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
