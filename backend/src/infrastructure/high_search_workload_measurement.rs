use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    application::crypto_search_orchestration::HighSearchTextAnalyzer,
    domain::memo::entity::Memo,
    error::{AppError, AppResult},
    infrastructure::crypto_search_analyzer::{
        IcuHighSearchTextAnalyzer, ICU_HIGH_SEARCH_ANALYSIS_VERSION,
    },
};

pub const MAX_HIGH_SEARCH_WORKLOAD_FILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_HIGH_SEARCH_WORKLOAD_DOCUMENTS: usize = 10_000;
const MAX_HIGH_SEARCH_WORKLOAD_QUERIES: usize = 10_000;

#[derive(Debug, Deserialize)]
struct WorkloadCorpus {
    documents: Vec<WorkloadDocument>,
    queries: Vec<WorkloadQuery>,
}

#[derive(Debug, Deserialize)]
struct WorkloadDocument {
    title: String,
    content: String,
    tags: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct WorkloadQuery {
    query: String,
    tag: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DistributionSummary {
    pub samples: usize,
    pub min: usize,
    pub p50: usize,
    pub p95: usize,
    pub max: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HighSearchWorkloadReport {
    pub analysis_version: String,
    pub documents: usize,
    pub queries: usize,
    pub documents_with_zero_content_terms: usize,
    pub queries_with_zero_content_terms: usize,
    pub queries_with_tag: usize,
    pub document_content_terms: DistributionSummary,
    pub document_tag_terms: DistributionSummary,
    pub query_content_terms: DistributionSummary,
    pub normalized_term_bytes: DistributionSummary,
}

/// Measure one local sanitized/generated HIGH-search workload without emitting
/// plaintext terms, document identifiers, query strings, or blind tokens.
///
/// The input is parsed entirely in-process and never sent to KMS or Manticore.
/// Callers are responsible for ensuring the source corpus is approved for local
/// processing and is not persisted into repository artifacts or logs.
pub fn measure_high_search_workload_json(input: &str) -> AppResult<HighSearchWorkloadReport> {
    let corpus: WorkloadCorpus = serde_json::from_str(input).map_err(|error| {
        AppError::ValidationError(format!(
            "HIGH search workload corpus must be valid JSON: {error}"
        ))
    })?;

    validate_corpus_bounds(&corpus)?;

    let analyzer = IcuHighSearchTextAnalyzer::new();
    let mut document_content_terms = Vec::with_capacity(corpus.documents.len());
    let mut document_tag_terms = Vec::with_capacity(corpus.documents.len());
    let mut query_content_terms = Vec::with_capacity(corpus.queries.len());
    let mut normalized_term_bytes = Vec::new();
    let mut documents_with_zero_content_terms = 0_usize;
    let mut queries_with_zero_content_terms = 0_usize;
    let mut queries_with_tag = 0_usize;

    for document in &corpus.documents {
        let memo = Memo::new(
            document.title.clone(),
            document.content.clone(),
            document.tags.clone(),
            Uuid::nil(),
        );
        let analyzed = analyzer.analyze_document(&memo)?;

        let content_count = analyzed.content_terms.len();
        if content_count == 0 {
            documents_with_zero_content_terms += 1;
        }
        document_content_terms.push(content_count);
        document_tag_terms.push(analyzed.tag_terms.len());
        normalized_term_bytes.extend(
            analyzed
                .content_terms
                .iter()
                .chain(&analyzed.tag_terms)
                .map(|term| term.len()),
        );
    }

    for query in &corpus.queries {
        let analyzed = analyzer.analyze_query(&query.query, query.tag.as_deref())?;
        let content_count = analyzed.content_terms.len();
        if content_count == 0 {
            queries_with_zero_content_terms += 1;
        }
        if analyzed.tag_term.is_some() {
            queries_with_tag += 1;
        }
        query_content_terms.push(content_count);
        normalized_term_bytes.extend(
            analyzed
                .content_terms
                .iter()
                .chain(analyzed.tag_term.iter())
                .map(|term| term.len()),
        );
    }

    Ok(HighSearchWorkloadReport {
        analysis_version: ICU_HIGH_SEARCH_ANALYSIS_VERSION.into(),
        documents: corpus.documents.len(),
        queries: corpus.queries.len(),
        documents_with_zero_content_terms,
        queries_with_zero_content_terms,
        queries_with_tag,
        document_content_terms: summarize_distribution(document_content_terms),
        document_tag_terms: summarize_distribution(document_tag_terms),
        query_content_terms: summarize_distribution(query_content_terms),
        normalized_term_bytes: summarize_distribution(normalized_term_bytes),
    })
}

fn validate_corpus_bounds(corpus: &WorkloadCorpus) -> AppResult<()> {
    if corpus.documents.is_empty() || corpus.queries.is_empty() {
        return Err(AppError::ValidationError(
            "HIGH search workload corpus requires at least one document and one query".into(),
        ));
    }
    if corpus.documents.len() > MAX_HIGH_SEARCH_WORKLOAD_DOCUMENTS {
        return Err(AppError::ValidationError(format!(
            "HIGH search workload corpus exceeds {MAX_HIGH_SEARCH_WORKLOAD_DOCUMENTS} documents"
        )));
    }
    if corpus.queries.len() > MAX_HIGH_SEARCH_WORKLOAD_QUERIES {
        return Err(AppError::ValidationError(format!(
            "HIGH search workload corpus exceeds {MAX_HIGH_SEARCH_WORKLOAD_QUERIES} queries"
        )));
    }
    Ok(())
}

fn summarize_distribution(mut samples: Vec<usize>) -> DistributionSummary {
    if samples.is_empty() {
        return DistributionSummary {
            samples: 0,
            min: 0,
            p50: 0,
            p95: 0,
            max: 0,
        };
    }

    samples.sort_unstable();
    DistributionSummary {
        samples: samples.len(),
        min: samples[0],
        p50: nearest_rank(&samples, 50),
        p95: nearest_rank(&samples, 95),
        max: *samples.last().expect("non-empty distribution"),
    }
}

fn nearest_rank(sorted: &[usize], percentile: usize) -> usize {
    debug_assert!(!sorted.is_empty());
    debug_assert!((1..=100).contains(&percentile));
    let rank = sorted.len().saturating_mul(percentile).div_ceil(100).max(1);
    sorted[rank - 1]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measurement_emits_only_aggregate_workload_properties() {
        let report = measure_high_search_workload_json(
            r#"{
                "documents": [
                    {
                        "title": "Snow Memo",
                        "content": "Private winter notes",
                        "tags": ["Travel"]
                    },
                    {
                        "title": "こんにちは世界",
                        "content": "雪 メモ",
                        "tags": ["日本語"]
                    }
                ],
                "queries": [
                    {"query": "snow memo", "tag": null},
                    {"query": "こんにちは世界", "tag": "日本語"}
                ]
            }"#,
        )
        .unwrap();

        assert_eq!(report.analysis_version, ICU_HIGH_SEARCH_ANALYSIS_VERSION);
        assert_eq!(report.documents, 2);
        assert_eq!(report.queries, 2);
        assert_eq!(report.queries_with_tag, 1);
        assert_eq!(report.document_content_terms.samples, 2);
        assert_eq!(report.query_content_terms.samples, 2);
        assert!(report.normalized_term_bytes.max >= "こんにちは".len());

        let serialized = serde_json::to_string(&report).unwrap();
        for plaintext in [
            "Snow Memo",
            "Private winter notes",
            "こんにちは世界",
            "日本語",
        ] {
            assert!(!serialized.contains(plaintext));
        }
    }

    #[test]
    fn measurement_rejects_invalid_or_empty_corpus_without_echoing_plaintext() {
        let secret = "do-not-echo-secret";
        let error = measure_high_search_workload_json(&format!(
            r#"{{"documents":[],"queries":[],"extra":"{secret}"}}"#
        ))
        .unwrap_err();

        assert!(!error.to_string().contains(secret));
        assert!(measure_high_search_workload_json("not-json").is_err());
    }

    #[test]
    fn distribution_uses_deterministic_nearest_rank_percentiles() {
        assert_eq!(
            summarize_distribution(vec![1, 2, 3, 4, 100]),
            DistributionSummary {
                samples: 5,
                min: 1,
                p50: 3,
                p95: 100,
                max: 100,
            }
        );
        assert_eq!(
            summarize_distribution(Vec::new()),
            DistributionSummary {
                samples: 0,
                min: 0,
                p50: 0,
                p95: 0,
                max: 0,
            }
        );
    }
}
