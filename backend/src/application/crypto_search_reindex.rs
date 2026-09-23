use std::sync::Arc;

use uuid::Uuid;

use crate::{
    application::{
        crypto_migration_batch::{
            validate_page, validate_page_size, PlaintextMemoMigrationSource,
        },
        crypto_search_orchestration::HighSearchProjectionService,
        crypto_search_projection::HighSearchProjectionMigrationInspector,
    },
    error::{AppError, AppResult},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HighSearchReindexStats {
    pub source_count: u64,
    pub projection_count: u64,
    pub projected_visited: u64,
    pub verified_visited: u64,
}

/// Bounded-memory reindex and convergence verification for the isolated HIGH
/// search projection.
///
/// This is migration orchestration only. Runtime search cutover remains a
/// separate operator-controlled action after analyzer/key-provider readiness.
pub struct HighSearchReindexService {
    source: Arc<dyn PlaintextMemoMigrationSource>,
    projection: Arc<HighSearchProjectionService>,
    inspector: Arc<dyn HighSearchProjectionMigrationInspector>,
}

impl HighSearchReindexService {
    pub fn new(
        source: Arc<dyn PlaintextMemoMigrationSource>,
        projection: Arc<HighSearchProjectionService>,
        inspector: Arc<dyn HighSearchProjectionMigrationInspector>,
    ) -> Self {
        Self {
            source,
            projection,
            inspector,
        }
    }

    pub async fn reindex_all(&self, page_size: usize) -> AppResult<HighSearchReindexStats> {
        validate_page_size(page_size)?;

        let source_count_before = self.source.count_source_memos().await?;
        let projected_visited = self.run_projection_pass(page_size).await?;
        let source_count_after_projection = self.source.count_source_memos().await?;

        if source_count_before != source_count_after_projection {
            return Err(AppError::Conflict(format!(
                "Authoritative memo count changed during HIGH search reindex: before={source_count_before} after={source_count_after_projection}"
            )));
        }
        if projected_visited != source_count_after_projection {
            return Err(AppError::Conflict(format!(
                "HIGH search reindex visited {projected_visited} memo(s) but authoritative source contains {source_count_after_projection}"
            )));
        }

        let verified_visited = self.run_verification_pass(page_size).await?;
        let source_count_after_verification = self.source.count_source_memos().await?;

        if source_count_after_projection != source_count_after_verification {
            return Err(AppError::Conflict(format!(
                "Authoritative memo count changed during HIGH search verification: before={source_count_after_projection} after={source_count_after_verification}"
            )));
        }
        if verified_visited != source_count_after_verification {
            return Err(AppError::Conflict(format!(
                "HIGH search verification visited {verified_visited} memo(s) but authoritative source contains {source_count_after_verification}"
            )));
        }

        let projection_count = self.inspector.count_documents().await?;
        if projection_count != source_count_after_verification {
            return Err(AppError::Conflict(format!(
                "HIGH search reindex verification failed: authoritative source has {source_count_after_verification} memo(s) but protected projection has {projection_count}"
            )));
        }

        Ok(HighSearchReindexStats {
            source_count: source_count_after_verification,
            projection_count,
            projected_visited,
            verified_visited,
        })
    }

    async fn run_projection_pass(&self, page_size: usize) -> AppResult<u64> {
        let mut cursor = None;
        let mut visited = 0u64;

        loop {
            let page = self.source.page_source_memos(cursor, page_size).await?;
            if page.is_empty() {
                break;
            }
            validate_page(&page, cursor, page_size)?;

            for memo in &page {
                let metadata = self.projection.replace_memo(memo).await?;
                if !self.inspector.contains_metadata(&metadata).await? {
                    return Err(AppError::DatabaseError(format!(
                        "Protected HIGH search projection failed read-after-write verification for memo {}",
                        memo.id
                    )));
                }
                visited += 1;
            }

            cursor = page.last().map(|memo| memo.id);
        }

        Ok(visited)
    }

    async fn run_verification_pass(&self, page_size: usize) -> AppResult<u64> {
        let mut cursor = None;
        let mut visited = 0u64;

        loop {
            let page = self.source.page_source_memos(cursor, page_size).await?;
            if page.is_empty() {
                break;
            }
            validate_page(&page, cursor, page_size)?;

            for memo in &page {
                let expected = self.projection.expected_metadata_for_memo(memo).await?;
                if !self.inspector.contains_metadata(&expected).await? {
                    return Err(AppError::Conflict(format!(
                        "Protected HIGH search projection differs from authoritative memo {}",
                        memo.id
                    )));
                }
                visited += 1;
            }

            cursor = page.last().map(|memo| memo.id);
        }

        Ok(visited)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Mutex,
        },
    };

    use async_trait::async_trait;
    use chrono::{TimeZone, Utc};

    use super::*;
    use crate::{
        application::{
            crypto_search::{
                HighSearchToken, HighSearchTokenCryptography, SEARCH_HIGH_SUITE_ID,
                SEARCH_HIGH_TOKEN_BYTES,
            },
            crypto_search_orchestration::{
                HighSearchAnalyzedDocument, HighSearchAnalyzedQuery, HighSearchTextAnalyzer,
            },
            crypto_search_projection::{
                HighMemoSearchProjection, HighSearchProjectionDocument,
                HighSearchProjectionMetadata, HighSearchProjectionPage, HighSearchProjectionQuery,
            },
        },
        domain::memo::entity::Memo,
    };

    struct FakeSource {
        memos: Mutex<Vec<Memo>>,
    }

    impl FakeSource {
        fn new(mut memos: Vec<Memo>) -> Self {
            memos.sort_by_key(|memo| memo.id);
            Self {
                memos: Mutex::new(memos),
            }
        }
    }

    #[async_trait]
    impl PlaintextMemoMigrationSource for FakeSource {
        async fn count_source_memos(&self) -> AppResult<u64> {
            Ok(self.memos.lock().unwrap().len() as u64)
        }

        async fn page_source_memos(
            &self,
            after: Option<Uuid>,
            limit: usize,
        ) -> AppResult<Vec<Memo>> {
            Ok(self
                .memos
                .lock()
                .unwrap()
                .iter()
                .filter(|memo| after.is_none_or(|after| memo.id > after))
                .take(limit)
                .cloned()
                .collect())
        }
    }

    struct FakeAnalyzer;

    impl HighSearchTextAnalyzer for FakeAnalyzer {
        fn analyze_document(&self, _memo: &Memo) -> AppResult<HighSearchAnalyzedDocument> {
            Ok(HighSearchAnalyzedDocument {
                analysis_version: "analysis-v1".into(),
                content_terms: vec!["memo".into()],
                tag_terms: vec!["tag".into()],
            })
        }

        fn analyze_query(
            &self,
            _query: &str,
            _tag: Option<&str>,
        ) -> AppResult<HighSearchAnalyzedQuery> {
            unreachable!("reindex tests do not execute queries")
        }
    }

    struct FakeCrypto;

    #[async_trait]
    impl HighSearchTokenCryptography for FakeCrypto {
        async fn derive_token(
            &self,
            _owner_partition: Uuid,
            normalized_term: &str,
        ) -> AppResult<HighSearchToken> {
            let pair = if normalized_term == "tag" { "cd" } else { "ab" };
            Ok(HighSearchToken {
                value: pair.repeat(SEARCH_HIGH_TOKEN_BYTES),
                key_version: "search-v1".into(),
                suite_id: SEARCH_HIGH_SUITE_ID.into(),
            })
        }
    }

    #[derive(Default)]
    struct FakeProjection {
        metadata: Mutex<HashMap<(Uuid, Uuid), HighSearchProjectionMetadata>>,
    }

    #[async_trait]
    impl HighMemoSearchProjection for FakeProjection {
        async fn replace_document(&self, document: &HighSearchProjectionDocument) -> AppResult<()> {
            let metadata = HighSearchProjectionMetadata::from(document);
            self.metadata
                .lock()
                .unwrap()
                .insert((metadata.owner_partition, metadata.memo_id), metadata);
            Ok(())
        }

        async fn search_memo_ids(
            &self,
            _owner_partition: Uuid,
            _query: &HighSearchProjectionQuery,
            _page: usize,
            _limit: usize,
        ) -> AppResult<HighSearchProjectionPage> {
            unreachable!("reindex tests do not execute queries")
        }

        async fn delete_document(&self, owner_partition: Uuid, memo_id: Uuid) -> AppResult<()> {
            self.metadata
                .lock()
                .unwrap()
                .remove(&(owner_partition, memo_id));
            Ok(())
        }
    }

    #[async_trait]
    impl HighSearchProjectionMigrationInspector for FakeProjection {
        async fn contains_metadata(
            &self,
            metadata: &HighSearchProjectionMetadata,
        ) -> AppResult<bool> {
            Ok(self
                .metadata
                .lock()
                .unwrap()
                .get(&(metadata.owner_partition, metadata.memo_id))
                .is_some_and(|stored| stored == metadata))
        }

        async fn count_documents(&self) -> AppResult<u64> {
            Ok(self.metadata.lock().unwrap().len() as u64)
        }
    }

    fn memo(id: u128, version: i32) -> Memo {
        Memo {
            id: Uuid::from_u128(id),
            title: format!("memo-{id}"),
            content: "content".into(),
            tags: vec!["tag".into()],
            user_id: Uuid::from_u128(10_000 + id),
            created_at: Utc.timestamp_millis_opt(1_700_000_000_000).unwrap(),
            updated_at: Utc.timestamp_millis_opt(1_700_000_001_000).unwrap(),
            version,
        }
    }

    fn service(
        source: Arc<dyn PlaintextMemoMigrationSource>,
        projection: Arc<FakeProjection>,
    ) -> HighSearchReindexService {
        let projection_service = Arc::new(HighSearchProjectionService::new(
            Arc::new(FakeAnalyzer),
            Arc::new(FakeCrypto),
            projection.clone(),
        ));
        HighSearchReindexService::new(source, projection_service, projection)
    }

    #[tokio::test]
    async fn reindexes_multiple_pages_and_verifies_projection() {
        let source = Arc::new(FakeSource::new(vec![
            memo(1, 1),
            memo(2, 2),
            memo(3, 3),
            memo(4, 4),
            memo(5, 5),
        ]));
        let projection = Arc::new(FakeProjection::default());
        let service = service(source, projection);

        assert_eq!(
            service.reindex_all(2).await.unwrap(),
            HighSearchReindexStats {
                source_count: 5,
                projection_count: 5,
                projected_visited: 5,
                verified_visited: 5,
            }
        );
    }

    #[tokio::test]
    async fn rejects_stale_target_only_projection_rows() {
        let source = Arc::new(FakeSource::new(vec![memo(1, 1), memo(2, 1)]));
        let projection = Arc::new(FakeProjection::default());
        let stale = HighSearchProjectionMetadata {
            memo_id: Uuid::from_u128(99),
            owner_partition: Uuid::from_u128(10_099),
            version: 1,
            analysis_version: "analysis-v1".into(),
            search_key_version: "search-v1".into(),
        };
        projection
            .metadata
            .lock()
            .unwrap()
            .insert((stale.owner_partition, stale.memo_id), stale);

        let service = service(source, projection);
        assert!(matches!(
            service.reindex_all(1).await,
            Err(AppError::Conflict(_))
        ));
    }

    struct ChangingVersionSource {
        page_calls: AtomicUsize,
    }

    #[async_trait]
    impl PlaintextMemoMigrationSource for ChangingVersionSource {
        async fn count_source_memos(&self) -> AppResult<u64> {
            Ok(1)
        }

        async fn page_source_memos(
            &self,
            after: Option<Uuid>,
            limit: usize,
        ) -> AppResult<Vec<Memo>> {
            let call = self.page_calls.fetch_add(1, Ordering::Relaxed);
            let current = memo(1, if call >= 2 { 2 } else { 1 });
            Ok((limit > 0 && after.is_none()).then_some(current).into_iter().collect())
        }
    }

    #[tokio::test]
    async fn rejects_source_version_change_between_projection_and_verification() {
        let source = Arc::new(ChangingVersionSource {
            page_calls: AtomicUsize::new(0),
        });
        let projection = Arc::new(FakeProjection::default());
        let service = service(source, projection);

        assert!(matches!(
            service.reindex_all(10).await,
            Err(AppError::Conflict(_))
        ));
    }

    #[tokio::test]
    async fn rejects_invalid_page_size() {
        let source = Arc::new(FakeSource::new(vec![]));
        let projection = Arc::new(FakeProjection::default());
        let service = service(source, projection);

        assert!(matches!(
            service.reindex_all(0).await,
            Err(AppError::ValidationError(_))
        ));
    }
}
