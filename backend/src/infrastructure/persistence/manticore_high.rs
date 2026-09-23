use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use tokio::sync::OnceCell;
use uuid::Uuid;

use crate::{
    application::{
        crypto_search_projection::{
            HighMemoSearchProjection, HighSearchProjectionDocument, HighSearchProjectionMetadata,
            HighSearchProjectionMigrationInspector, HighSearchProjectionPage,
            HighSearchProjectionQuery,
        },
        health::HealthProbe,
    },
    error::{AppError, AppResult},
};

const TABLE_NAME: &str = "memos_high_v1";
const CREATE_TABLE_SQL: &str = "CREATE TABLE IF NOT EXISTS memos_high_v1 (id uuid, content_tokens text indexed, tag_tokens text indexed, owner_partition string, version int, analysis_version string, search_key_version string, memo_sort_key string) dict='keywords_32k'";

pub struct HighManticoreClient {
    client: Client,
    base_url: String,
    initialized: OnceCell<()>,
}

impl HighManticoreClient {
    pub fn new(uri: &str) -> AppResult<Self> {
        let client = Client::builder().build().map_err(|error| {
            AppError::DatabaseError(format!(
                "Failed to create HIGH Manticore HTTP client: {error}"
            ))
        })?;

        Ok(Self {
            client,
            base_url: uri.trim_end_matches('/').to_string(),
            initialized: OnceCell::new(),
        })
    }

    async fn ensure_table(&self) -> AppResult<()> {
        self.initialized
            .get_or_try_init(|| async { self.initialize_table().await })
            .await
            .map(|_| ())
    }

    async fn initialize_table(&self) -> AppResult<()> {
        self.execute_raw_sql(CREATE_TABLE_SQL, "protected table initialization")
            .await
    }

    async fn execute_raw_sql(&self, sql: &str, operation: &str) -> AppResult<()> {
        let response = self
            .client
            .post(format!("{}/sql?mode=raw", self.base_url))
            .header(reqwest::header::CONTENT_TYPE, "text/plain")
            .body(sql.to_string())
            .send()
            .await
            .map_err(|error| {
                AppError::DatabaseError(format!(
                    "HIGH Manticore {operation} request failed: {error}"
                ))
            })?;

        let status = response.status();
        let result = response.json::<Value>().await.map_err(|error| {
            AppError::DatabaseError(format!(
                "Failed to parse HIGH Manticore {operation} response: {error}"
            ))
        })?;

        if !status.is_success() {
            return Err(AppError::DatabaseError(format!(
                "HIGH Manticore {operation} failed with status {status}"
            )));
        }

        let result_sets = result.as_array().ok_or_else(|| {
            AppError::DatabaseError(format!(
                "Invalid HIGH Manticore {operation} response format"
            ))
        })?;

        if let Some(error) = result_sets
            .iter()
            .filter_map(|result_set| result_set.get("error").and_then(Value::as_str))
            .find(|error| !error.is_empty())
        {
            return Err(AppError::DatabaseError(format!(
                "HIGH Manticore {operation} failed: {error}"
            )));
        }

        Ok(())
    }

    async fn post_json(&self, endpoint: &str, body: &Value) -> AppResult<Value> {
        let response = self
            .client
            .post(format!("{}/{}", self.base_url, endpoint))
            .json(body)
            .send()
            .await
            .map_err(|error| {
                AppError::DatabaseError(format!(
                    "HIGH Manticore {endpoint} request failed: {error}"
                ))
            })?;

        let status = response.status();
        if !status.is_success() {
            return Err(AppError::DatabaseError(format!(
                "HIGH Manticore {endpoint} request failed with status {status}"
            )));
        }

        response.json::<Value>().await.map_err(|error| {
            AppError::DatabaseError(format!(
                "Failed to parse HIGH Manticore {endpoint} response: {error}"
            ))
        })
    }

    fn projection_body(document: &HighSearchProjectionDocument) -> AppResult<Value> {
        document.validate()?;

        Ok(json!({
            "table": TABLE_NAME,
            "id": document.memo_id.to_string(),
            "doc": {
                "content_tokens": join_tokens(&document.content_tokens),
                "tag_tokens": join_tokens(&document.tag_tokens),
                "owner_partition": document.owner_partition.to_string(),
                "version": document.version,
                "analysis_version": document.analysis_version,
                "search_key_version": document.search_key_version,
                "memo_sort_key": document.memo_id.to_string(),
            }
        }))
    }

