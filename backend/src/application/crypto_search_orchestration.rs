use std::{collections::BTreeSet, sync::Arc};

use uuid::Uuid;

use crate::{
    application::{
        crypto_search::{
            validate_normalized_search_term, HighSearchToken, HighSearchTokenCryptography,
        },
        crypto_search_projection::{
            HighMemoSearchProjection, HighSearchProjectionDocument, HighSearchProjectionPage,
            HighSearchProjectionQuery,
        },
    },
    domain::memo::entity::Memo,
    error::{AppError, AppResult},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HighSearchAnalyzedDocument {
    pub content_terms: Vec<String>,
    pub tag_terms: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HighSearchAnalyzedQuery {
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

    pub async fn replace_memo(&self, memo: &Memo) -> AppResult<()> {
        if !memo.validate() {
            return Err(AppError::ValidationError(
                "Memo violates domain invariants before HIGH search projection".into(),
            ));
        }

        let analyzed = self.analyzer.analyze_document(memo)?;
        let content_terms = canonicalize_terms(analyzed.content_terms)?;
        let tag_terms = canonicalize_terms(analyzed.tag_terms)?;

        if content_terms.is_empty() {
            return Err(AppError::ValidationError(
                "HIGH search analyzer produced no searchable content terms".into(),
            ));
        }

        let mut key_version = None;
        let content_tokens = self
            .derive_terms(memo.user_id, &content_terms, &mut key_version)
            .await?;
        let tag_tokens = self
            .derive_terms(memo.user_id, &tag_terms, &mut key_version)
            .await?;
        let search_key_version = key_version.ok_or_else(|| {
            AppError::InternalServerError(
                "HIGH search projection produced no search-key version".into(),
            )
        })?;

        let document = HighSearchProjectionDocument {
            memo_id: memo.id,
            owner_partition: memo.user_id,
            version: memo.version,
            search_key_version,
            content_tokens,
            tag_tokens,
        };
        document.validate()?;
        self.projection.replace_document(&document).await
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
        let content_terms = canonicalize_terms(analyzed.content_terms)?;
        let tag_term = analyzed
            .tag_term
            .map(validate_single_term)
            .transpose()?;

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

        let mut key_version = None;
        let content_tokens = self
            .derive_terms(owner_partition, &content_terms, &mut key_version)
            .await?;
        let tag_token = match tag_term {
            Some(term) => {
                let tokens = self
                    .derive_terms(owner_partition, &[term], &mut key_version)
                    .await?;
                tokens.into_iter().next()
            }
            None => None,
        };

        let projection_query = HighSearchProjectionQuery {
            content_tokens,
            tag_token,
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
        operation_key_version: &mut Option<String>,
    ) -> AppResult<Vec<HighSearchToken>> {
        let mut tokens = Vec::with_capacity(terms.len());

        for term in terms {
            let token = self
                .cryptography
                .derive_token(owner_partition, term)
                .await?;
            token.validate()?;

            if let Some(expected) = operation_key_version.as_deref() {
                if expected != token.key_version.as_str() {
                    return Err(AppError::ServiceUnavailable(
                        "HIGH search key version changed during one tokenization operation; retry"
                            .into(),
                    ));
                }
            } else {
                *operation_key_version = Some(token.key_version.clone());
            }

            tokens.push(token);
        }

        Ok(tokens)
    }
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
    use crate::application::crypto_search::{
        SEARCH_HIGH_SUITE_ID, SEARCH_HIGH_TOKEN_BYTES,
    };

    struct FakeAnalyzer;

    impl HighSearchTextAnalyzer for FakeAnalyzer {
        fn analyze_document(&self, _memo: &Memo) -> AppResult<HighSearchAnalyzedDocument> {
            Ok(HighSearchAnalyzedDocument {
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
            self.calls
                .lock()
                .unwrap()
                .push(normalized_term.to_string());

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
        async fn replace_document(
            &self,
            document: &HighSearchProjectionDocument,
        ) -> AppResult<()> {
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

        async fn delete_document(
            &self,
            owner_partition: Uuid,
            memo_id: Uuid,
        ) -> AppResult<()> {
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
        let service =
            HighSearchProjectionService::new(Arc::new(FakeAnalyzer), crypto.clone(), projection.clone());
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
        assert!(query.search_key_version.is_none());
        assert_eq!((page, limit), (2, 25));
    }

    #[tokio::test]
    async fn tokenized_query_deduplicates_and_uses_one_key_version() {
        let crypto = Arc::new(FakeCrypto::stable());
        let projection = Arc::new(FakeProjection::default());
        let service =
            HighSearchProjectionService::new(Arc::new(FakeAnalyzer), crypto.clone(), projection.clone());
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
