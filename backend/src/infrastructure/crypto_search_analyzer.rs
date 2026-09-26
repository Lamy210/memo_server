// Versioned production-candidate analyzer for SEARCH-HIGH-1.
// Plaintext terms exist only in process memory before blind-token derivation.
#![allow(dead_code)]

use std::collections::BTreeSet;

use icu_casemap::CaseMapper;
use icu_normalizer::ComposingNormalizer;
use icu_segmenter::{options::WordBreakInvariantOptions, WordSegmenter};

use crate::{
    application::crypto_search_orchestration::{
        HighSearchAnalyzedDocument, HighSearchAnalyzedQuery, HighSearchTextAnalyzer,
    },
    domain::memo::entity::Memo,
    error::{AppError, AppResult},
};

pub(crate) const ICU_HIGH_SEARCH_ANALYSIS_VERSION: &str = "icu4x-2.3.0-uax29-17-nfkc-fold-dict-v1";

#[derive(Debug, Default)]
pub(crate) struct IcuHighSearchTextAnalyzer;

impl IcuHighSearchTextAnalyzer {
    pub(crate) fn new() -> Self {
        Self
    }

    fn normalize(input: &str) -> AppResult<String> {
        if input.contains('\0') {
            return Err(AppError::ValidationError(
                "HIGH search analyzer input must not contain NUL".into(),
            ));
        }

        let nfkc = ComposingNormalizer::new_nfkc();
        let normalized = nfkc.normalize(input);
        let folded = CaseMapper::new().fold_string(normalized.as_ref());
        let normalized_folded = nfkc.normalize(folded.as_ref());

        Ok(normalized_folded.trim().to_string())
    }

    fn segment_words(input: &str) -> AppResult<Vec<String>> {
        let normalized = Self::normalize(input)?;
        if normalized.is_empty() {
            return Ok(Vec::new());
        }

        let segmenter = WordSegmenter::new_dictionary(WordBreakInvariantOptions::default());
        let mut boundaries = segmenter.segment_str(&normalized).iter_with_word_type();
        let Some((mut start, _)) = boundaries.next() else {
            return Ok(Vec::new());
        };

        let mut terms = BTreeSet::new();
        for (end, word_type) in boundaries {
            if word_type.is_word_like() {
                let term = &normalized[start..end];
                if !term.is_empty() {
                    terms.insert(term.to_string());
                }
            }
            start = end;
        }

        Ok(terms.into_iter().collect())
    }

    fn normalize_exact_term(input: &str) -> AppResult<Option<String>> {
        let normalized = Self::normalize(input)?;
        Ok((!normalized.is_empty()).then_some(normalized))
    }
}

impl HighSearchTextAnalyzer for IcuHighSearchTextAnalyzer {
    fn analyze_document(&self, memo: &Memo) -> AppResult<HighSearchAnalyzedDocument> {
        if !memo.validate() {
            return Err(AppError::ValidationError(
                "Memo violates domain invariants before HIGH search analysis".into(),
            ));
        }

        let mut content_terms = Self::segment_words(&memo.title)?;
        content_terms.extend(Self::segment_words(&memo.content)?);
        content_terms.sort();
        content_terms.dedup();

        let mut tag_terms = Vec::with_capacity(memo.tags.len());
        for tag in &memo.tags {
            let normalized = Self::normalize_exact_term(tag)?.ok_or_else(|| {
                AppError::ValidationError("HIGH search tag became empty after normalization".into())
            })?;
            tag_terms.push(normalized);
        }

        Ok(HighSearchAnalyzedDocument {
            analysis_version: ICU_HIGH_SEARCH_ANALYSIS_VERSION.into(),
            content_terms,
            tag_terms,
        })
    }