    fn search_body(
        owner_partition: Uuid,
        query: &HighSearchProjectionQuery,
        page: usize,
        limit: usize,
    ) -> AppResult<Value> {
        query.validate()?;
        validate_pagination(page, limit)?;

        let mut must = vec![json!({
            "equals": {
                "owner_partition": owner_partition.to_string()
            }
        })];

        if let Some(analysis_version) = query.analysis_version.as_deref() {
            must.push(json!({
                "equals": {
                    "analysis_version": analysis_version
                }
            }));
        }

        if let Some(key_version) = query.search_key_version.as_deref() {
            must.push(json!({
                "equals": {
                    "search_key_version": key_version
                }
            }));
        }

        if !query.content_tokens.is_empty() {
            must.push(json!({
                "match": {
                    "content_tokens": {
                        "query": join_tokens(&query.content_tokens),
                        "operator": "and"
                    }
                }
            }));
        }

        if let Some(tag_token) = query.tag_token.as_ref() {
            must.push(json!({
                "match": {
                    "tag_tokens": tag_token.value
                }
            }));
        }

        let offset = (page - 1).saturating_mul(limit);
        Ok(json!({
            "table": TABLE_NAME,
            "query": {
                "bool": {
                    "must": must
                }
            },
            "sort": [
                { "memo_sort_key": "asc" }
            ],
            "_source": {
                "excludes": ["*"]
            },
            "offset": offset,
            "limit": limit
        }))
    }

    async fn replace_document_inner(
        &self,
        document: &HighSearchProjectionDocument,
    ) -> AppResult<()> {
        self.ensure_table().await?;
        let body = Self::projection_body(document)?;
        self.post_json("replace", &body).await?;
        Ok(())
    }

    async fn search_memo_ids_inner(
        &self,
        owner_partition: Uuid,
        query: &HighSearchProjectionQuery,
        page: usize,
        limit: usize,
    ) -> AppResult<HighSearchProjectionPage> {
        self.ensure_table().await?;
        let body = Self::search_body(owner_partition, query, page, limit)?;
        let result = self.post_json("search", &body).await?;

        let total = parse_search_total(&result)?;

        let hits = result["hits"]["hits"].as_array().ok_or_else(|| {
            AppError::DatabaseError("Invalid HIGH Manticore search response format".to_string())
        })?;
        let memo_ids = hits
            .iter()
            .map(|hit| {
                hit["_id"]
                    .as_str()
                    .ok_or_else(|| {
                        AppError::DatabaseError(
                            "HIGH Manticore search hit is missing UUID _id".to_string(),
                        )
                    })
                    .and_then(|value| {
                        Uuid::parse_str(value).map_err(|error| {
                            AppError::DatabaseError(format!(
                                "Invalid memo UUID in HIGH Manticore response: {error}"
                            ))
                        })
                    })
            })
            .collect::<AppResult<Vec<_>>>()?;

        Ok(HighSearchProjectionPage { memo_ids, total })
    }

    fn metadata_match_body(metadata: &HighSearchProjectionMetadata) -> AppResult<Value> {
        metadata.validate()?;

        Ok(json!({
            "table": TABLE_NAME,
            "query": {
                "bool": {
                    "must": [
                        { "equals": { "owner_partition": metadata.owner_partition.to_string() } },
                        { "equals": { "memo_sort_key": metadata.memo_id.to_string() } },
                        { "equals": { "version": metadata.version } },
                        { "equals": { "analysis_version": metadata.analysis_version } },
                        { "equals": { "search_key_version": metadata.search_key_version } }
                    ]
                }
            },
            "_source": {
                "excludes": ["*"]
            },
            "limit": 1
        }))
    }

    fn count_body() -> Value {
        json!({
            "table": TABLE_NAME,
            "query": {
                "match_all": {}
            },
            "_source": {
                "excludes": ["*"]
            },
            "limit": 1
        })
    }

    async fn contains_metadata_inner(
        &self,
        metadata: &HighSearchProjectionMetadata,
    ) -> AppResult<bool> {
        self.ensure_table().await?;
        let result = self
            .post_json("search", &Self::metadata_match_body(metadata)?)
            .await?;
        Ok(parse_search_total(&result)? == 1)
    }

    async fn count_documents_inner(&self) -> AppResult<u64> {
        self.ensure_table().await?;
        let result = self.post_json("search", &Self::count_body()).await?;
        search_total_u64(&result)
    }

    fn delete_body(owner_partition: Uuid, memo_id: Uuid) -> Value {
        json!({
            "table": TABLE_NAME,
            "query": {
                "bool": {
                    "must": [
                        {
                            "equals": {
                                "owner_partition": owner_partition.to_string()
                            }
                        },
                        {
                            "equals": {
                                "memo_sort_key": memo_id.to_string()
                            }
                        }
                    ]
                }
            }
        })
    }

    async fn delete_document_inner(&self, owner_partition: Uuid, memo_id: Uuid) -> AppResult<()> {
        self.ensure_table().await?;
        let body = Self::delete_body(owner_partition, memo_id);
        self.post_json("delete", &body).await?;
        Ok(())
    }

