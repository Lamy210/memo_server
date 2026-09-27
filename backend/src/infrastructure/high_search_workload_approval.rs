use serde::{Deserialize, Serialize};

use crate::{
    config::HighSearchConfig,
    error::{AppError, AppResult},
    infrastructure::{
        crypto_search_analyzer::ICU_HIGH_SEARCH_ANALYSIS_VERSION,
        high_search_workload_measurement::{DistributionSummary, HighSearchWorkloadReport},
    },
};

pub const HIGH_SEARCH_WORKLOAD_APPROVAL_SCHEMA_V1: &str = "high-search-workload-approval-v1";
pub const MAX_HIGH_SEARCH_WORKLOAD_APPROVAL_FILE_BYTES: u64 = 256 * 1024;
const MAX_APPROVAL_ID_BYTES: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HighSearchWorkloadSource {
    SanitizedProductionLike,
    GeneratedProductionLike,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HighSearchApprovedBudgets {
    pub max_document_content_terms: usize,
    pub max_query_content_terms: usize,
    pub max_normalized_term_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HighSearchWorkloadApproval {
    pub schema_version: String,
    pub approval_id: String,
    pub workload_source: HighSearchWorkloadSource,
    pub production_like_corpus_reviewed: bool,
    pub projection_load_test_completed: bool,
    pub measurement: HighSearchWorkloadReport,
    pub selected_budgets: HighSearchApprovedBudgets,
}

impl HighSearchWorkloadApproval {
    pub fn validate(&self) -> AppResult<()> {
        if self.schema_version != HIGH_SEARCH_WORKLOAD_APPROVAL_SCHEMA_V1 {
            return Err(AppError::ValidationError(format!(
                "unsupported HIGH search workload approval schema: {}",
                self.schema_version
            )));
        }
        validate_approval_id(&self.approval_id)?;

        if !self.production_like_corpus_reviewed {
            return Err(AppError::ValidationError(
                "HIGH search workload approval requires production-like corpus review".into(),
            ));
        }
        if !self.projection_load_test_completed {
            return Err(AppError::ValidationError(
                "HIGH search workload approval requires projection load testing".into(),
            ));
        }
        if self.measurement.analysis_version != ICU_HIGH_SEARCH_ANALYSIS_VERSION {
            return Err(AppError::ValidationError(format!(
                "HIGH search workload approval analysis_version mismatch: expected {}",
                ICU_HIGH_SEARCH_ANALYSIS_VERSION
            )));
        }

        validate_measurement(&self.measurement)?;
        validate_selected_budgets(&self.measurement, self.selected_budgets)
    }

    /// Require the reviewed deployment budgets to match the runtime values
    /// exactly. This is intentionally strict so a future protected-query
    /// cutover cannot silently run with values different from the reviewed
    /// workload approval.
    pub fn validate_against_config(&self, config: &HighSearchConfig) -> AppResult<()> {
        self.validate()?;

        let HighSearchConfig::AwsKms {
            max_document_content_terms,
            max_query_content_terms,
            max_normalized_term_bytes,
            ..
        } = config
        else {
            return Err(AppError::ValidationError(
                "HIGH search workload approval requires HIGH_SEARCH_MODE=aws-kms".into(),
            ));
        };

        let configured = HighSearchApprovedBudgets {
            max_document_content_terms: *max_document_content_terms,
            max_query_content_terms: *max_query_content_terms,
            max_normalized_term_bytes: *max_normalized_term_bytes,
        };
        if configured != self.selected_budgets {
            return Err(AppError::ValidationError(
                "HIGH search runtime budgets differ from the reviewed workload approval".into(),
            ));
        }

        Ok(())
    }
}

pub fn parse_high_search_workload_approval_json(
    input: &str,
) -> AppResult<HighSearchWorkloadApproval> {
    let approval: HighSearchWorkloadApproval = serde_json::from_str(input).map_err(|error| {
        AppError::ValidationError(format!(
            "HIGH search workload approval must be valid JSON: {error}"
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
            "HIGH search workload approval_id must be 1..=128 safe identifier bytes".into(),
        ))
    }
}

fn validate_measurement(report: &HighSearchWorkloadReport) -> AppResult<()> {
    if report.documents == 0 || report.queries == 0 {
        return Err(AppError::ValidationError(
            "HIGH search workload approval requires non-empty document and query measurements"
                .into(),
        ));
    }
    if report.documents_with_zero_content_terms > report.documents
        || report.queries_with_zero_content_terms > report.queries
        || report.queries_with_tag > report.queries
    {
        return Err(AppError::ValidationError(
            "HIGH search workload report aggregate counts are inconsistent".into(),
        ));
    }

    validate_distribution(
        "document_content_terms",
        &report.document_content_terms,
        Some(report.documents),
    )?;
    validate_distribution(
        "document_tag_terms",
        &report.document_tag_terms,
        Some(report.documents),
    )?;
    validate_distribution(
        "query_content_terms",
        &report.query_content_terms,
        Some(report.queries),
    )?;
    validate_distribution("normalized_term_bytes", &report.normalized_term_bytes, None)?;

    if report.normalized_term_bytes.samples == 0 {
        return Err(AppError::ValidationError(
            "HIGH search workload approval requires at least one normalized term sample".into(),
        ));
    }

    Ok(())
}

fn validate_distribution(
    name: &str,
    summary: &DistributionSummary,
    expected_samples: Option<usize>,
) -> AppResult<()> {
    if let Some(expected) = expected_samples {
        if summary.samples != expected {
            return Err(AppError::ValidationError(format!(
                "HIGH search workload report {name} sample count mismatch"
            )));
        }
    }

    if summary.samples == 0 {
        if summary.min == 0 && summary.p50 == 0 && summary.p95 == 0 && summary.max == 0 {
            return Ok(());
        }
        return Err(AppError::ValidationError(format!(
            "HIGH search workload report {name} has values without samples"
        )));
    }

    if !(summary.min <= summary.p50 && summary.p50 <= summary.p95 && summary.p95 <= summary.max) {
        return Err(AppError::ValidationError(format!(
            "HIGH search workload report {name} distribution is not monotonic"
        )));
    }

    Ok(())
}

fn validate_selected_budgets(
    report: &HighSearchWorkloadReport,
    budgets: HighSearchApprovedBudgets,
) -> AppResult<()> {
    if budgets.max_document_content_terms == 0
        || budgets.max_query_content_terms == 0
        || budgets.max_normalized_term_bytes == 0
    {
        return Err(AppError::ValidationError(
            "HIGH search approved budgets must be positive".into(),
        ));
    }

    if budgets.max_document_content_terms < report.document_content_terms.max {
        return Err(AppError::ValidationError(
            "HIGH search approved document-term budget is below the measured maximum".into(),
        ));
    }
    if budgets.max_query_content_terms < report.query_content_terms.max {
        return Err(AppError::ValidationError(
            "HIGH search approved query-term budget is below the measured maximum".into(),
        ));
    }
    if budgets.max_normalized_term_bytes < report.normalized_term_bytes.max {
        return Err(AppError::ValidationError(
            "HIGH search approved normalized-term byte budget is below the measured maximum".into(),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> HighSearchWorkloadReport {
        HighSearchWorkloadReport {
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
        }
    }

    fn approval() -> HighSearchWorkloadApproval {
        HighSearchWorkloadApproval {
            schema_version: HIGH_SEARCH_WORKLOAD_APPROVAL_SCHEMA_V1.into(),
            approval_id: "prod-like-2026-09-27".into(),
            workload_source: HighSearchWorkloadSource::SanitizedProductionLike,
            production_like_corpus_reviewed: true,
            projection_load_test_completed: true,
            measurement: report(),
            selected_budgets: HighSearchApprovedBudgets {
                max_document_content_terms: 128,
                max_query_content_terms: 32,
                max_normalized_term_bytes: 128,
            },
        }
    }

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

    #[test]
    fn valid_approval_matches_reviewed_runtime_budgets() {
        let approval = approval();
        approval.validate().unwrap();
        approval.validate_against_config(&config()).unwrap();
    }

    #[test]
    fn rejects_unreviewed_or_unloadtested_approval() {
        let mut value = approval();
        value.production_like_corpus_reviewed = false;
        assert!(matches!(
            value.validate(),
            Err(AppError::ValidationError(_))
        ));

        let mut value = approval();
        value.projection_load_test_completed = false;
        assert!(matches!(
            value.validate(),
            Err(AppError::ValidationError(_))
        ));
    }

    #[test]
    fn rejects_analysis_version_or_budget_drift() {
        let mut value = approval();
        value.measurement.analysis_version = "other-analysis".into();
        assert!(matches!(
            value.validate(),
            Err(AppError::ValidationError(_))
        ));

        let mut value = approval();
        value.selected_budgets.max_query_content_terms =
            value.measurement.query_content_terms.max - 1;
        assert!(matches!(
            value.validate(),
            Err(AppError::ValidationError(_))
        ));

        let mut runtime = config();
        let HighSearchConfig::AwsKms {
            max_query_content_terms,
            ..
        } = &mut runtime
        else {
            unreachable!();
        };
        *max_query_content_terms += 1;
        assert!(matches!(
            approval().validate_against_config(&runtime),
            Err(AppError::ValidationError(_))
        ));
    }

    #[test]
    fn rejects_malformed_aggregate_report() {
        let mut value = approval();
        value.measurement.document_content_terms.samples -= 1;
        assert!(matches!(
            value.validate(),
            Err(AppError::ValidationError(_))
        ));

        let mut value = approval();
        value.measurement.normalized_term_bytes.p95 =
            value.measurement.normalized_term_bytes.max + 1;
        assert!(matches!(
            value.validate(),
            Err(AppError::ValidationError(_))
        ));
    }

    #[test]
    fn json_parser_rejects_unknown_fields_and_invalid_identifiers() {
        let mut json = serde_json::to_value(approval()).unwrap();
        json.as_object_mut()
            .unwrap()
            .insert("unexpected".into(), serde_json::json!(true));
        assert!(parse_high_search_workload_approval_json(&json.to_string()).is_err());

        let mut value = approval();
        value.approval_id = "contains spaces".into();
        assert!(matches!(
            value.validate(),
            Err(AppError::ValidationError(_))
        ));
    }
}