    fn analyze_query(&self, query: &str, tag: Option<&str>) -> AppResult<HighSearchAnalyzedQuery> {
        let content_terms = Self::segment_words(query)?;
        let tag_term = tag.map(Self::normalize_exact_term).transpose()?.flatten();

        Ok(HighSearchAnalyzedQuery {
            analysis_version: ICU_HIGH_SEARCH_ANALYSIS_VERSION.into(),
            content_terms,
            tag_term,
        })
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use serde::Deserialize;
    use uuid::Uuid;

    use super::*;

    #[derive(Debug, Deserialize)]
    struct AnalyzerCorpus {
        analysis_version: String,
        cases: Vec<AnalyzerCorpusCase>,
    }

    #[derive(Debug, Deserialize)]
    struct AnalyzerCorpusCase {
        name: String,
        query: String,
        tag: Option<String>,
        expected_content_terms: Vec<String>,
        expected_tag_term: Option<String>,
    }

    #[derive(Debug, Deserialize)]
    struct RepresentativeCorpus {
        analysis_version: String,
        documents: Vec<RepresentativeDocument>,
        queries: Vec<RepresentativeQuery>,
        expected_metrics: RepresentativeMetrics,
    }

    #[derive(Debug, Deserialize)]
    struct RepresentativeDocument {
        id: String,
        title: String,
        content: String,
        tags: Vec<String>,
    }

    #[derive(Debug, Deserialize)]
    struct RepresentativeQuery {
        name: String,
        query: String,
        tag: Option<String>,
        expected_document_ids: Vec<String>,
    }

    #[derive(Debug, Deserialize, PartialEq, Eq)]
    struct RepresentativeMetrics {
        document_content_term_counts_sorted: Vec<usize>,
        query_content_term_counts_sorted: Vec<usize>,
        max_normalized_term_bytes: usize,
    }

    fn memo(title: &str, content: &str, tags: Vec<&str>) -> Memo {
        Memo {
            id: Uuid::from_u128(7),
            title: title.into(),
            content: content.into(),
            tags: tags.into_iter().map(str::to_string).collect(),
            user_id: Uuid::from_u128(8),
            created_at: Utc.timestamp_millis_opt(1_700_000_000_000).unwrap(),
            updated_at: Utc.timestamp_millis_opt(1_700_000_001_000).unwrap(),
            version: 3,
        }
    }

    #[test]
    fn synthetic_conformance_corpus_matches_versioned_analyzer_contract() {
        let corpus: AnalyzerCorpus = serde_json::from_str(include_str!(
            "../../testdata/high_search_analysis_corpus_v1.json"
        ))
        .unwrap();
        let analyzer = IcuHighSearchTextAnalyzer::new();

        assert_eq!(corpus.analysis_version, ICU_HIGH_SEARCH_ANALYSIS_VERSION);

        for case in corpus.cases {
            let analyzed = analyzer
                .analyze_query(&case.query, case.tag.as_deref())
                .unwrap_or_else(|error| panic!("corpus case {} failed: {error}", case.name));

            assert_eq!(
                analyzed.analysis_version, corpus.analysis_version,
                "analysis version mismatch for corpus case {}",
                case.name
            );
            assert_eq!(
                analyzed.content_terms, case.expected_content_terms,
                "content terms mismatch for corpus case {}",
                case.name
            );
            assert_eq!(
                analyzed.tag_term, case.expected_tag_term,
                "tag term mismatch for corpus case {}",
                case.name
            );
        }
    }

    #[test]
    fn representative_workload_preserves_index_query_compatibility_and_budgets() {
        let corpus: RepresentativeCorpus = serde_json::from_str(include_str!(
            "../../testdata/high_search_representative_corpus_v1.json"
        ))
        .unwrap();
        let analyzer = IcuHighSearchTextAnalyzer::new();

        assert_eq!(corpus.analysis_version, ICU_HIGH_SEARCH_ANALYSIS_VERSION);

        let mut analyzed_documents = Vec::with_capacity(corpus.documents.len());
        let mut document_content_term_counts = Vec::with_capacity(corpus.documents.len());
        let mut query_content_term_counts = Vec::with_capacity(corpus.queries.len());
        let mut max_normalized_term_bytes = 0_usize;

        for (index, document) in corpus.documents.iter().enumerate() {
            let memo = Memo {
                id: Uuid::from_u128(index as u128 + 1),
                title: document.title.clone(),
                content: document.content.clone(),
                tags: document.tags.clone(),
                user_id: Uuid::from_u128(100),
                created_at: Utc.timestamp_millis_opt(1_700_000_000_000).unwrap(),
                updated_at: Utc.timestamp_millis_opt(1_700_000_001_000).unwrap(),
                version: 1,
            };
            let analyzed = analyzer
                .analyze_document(&memo)
                .unwrap_or_else(|error| panic!("representative document {} failed: {error}", document.id));

            assert_eq!(analyzed.analysis_version, corpus.analysis_version);
            document_content_term_counts.push(analyzed.content_terms.len());
            max_normalized_term_bytes = max_normalized_term_bytes.max(
                analyzed
                    .content_terms
                    .iter()
                    .chain(&analyzed.tag_terms)
                    .map(|term| term.len())
                    .max()
                    .unwrap_or(0),
            );
            analyzed_documents.push((document.id.as_str(), analyzed));
        }

        for query in &corpus.queries {
            let analyzed = analyzer
                .analyze_query(&query.query, query.tag.as_deref())
                .unwrap_or_else(|error| panic!("representative query {} failed: {error}", query.name));

            assert_eq!(analyzed.analysis_version, corpus.analysis_version);
            query_content_term_counts.push(analyzed.content_terms.len());
            max_normalized_term_bytes = max_normalized_term_bytes.max(
                analyzed
                    .content_terms
                    .iter()
                    .chain(analyzed.tag_term.iter())
                    .map(|term| term.len())
                    .max()
                    .unwrap_or(0),
            );

            let mut matched = analyzed_documents
                .iter()
                .filter_map(|(document_id, document)| {
                    let content_matches = analyzed
                        .content_terms
                        .iter()
                        .all(|term| document.content_terms.binary_search(term).is_ok());
                    let tag_matches = analyzed
                        .tag_term
                        .as_ref()
                        .is_none_or(|tag| document.tag_terms.iter().any(|candidate| candidate == tag));
                    (content_matches && tag_matches).then_some((*document_id).to_string())
                })
                .collect::<Vec<_>>();
            matched.sort();

            let mut expected = query.expected_document_ids.clone();
            expected.sort();
            assert_eq!(
                matched, expected,
                "representative HIGH semantics mismatch for query {}",
                query.name
            );
        }

        document_content_term_counts.sort_unstable();
        query_content_term_counts.sort_unstable();

        assert_eq!(
            RepresentativeMetrics {
                document_content_term_counts_sorted: document_content_term_counts,
                query_content_term_counts_sorted: query_content_term_counts,
                max_normalized_term_bytes,
            },
            corpus.expected_metrics
        );
    }

    #[test]
    fn english_width_and_case_normalize_to_stable_terms() {
        let analyzed = IcuHighSearchTextAnalyzer::new()
            .analyze_query("ＨＥＬＬＯ　World", None)
            .unwrap();

        assert_eq!(analyzed.analysis_version, ICU_HIGH_SEARCH_ANALYSIS_VERSION);
        assert_eq!(
            analyzed.content_terms,
            vec!["hello".to_string(), "world".to_string()]
        );
    }

    #[test]
    fn dictionary_segmenter_handles_representative_japanese() {
        let analyzed = IcuHighSearchTextAnalyzer::new()
            .analyze_query("こんにちは世界", None)
            .unwrap();

        assert_eq!(
            analyzed.content_terms,
            vec!["こんにちは".to_string(), "世界".to_string()]
        );
    }

    #[test]
    fn document_analyzes_title_content_and_exact_tags() {
        let analyzed = IcuHighSearchTextAnalyzer::new()
            .analyze_document(&memo(
                "Snow Memo",
                "こんにちは世界",
                vec!["ＨｏｌｏＬｉｖｅ", "High Priority"],
            ))
            .unwrap();

        assert_eq!(analyzed.analysis_version, ICU_HIGH_SEARCH_ANALYSIS_VERSION);
        assert!(analyzed.content_terms.contains(&"snow".to_string()));
        assert!(analyzed.content_terms.contains(&"memo".to_string()));
        assert!(analyzed.content_terms.contains(&"こんにちは".to_string()));
        assert!(analyzed.content_terms.contains(&"世界".to_string()));
        assert_eq!(
            analyzed.tag_terms,
            vec!["hololive".to_string(), "high priority".to_string()]
        );
    }

    #[test]
    fn query_and_document_share_exact_analysis_generation() {
        let analyzer = IcuHighSearchTextAnalyzer::new();
        let document = analyzer
            .analyze_document(&memo("HELLO", "こんにちは世界", vec!["Tag"]))
            .unwrap();
        let query = analyzer.analyze_query("hello", Some("ＴＡＧ")).unwrap();

        assert_eq!(document.analysis_version, query.analysis_version);
        assert_eq!(query.content_terms, vec!["hello".to_string()]);
        assert_eq!(query.tag_term.as_deref(), Some("tag"));
    }

    #[test]
    fn analyzer_deduplicates_repeated_plaintext_terms_before_crypto_boundary() {
        let analyzed = IcuHighSearchTextAnalyzer::new()
            .analyze_document(&memo("Snow Snow", "snow snow snow", vec!["Tag"]))
            .unwrap();

        assert_eq!(analyzed.content_terms, vec!["snow".to_string()]);
    }

    #[test]
    fn punctuation_only_query_has_no_searchable_words() {
        let analyzed = IcuHighSearchTextAnalyzer::new()
            .analyze_query("...！？", None)
            .unwrap();

        assert!(analyzed.content_terms.is_empty());
    }

    #[test]
    fn analyzer_rejects_nul_before_tokenization() {
        assert!(matches!(
            IcuHighSearchTextAnalyzer::new().analyze_query("snow\0memo", None),
            Err(AppError::ValidationError(_))
        ));
    }
}
