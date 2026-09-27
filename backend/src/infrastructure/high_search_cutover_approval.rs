use serde::{Deserialize, Serialize};

use crate::{
    config::HighSearchConfig,
    error::{AppError, AppResult},
    infrastructure::high_search_workload_approval::HighSearchWorkloadApproval,
};

pub const HIGH_SEARCH_CUTOVER_APPROVAL_SCHEMA_V1: &str = "high-search-cutover-approval-v1";
pub const MAX_HIGH_SEARCH_CUTOVER_APPROVAL_FILE_BYTES: u64 = 512 * 1024;
const MAX_APPROVAL_ID_BYTES: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HighSearchCutoverRuntimeBinding {
    pub key_arn: String,
    pub region: String,
    pub provider_seed_version: String,
    pub cache_ttl_seconds: u64,
    pub cache_max_entries: usize,
    pub cache_sweep_seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HighSearchCutoverApproval {
    pub schema_version: String,
    pub approval_id: String,
    pub workload_approval: HighSearchWorkloadApproval,
    pub runtime: HighSearchCutoverRuntimeBinding,
    pub kms_key_and_iam_reviewed: bool,
    pub credential_provider_validated: bool,
    pub cache_capacity_reviewed: bool,
    pub rotation_reindex_rehearsed: bool,
    pub rollback_rehearsed: bool,
    pub security_migration_gates_reviewed: bool,
    pub all_replicas_route_aware_and_barrier_participating: bool,
    pub all_replicas_runtime_config_reviewed: bool,
}

impl HighSearchCutoverApproval {
    pub fn validate(&self) -> AppResult<()> {
        if self.schema_version != HIGH_SEARCH_CUTOVER_APPROVAL_SCHEMA_V1 {
            return Err(AppError::ValidationError(format!(
                "unsupported HIGH search cutover approval schema: {}",
                self.schema_version
            )));
        }
        validate_approval_id(&self.approval_id)?;
        self.workload_approval.validate()?;

        let required_attestations = [
            ("kms_key_and_iam_reviewed", self.kms_key_and_iam_reviewed),
            (
                "credential_provider_validated",
                self.credential_provider_validated,
            ),
            ("cache_capacity_reviewed", self.cache_capacity_reviewed),
            ("rotation_reindex_rehearsed", self.rotation_reindex_rehearsed),
            ("rollback_rehearsed", self.rollback_rehearsed),
            (
                "security_migration_gates_reviewed",
                self.security_migration_gates_reviewed,
            ),
            (
                "all_replicas_route_aware_and_barrier_participating",
                self.all_replicas_route_aware_and_barrier_participating,
            ),
            (
                "all_replicas_runtime_config_reviewed",
                self.all_replicas_runtime_config_reviewed,
            ),
        ];
        if let Some((name, _)) = required_attestations
            .into_iter()
            .find(|(_, confirmed)| !confirmed)
        {
            return Err(AppError::ValidationError(format!(
                "HIGH search cutover approval requires {name}=true"
            )));
        }

        if self.runtime.key_arn.trim().is_empty()
            || self.runtime.region.trim().is_empty()
            || self.runtime.provider_seed_version.trim().is_empty()
            || self.runtime.cache_ttl_seconds == 0
            || self.runtime.cache_max_entries == 0
            || self.runtime.cache_sweep_seconds == 0
        {
            return Err(AppError::ValidationError(
                "HIGH search cutover runtime binding must contain non-empty identifiers and positive cache settings"
                    .into(),
            ));
        }

        Ok(())
    }

    pub fn validate_against_config(&self, config: &HighSearchConfig) -> AppResult<()> {
        self.validate()?;
        self.workload_approval.validate_against_config(config)?;

        let HighSearchConfig::AwsKms {
            key_arn,
            region,
            provider_seed_version,
            cache_ttl_seconds,
            cache_max_entries,
            cache_sweep_seconds,
            ..
        } = config
        else {
            return Err(AppError::ValidationError(
                "HIGH search cutover approval requires HIGH_SEARCH_MODE=aws-kms".into(),
            ));
        };

        let configured = HighSearchCutoverRuntimeBinding {
            key_arn: key_arn.clone(),
            region: region.clone(),
            provider_seed_version: provider_seed_version.clone(),
            cache_ttl_seconds: *cache_ttl_seconds,
            cache_max_entries: *cache_max_entries,
            cache_sweep_seconds: *cache_sweep_seconds,
        };
        if configured != self.runtime {
            return Err(AppError::ValidationError(
                "HIGH search runtime key/cache configuration differs from the reviewed cutover approval"
                    .into(),
            ));
        }

        Ok(())
    }
}

pub fn parse_high_search_cutover_approval_json(input: &str) -> AppResult<HighSearchCutoverApproval> {
    let approval: HighSearchCutoverApproval = serde_json::from_str(input).map_err(|error| {
        AppError::ValidationError(format!(
            "HIGH search cutover approval must be valid JSON: {error}"
        ))
    })?;
    approval.validate()?;
    Ok(approval)
}

fn validate_approval_id(value: &str) -> AppResult<()> {
    let valid = !value.is_empty()
        && value.len() <= MAX_APPROVAL_ID_BYTES
        && value.trim() == value
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'));

    if valid {
        Ok(())
    } else {
        Err(AppError::ValidationError(
            "HIGH search cutover approval_id must be 1..=128 safe identifier bytes".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::HighSearchConfig,
        infrastructure::{
            crypto_search_analyzer::ICU_HIGH_SEARCH_ANALYSIS_VERSION,
            high_search_workload_approval::{
                HighSearchApprovedBudgets, HighSearchWorkloadSource,
                HIGH_SEARCH_WORKLOAD_APPROVAL_SCHEMA_V1,
            },
            high_search_workload_measurement::{
                DistributionSummary, HighSearchWorkloadReport,
            },
        },
    };

    fn config() -> HighSearchConfig {
        HighSearchConfig::AwsKms {
            key_arn:
                "arn:aws:kms:ap-northeast-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab"
                    .into(),
            region: "ap-northeast-1".into(),
            provider_seed_version: "search-seed-v1".into(),
            cache_ttl_seconds: 60,
            cache_max_entries: 512,
            cache_sweep_seconds: 30,
            max_document_content_terms: 128,
            max_query_content_terms: 32,
            max_normalized_term_bytes: 128,
        }
    }

    fn approval() -> HighSearchCutoverApproval {
        HighSearchCutoverApproval {
            schema_version: HIGH_SEARCH_CUTOVER_APPROVAL_SCHEMA_V1.into(),
            approval_id: "cutover-2026-09-27".into(),
            workload_approval: HighSearchWorkloadApproval {
                schema_version: HIGH_SEARCH_WORKLOAD_APPROVAL_SCHEMA_V1.into(),
                approval_id: "workload-2026-09-27".into(),
                workload_source: HighSearchWorkloadSource::SanitizedProductionLike,
                production_like_corpus_reviewed: true,
                projection_load_test_completed: true,
                measurement: HighSearchWorkloadReport {
                    analysis_version: ICU_HIGH_SEARCH_ANALYSIS_VERSION.into(),
                    documents: 100,
                    queries: 80,
                    documents_with_zero_content_terms: 0,
                    queries_with_zero_content_terms: 2,
                    queries_with_tag: 20,
                    document_content_terms: DistributionSummary {
                        samples: 100,
                        min: 1,
                        p50: 8,
                        p95: 40,
                        max: 70,
                    },
                    document_tag_terms: DistributionSummary {
                        samples: 100,
                        min: 0,
                        p50: 2,
                        p95: 5,
                        max: 8,
                    },
                    query_content_terms: DistributionSummary {
                        samples: 80,
                        min: 0,
                        p50: 2,
                        p95: 8,
                        max: 12,
                    },
                    normalized_term_bytes: DistributionSummary {
                        samples: 2_000,
                        min: 1,
                        p50: 6,
                        p95: 24,
                        max: 64,
                    },
                },
                selected_budgets: HighSearchApprovedBudgets {
                    max_document_content_terms: 128,
                    max_query_content_terms: 32,
                    max_normalized_term_bytes: 128,
                },
            },
            runtime: HighSearchCutoverRuntimeBinding {
                key_arn:
                    "arn:aws:kms:ap-northeast-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab"
                        .into(),
                region: "ap-northeast-1".into(),
                provider_seed_version: "search-seed-v1".into(),
                cache_ttl_seconds: 60,
                cache_max_entries: 512,
                cache_sweep_seconds: 30,
            },
            kms_key_and_iam_reviewed: true,
            credential_provider_validated: true,
            cache_capacity_reviewed: true,
            rotation_reindex_rehearsed: true,
            rollback_rehearsed: true,
            security_migration_gates_reviewed: true,
            all_replicas_route_aware_and_barrier_participating: true,
            all_replicas_runtime_config_reviewed: true,
        }
    }

    #[test]
    fn valid_cutover_approval_matches_runtime_and_workload_settings() {
        let approval = approval();
        approval.validate().unwrap();
        approval.validate_against_config(&config()).unwrap();
    }

    #[test]
    fn every_cutover_attestation_is_required() {
        let mut value = approval();
        value.rollback_rehearsed = false;
        assert!(matches!(
            value.validate(),
            Err(AppError::ValidationError(_))
        ));

        let mut value = approval();
        value.kms_key_and_iam_reviewed = false;
        assert!(matches!(
            value.validate(),
            Err(AppError::ValidationError(_))
        ));

        let mut value = approval();
        value.security_migration_gates_reviewed = false;
        assert!(matches!(
            value.validate(),
            Err(AppError::ValidationError(_))
        ));

        let mut value = approval();
        value.all_replicas_route_aware_and_barrier_participating = false;
        assert!(matches!(
            value.validate(),
            Err(AppError::ValidationError(_))
        ));
    }

    #[test]
    fn runtime_binding_drift_is_rejected() {
        let mut value = approval();
        value.runtime.provider_seed_version = "other-seed".into();
        assert!(matches!(
            value.validate_against_config(&config()),
            Err(AppError::ValidationError(_))
        ));

        let mut value = approval();
        value.runtime.cache_max_entries += 1;
        assert!(matches!(
            value.validate_against_config(&config()),
            Err(AppError::ValidationError(_))
        ));
    }

    #[test]
    fn nested_workload_budget_drift_is_rejected() {
        let mut value = approval();
        value.workload_approval.selected_budgets.max_query_content_terms += 1;
        assert!(matches!(
            value.validate_against_config(&config()),
            Err(AppError::ValidationError(_))
        ));
    }

    #[test]
    fn parser_rejects_unknown_fields() {
        let mut json = serde_json::to_value(approval()).unwrap();
        json.as_object_mut()
            .unwrap()
            .insert("unexpected".into(), serde_json::json!(true));
        assert!(parse_high_search_cutover_approval_json(&json.to_string()).is_err());
    }
}
