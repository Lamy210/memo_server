use std::{collections::BTreeSet, sync::Arc};

use uuid::Uuid;

use crate::{
    application::{
        crypto_search::{
            search_version_identifier_is_valid, validate_normalized_search_term, HighSearchToken,
            HighSearchTokenCryptography, MAX_SEARCH_VERSION_ID_CHARS,
        },
        crypto_search_projection::{
            HighMemoSearchProjection, HighSearchProjectionDocument, HighSearchProjectionMetadata,
            HighSearchProjectionPage, HighSearchProjectionQuery,
        },
    },
    domain::memo::entity::Memo,
    error::{AppError, AppResult},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HighSearchAnalyzedDocument {
    pub analysis_version: String,
    pub content_terms: Vec<String>,
    pub tag_terms: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HighSearchAnalyzedQuery {
    pub analysis_version: String,
    pub content_terms: Vec<String>,
    pub tag_term: Option<String>,
}

/// Language/search-semantics boundary for HIGH blind indexing.
///
/// Implementations own text normalization and segmentation. They must return
/// already-normalized terms and must not persist or log plaintext input.
pub trait HighSearchTextAnalyzer: Send + Sync {
    fn analyze_document(&self, memo: &Memo) -> AppResult<HighSearchAnalyzedDocument>;

    fn analyze_query(&self, query: &str, tag: Option<&str>) -> AppResult<HighSearchAnalyzedQuery>;
}

/// Application orchestration for the protected HIGH search projection.
///
/// This service is intentionally independent of Manticore, HMAC, and concrete
/// language tokenizers. It turns analyzer output into owner-scoped blind tokens,
/// rejects mixed key versions during rotation, and passes only opaque projection
/// material to the search adapter.
pub struct HighSearchProjectionService {
    analyzer: Arc<dyn HighSearchTextAnalyzer>,
    cryptography: Arc<dyn HighSearchTokenCryptography>,
    projection: Arc<dyn HighMemoSearchProjection>,
}

impl HighSearchProjectionService {
    pub fn new(
        analyzer: Arc<dyn HighSearchTextAnalyzer>,
        cryptography: Arc<dyn HighSearchTokenCryptography>,
        projection: Arc<dyn HighMemoSearchProjection>,
    ) -> Self {
        Self {
            analyzer,
            cryptography,
            projection,
        }
    }

    pub async fn replace_memo(&self, memo: &Memo) -> AppResult<HighSearchProjectionMetadata> {
        let document = self.build_projection_document(memo).await?;
        let metadata = HighSearchProjectionMetadata::from(&document);
        self.projection.replace_document(&document).await?;
        Ok(metadata)
    }

    /// Recompute the expected whitelisted projection metadata without writing.
    ///
    /// Migration verification uses this to detect source version changes,
    /// analyzer-version changes, or search-key rotation between passes.
    pub async fn expected_metadata_for_memo(
        &self,
        memo: &Memo,
    ) -> AppResult<HighSearchProjectionMetadata> {
        let document = self.build_projection_document(memo).await?;
        Ok(HighSearchProjectionMetadata::from(&document))
    }

    async fn build_projection_document(
        &self,
        memo: &Memo,
    ) -> AppResult<HighSearchProjectionDocument> {
        if !memo.validate() {
            return Err(AppError::ValidationError(
                "Memo violates domain invariants before HIGH search projection".into(),
            ));
        }

        let analyzed = self.analyzer.analyze_document(memo)?;
        let analysis_version = validate_analysis_version(analyzed.analysis_version)?;
        let content_terms = canonicalize_terms(analyzed.content_terms)?;
        let tag_terms = canonicalize_terms(analyzed.tag_terms)?;

        if content_terms.is_empty() {
            return Err(AppError::ValidationError(
                "HIGH search analyzer produced no searchable content terms".into(),
            ));
        }

        let content_len = content_terms.len();
        let mut combined_terms = content_terms;
        combined_terms.extend(tag_terms);
        let (mut combined_tokens, key_version) =
            self.derive_terms(memo.user_id, &combined_terms).await?;
        let mut tag_tokens = combined_tokens.split_off(content_len);
        let mut content_tokens = combined_tokens;
        content_tokens.sort_by(|left, right| left.value.cmp(&right.value));
        tag_tokens.sort_by(|left, right| left.value.cmp(&right.value));
        let search_key_version = key_version.ok_or_else(|| {
            AppError::InternalServerError(
                "HIGH search projection produced no search-key version".into(),
            )
        })?;

        let document = HighSearchProjectionDocument {
            memo_id: memo.id,
            owner_partition: memo.user_id,
            version: memo.version,
            analysis_version,
            search_key_version,
            content_tokens,
            tag_tokens,
        };
        document.validate()?;
        Ok(document)
    }

    pub async fn search_memo_ids(
        &self,
        owner_partition: Uuid,
        query: &str,
        tag: Option<&str>,
        page: usize,
        limit: usize,
    ) -> AppResult<HighSearchProjectionPage> {
        let analyzed = self.analyzer.analyze_query(query, tag)?;
        let analysis_version = validate_analysis_version(analyzed.analysis_version)?;
        let content_terms = canonicalize_terms(analyzed.content_terms)?;
        let tag_term = analyzed.tag_term.map(validate_single_term).transpose()?;

        if !query.is_empty() && content_terms.is_empty() {
            return Err(AppError::ValidationError(
                "HIGH search analyzer produced no terms for a non-empty query".into(),
            ));
        }
        if tag.is_some() && tag_term.is_none() {
            return Err(AppError::ValidationError(
                "HIGH search analyzer produced no term for a requested tag".into(),
            ));
        }

        let has_tag = tag_term.is_some();
        let mut combined_terms = content_terms;
        if let Some(term) = tag_term {
            combined_terms.push(term);
        }
        let (mut combined_tokens, key_version) =
            self.derive_terms(owner_partition, &combined_terms).await?;
        let tag_token = if has_tag {
            combined_tokens.pop()
        } else {
            None
        };
        let mut content_tokens = combined_tokens;
        content_tokens.sort_by(|left, right| left.value.cmp(&right.value));

        let tokenized = !content_tokens.is_empty() || tag_token.is_some();
        let projection_query = HighSearchProjectionQuery {
            content_tokens,
            tag_token,
            analysis_version: tokenized.then_some(analysis_version),
            search_key_version: key_version,
        };
        projection_query.validate()?;

        self.projection
            .search_memo_ids(owner_partition, &projection_query, page, limit)
            .await
    }

    pub async fn delete_memo(&self, owner_partition: Uuid, memo_id: Uuid) -> AppResult<()> {
        self.projection
            .delete_document(owner_partition, memo_id)
            .await
    }

    async fn derive_terms(
        &self,
        owner_partition: Uuid,
        terms: &[String],
    ) -> AppResult<(Vec<HighSearchToken>, Option<String>)> {
        let tokens = self
            .cryptography
            .derive_tokens(owner_partition, terms)
            .await?;
        if tokens.len() != terms.len() {
            return Err(AppError::InternalServerError(format!(
                "HIGH search cryptography returned {} token(s) for {} normalized term(s)",
                tokens.len(),
                terms.len()
            )));
        }

        let mut operation_key_version = None;
        for token in &tokens {
            token.validate()?;

            if let Some(expected) = operation_key_version.as_deref() {
                if expected != token.key_version.as_str() {
                    return Err(AppError::ServiceUnavailable(
                        "HIGH search key version changed during one tokenization operation; retry"
                            .into(),
                    ));
                }
            } else {
                operation_key_version = Some(token.key_version.clone());
            }
        }

        Ok((tokens, operation_key_version))
    }
}

fn validate_analysis_version(analysis_version: String) -> AppResult<String> {
    if !search_version_identifier_is_valid(&analysis_version) {
        return Err(AppError::ValidationError(format!(
            "HIGH search analysis version must be 1..={MAX_SEARCH_VERSION_ID_CHARS} ASCII identifier characters"
        )));
    }
    Ok(analysis_version)
}

fn canonicalize_terms(terms: Vec<String>) -> AppResult<Vec<String>> {
    let mut unique = BTreeSet::new();
    for term in terms {
        validate_normalized_search_term(&term)?;
        unique.insert(term);
    }
    Ok(unique.into_iter().collect())
}

fn validate_single_term(term: String) -> AppResult<String> {
    validate_normalized_search_term(&term)?;
    Ok(term)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use chrono::{TimeZone, Utc};

    use super::*;
    use crate::application::crypto_search::{SEARCH_HIGH_SUITE_ID, SEARCH_HIGH_TOKEN_BYTES};

    struct FakeAnalyzer;

    impl HighSearchTextAnalyzer for FakeAnalyzer {
        fn analyze_document(&self, _memo: &Memo) -> AppResult<HighSearchAnalyzedDocument> {
            Ok(HighSearchAnalyzedDocument {
                analysis_version: "analysis-v1".into(),
                content_terms: vec!["snow".into(), "memo".into(), "snow".into()],
                tag_terms: vec!["tag".into(), "tag".into()],
            })
        }

        fn analyze_query(
            &self,
            query: &str,
            tag: Option<&str>,
        ) -> AppResult<HighSearchAnalyzedQuery> {
            Ok(HighSearchAnalyzedQuery {
                analysis_version: "analysis-v1".into(),
                content_terms: if query.is_empty() {
                    vec![]
                } else {
                    vec!["snow".into(), "snow".into()]
                },
                tag_term: tag.map(|_| "tag".into()),
            })
        }
    }

    struct FakeCrypto {
        calls: Mutex<Vec<String>>,
        rotate_on_memo: bool,
    }

    impl FakeCrypto {
        fn stable() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                rotate_on_memo: false,
            }
        }

        fn rotating() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                rotate_on_memo: true,
            }
        }
    }

    #[async_trait]
    impl HighSearchTokenCryptography for FakeCrypto {
        async fn derive_token(
            &self,
            _owner_partition: Uuid,
            normalized_term: &str,
        ) -> AppResult<HighSearchToken> {
            self.calls.lock().unwrap().push(normalized_term.to_string());

            let pair = match normalized_term {
                "memo" => "ab",
                "snow" => "cd",
                "tag" => "ef",
                _ => "01",
            };
            Ok(HighSearchToken {
                value: pair.repeat(SEARCH_HIGH_TOKEN_BYTES),
                key_version: if self.rotate_on_memo && normalized_term == "memo" {
                    "search-v2".into()
                } else {
                    "search-v1".into()
                },
                suite_id: SEARCH_HIGH_SUITE_ID.into(),
            })
        }
    }

    #[derive(Default)]
    struct FakeProjection {
        document: Mutex<Option<HighSearchProjectionDocument>>,
        query: Mutex<Option<(Uuid, HighSearchProjectionQuery, usize, usize)>>,
        deleted: Mutex<Option<(Uuid, Uuid)>>,
    }

    #[async_trait]
    impl HighMemoSearchProjection for FakeProjection {
        async fn replace_document(&self, document: &HighSearchProjectionDocument) -> AppResult<()> {
            *self.document.lock().unwrap() = Some(document.clone());
            Ok(())
        }

        async fn search_memo_ids(
            &self,
            owner_partition: Uuid,
            query: &HighSearchProjectionQuery,
            page: usize,
            limit: usize,
        ) -> AppResult<HighSearchProjectionPage> {
            *self.query.lock().unwrap() = Some((owner_partition, query.clone(), page, limit));
            Ok(HighSearchProjectionPage {
                memo_ids: vec![Uuid::from_u128(99)],
                total: 1,
            })
        }

        async fn delete_document(&self, owner_partition: Uuid, memo_id: Uuid) -> AppResult<()> {
            *self.deleted.lock().unwrap() = Some((owner_partition, memo_id));
            Ok(())
        }
    }

    fn memo() -> Memo {
        Memo {
            id: Uuid::from_u128(7),
            title: "Snow memo".into(),
            content: "Snow content".into(),
            tags: vec!["tag".into()],
            user_id: Uuid::from_u128(8),
            created_at: Utc.timestamp_millis_opt(1_700_000_000_000).unwrap(),
            updated_at: Utc.timestamp_millis_opt(1_700_000_001_000).unwrap(),
            version: 3,
        }
    }

    #[test]
    fn analysis_version_and_normalized_terms_fail_closed() {
        assert!(validate_analysis_version("analysis-v1".into()).is_ok());
        assert!(validate_analysis_version(" ".into()).is_err());
        assert!(validate_analysis_version("analysis v1".into()).is_err());
        assert!(validate_analysis_version("analysis\0v1".into()).is_err());
        assert!(validate_analysis_version("x".repeat(MAX_SEARCH_VERSION_ID_CHARS + 1)).is_err());

        assert_eq!(
            canonicalize_terms(vec!["snow".into(), "memo".into(), "snow".into()]).unwrap(),
            vec!["memo".to_string(), "snow".to_string()]
        );
        assert!(canonicalize_terms(vec![" snow".into()]).is_err());
    }

    #[tokio::test]
    async fn projection_deduplicates_terms_and_keeps_one_key_version() {
        let crypto = Arc::new(FakeCrypto::stable());
        let projection = Arc::new(FakeProjection::default());
        let service = HighSearchProjectionService::new(
            Arc::new(FakeAnalyzer),
            crypto.clone(),
            projection.clone(),
        );

        service.replace_memo(&memo()).await.unwrap();

        let document = projection.document.lock().unwrap().clone().unwrap();
        assert_eq!(document.owner_partition, Uuid::from_u128(8));
        assert_eq!(document.memo_id, Uuid::from_u128(7));
        assert_eq!(document.version, 3);
        assert_eq!(document.analysis_version, "analysis-v1");
        assert_eq!(document.search_key_version, "search-v1");
        assert_eq!(document.content_tokens.len(), 2);
        assert_eq!(document.tag_tokens.len(), 1);
        assert_eq!(
            crypto.calls.lock().unwrap().as_slice(),
            &["memo".to_string(), "snow".to_string(), "tag".to_string()]
        );
    }

    #[tokio::test]
    async fn mixed_key_versions_fail_closed_before_projection_write() {
        let projection = Arc::new(FakeProjection::default());
        let service = HighSearchProjectionService::new(
            Arc::new(FakeAnalyzer),
            Arc::new(FakeCrypto::rotating()),
            projection.clone(),
        );

        assert!(matches!(
            service.replace_memo(&memo()).await,
            Err(AppError::ServiceUnavailable(_))
        ));
        assert!(projection.document.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn empty_query_avoids_key_resolution_and_preserves_owner_scope() {
        let crypto = Arc::new(FakeCrypto::stable());
        let projection = Arc::new(FakeProjection::default());
        let service = HighSearchProjectionService::new(
            Arc::new(FakeAnalyzer),
            crypto.clone(),
            projection.clone(),
        );
        let owner = Uuid::from_u128(42);

        let page = service
            .search_memo_ids(owner, "", None, 2, 25)
            .await
            .unwrap();

        assert_eq!(page.total, 1);
        assert!(crypto.calls.lock().unwrap().is_empty());
        let (captured_owner, query, page, limit) =
            projection.query.lock().unwrap().clone().unwrap();
        assert_eq!(captured_owner, owner);
        assert!(query.content_tokens.is_empty());
        assert!(query.tag_token.is_none());
        assert!(query.analysis_version.is_none());
        assert!(query.search_key_version.is_none());
        assert_eq!((page, limit), (2, 25));
    }

    #[tokio::test]
    async fn tokenized_query_deduplicates_and_uses_one_key_version() {
        let crypto = Arc::new(FakeCrypto::stable());
        let projection = Arc::new(FakeProjection::default());
        let service = HighSearchProjectionService::new(
            Arc::new(FakeAnalyzer),
            crypto.clone(),
            projection.clone(),
        );
        let owner = Uuid::from_u128(42);

        service
            .search_memo_ids(owner, "snow", Some("tag"), 1, 20)
            .await
            .unwrap();

        assert_eq!(
            crypto.calls.lock().unwrap().as_slice(),
            &["snow".to_string(), "tag".to_string()]
        );
        let (_, query, _, _) = projection.query.lock().unwrap().clone().unwrap();
        assert_eq!(query.content_tokens.len(), 1);
        assert!(query.tag_token.is_some());
        assert_eq!(query.analysis_version.as_deref(), Some("analysis-v1"));
        assert_eq!(query.search_key_version.as_deref(), Some("search-v1"));
    }

    #[tokio::test]
    async fn delete_keeps_owner_partition_in_projection_boundary() {
        let projection = Arc::new(FakeProjection::default());
        let service = HighSearchProjectionService::new(
            Arc::new(FakeAnalyzer),
            Arc::new(FakeCrypto::stable()),
            projection.clone(),
        );
        let owner = Uuid::from_u128(11);
        let memo_id = Uuid::from_u128(12);

        service.delete_memo(owner, memo_id).await.unwrap();

        assert_eq!(*projection.deleted.lock().unwrap(), Some((owner, memo_id)));
    }
}