    pub async fn health_check(&self) -> AppResult<bool> {
        self.execute_raw_sql("SELECT 1", "health check").await?;
        Ok(true)
    }
}

fn search_total_u64(result: &Value) -> AppResult<u64> {
    result["hits"]["total"]
        .as_u64()
        .ok_or_else(|| AppError::DatabaseError("Invalid HIGH Manticore search total".to_string()))
}

fn parse_search_total(result: &Value) -> AppResult<usize> {
    let total = search_total_u64(result)?;
    usize::try_from(total).map_err(|_| {
        AppError::DatabaseError("HIGH Manticore search total exceeds platform limits".to_string())
    })
}

fn join_tokens(tokens: &[crate::application::crypto_search::HighSearchToken]) -> String {
    tokens
        .iter()
        .map(|token| token.value.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

fn validate_pagination(page: usize, limit: usize) -> AppResult<()> {
    if page == 0 || !(1..=100).contains(&limit) {
        return Err(AppError::ValidationError(
            "HIGH search page must be >= 1 and limit must be between 1 and 100".into(),
        ));
    }
    Ok(())
}

#[async_trait]
impl HighMemoSearchProjection for HighManticoreClient {
    async fn replace_document(&self, document: &HighSearchProjectionDocument) -> AppResult<()> {
        self.replace_document_inner(document).await
    }

    async fn search_memo_ids(
        &self,
        owner_partition: Uuid,
        query: &HighSearchProjectionQuery,
        page: usize,
        limit: usize,
    ) -> AppResult<HighSearchProjectionPage> {
        self.search_memo_ids_inner(owner_partition, query, page, limit)
            .await
    }

    async fn delete_document(&self, owner_partition: Uuid, memo_id: Uuid) -> AppResult<()> {
        self.delete_document_inner(owner_partition, memo_id).await
    }
}

#[async_trait]
impl HighSearchProjectionMigrationInspector for HighManticoreClient {
    async fn contains_metadata(&self, metadata: &HighSearchProjectionMetadata) -> AppResult<bool> {
        self.contains_metadata_inner(metadata).await
    }

    async fn count_documents(&self) -> AppResult<u64> {
        self.count_documents_inner().await
    }
}

#[async_trait]
impl HealthProbe for HighManticoreClient {
    async fn check(&self) -> bool {
        match self.health_check().await {
            Ok(healthy) => healthy,
            Err(error) => {
                log::warn!("HIGH Manticore health check failed: {error}");
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::{
        crypto_search::{HighSearchToken, SEARCH_HIGH_SUITE_ID, SEARCH_HIGH_TOKEN_BYTES},
        crypto_search_projection::{HighSearchProjectionDocument, HighSearchProjectionQuery},
    };

    fn token(key_version: &str, pair: &str) -> HighSearchToken {
        HighSearchToken {
            value: pair.repeat(SEARCH_HIGH_TOKEN_BYTES),
            key_version: key_version.into(),
            suite_id: SEARCH_HIGH_SUITE_ID.into(),
        }
    }

    #[tokio::test]
    #[ignore = "requires a local Manticore Search instance"]
    async fn high_manticore_preserves_owner_scope_and_blind_tokens() {
        let uri = std::env::var("MANTICORE_TEST_URL")
            .unwrap_or_else(|_| "http://localhost:9308".to_string());
        let client = HighManticoreClient::new(&uri).unwrap();

        client
            .execute_raw_sql(
                "DROP TABLE IF EXISTS memos_high_v1",
                "integration cleanup before test",
            )
            .await
            .unwrap();

        let owner = Uuid::new_v4();
        let other_owner = Uuid::new_v4();
        let memo_id = Uuid::new_v4();
        let blind_content = token("search-v1", "ab");
        let blind_tag = token("search-v1", "cd");
        let document = HighSearchProjectionDocument {
            memo_id,
            owner_partition: owner,
            version: 1,
            analysis_version: "analysis-v1".into(),
            search_key_version: "search-v1".into(),
            content_tokens: vec![blind_content.clone()],
            tag_tokens: vec![blind_tag.clone()],
        };

        client.replace_document(&document).await.unwrap();

        let metadata = HighSearchProjectionMetadata::from(&document);
        assert!(client.contains_metadata(&metadata).await.unwrap());
        assert_eq!(client.count_documents().await.unwrap(), 1);
        let mut wrong_version = metadata.clone();
        wrong_version.version += 1;
        assert!(!client.contains_metadata(&wrong_version).await.unwrap());

        let query = HighSearchProjectionQuery {
            content_tokens: vec![blind_content],
            tag_token: Some(blind_tag),
            analysis_version: Some("analysis-v1".into()),
            search_key_version: Some("search-v1".into()),
        };

        let owner_hits = client.search_memo_ids(owner, &query, 1, 20).await.unwrap();
        assert_eq!(owner_hits.total, 1);
        assert_eq!(owner_hits.memo_ids, vec![memo_id]);

        let other_owner_hits = client
            .search_memo_ids(other_owner, &query, 1, 20)
            .await
            .unwrap();
        assert_eq!(other_owner_hits.total, 0);
        assert!(other_owner_hits.memo_ids.is_empty());

        client.delete_document(other_owner, memo_id).await.unwrap();
        let after_wrong_owner_delete = client.search_memo_ids(owner, &query, 1, 20).await.unwrap();
        assert_eq!(after_wrong_owner_delete.memo_ids, vec![memo_id]);

        client.delete_document(owner, memo_id).await.unwrap();
        let after_owner_delete = client.search_memo_ids(owner, &query, 1, 20).await.unwrap();
        assert_eq!(after_owner_delete.total, 0);
        assert!(after_owner_delete.memo_ids.is_empty());
        assert_eq!(client.count_documents().await.unwrap(), 0);

        client
            .execute_raw_sql(
                "DROP TABLE IF EXISTS memos_high_v1",
                "integration cleanup after test",
            )
            .await
            .unwrap();
    }

    #[test]
    fn protected_projection_body_contains_no_semantic_plaintext_fields() {
        let document = HighSearchProjectionDocument {
            memo_id: Uuid::new_v4(),
            owner_partition: Uuid::new_v4(),
            version: 4,
            analysis_version: "analysis-v1".into(),
            search_key_version: "search-v1".into(),
            content_tokens: vec![token("search-v1", "ab")],
            tag_tokens: vec![token("search-v1", "cd")],
        };

        let body = HighManticoreClient::projection_body(&document).unwrap();
        let doc = body["doc"].as_object().unwrap();

        assert_eq!(
            doc.keys()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>(),
            std::collections::BTreeSet::from([
                "content_tokens".to_string(),
                "tag_tokens".to_string(),
                "owner_partition".to_string(),
                "version".to_string(),
                "analysis_version".to_string(),
                "search_key_version".to_string(),
                "memo_sort_key".to_string(),
            ])
        );
        assert!(!doc.contains_key("title"));
        assert!(!doc.contains_key("content"));
        assert!(!doc.contains_key("tags"));
        assert!(!doc.contains_key("updated_at"));
    }

    #[test]
    fn protected_search_filters_owner_and_key_version_and_hides_source() {
        let owner = Uuid::new_v4();
        let query = HighSearchProjectionQuery {
            content_tokens: vec![token("search-v1", "ab")],
            tag_token: Some(token("search-v1", "cd")),
            analysis_version: Some("analysis-v1".into()),
            search_key_version: Some("search-v1".into()),
        };

        let body = HighManticoreClient::search_body(owner, &query, 2, 25).unwrap();
        let serialized = serde_json::to_string(&body).unwrap();

        assert!(serialized.contains(&owner.to_string()));
        assert!(serialized.contains("analysis-v1"));
        assert!(serialized.contains("search-v1"));
        assert!(serialized.contains(&token("search-v1", "ab").value));
        assert!(serialized.contains(&token("search-v1", "cd").value));
        assert_eq!(body["offset"], 25);
        assert_eq!(body["limit"], 25);
        assert_eq!(body["_source"]["excludes"][0], "*");
    }

    #[test]
    fn protected_delete_is_owner_scoped() {
        let owner = Uuid::new_v4();
        let memo_id = Uuid::new_v4();
        let body = HighManticoreClient::delete_body(owner, memo_id);
        let serialized = serde_json::to_string(&body).unwrap();

        assert!(serialized.contains(&owner.to_string()));
        assert!(serialized.contains(&memo_id.to_string()));
        assert!(serialized.contains("owner_partition"));
        assert!(serialized.contains("memo_sort_key"));
        assert!(body.get("id").is_none());
    }

    #[test]
    fn protected_search_rejects_invalid_pagination() {
        let query = HighSearchProjectionQuery {
            content_tokens: vec![],
            tag_token: None,
            analysis_version: None,
            search_key_version: None,
        };

        assert!(HighManticoreClient::search_body(Uuid::new_v4(), &query, 0, 10).is_err());
        assert!(HighManticoreClient::search_body(Uuid::new_v4(), &query, 1, 0).is_err());
        assert!(HighManticoreClient::search_body(Uuid::new_v4(), &query, 1, 101).is_err());
    }

    #[test]
    fn protected_table_keeps_blind_tokens_indexed_only() {
        assert!(CREATE_TABLE_SQL.contains("content_tokens text indexed"));
        assert!(CREATE_TABLE_SQL.contains("tag_tokens text indexed"));
        assert!(CREATE_TABLE_SQL.contains("dict='keywords_32k'"));
        assert!(!CREATE_TABLE_SQL.contains("title"));
        assert!(!CREATE_TABLE_SQL.contains("updated_at"));
    }
}
