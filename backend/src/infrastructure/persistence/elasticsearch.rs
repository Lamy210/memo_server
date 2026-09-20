use async_trait::async_trait;

use crate::application::health::HealthProbe;
use crate::domain::memo::{entity::Memo, repository::MemoSearchPage};
use crate::error::{AppError, AppResult};

use super::ports::MemoSearchProjection;
use elasticsearch::{
    http::transport::Transport, params::Refresh, DeleteByQueryParts, Elasticsearch, IndexParts,
    SearchParts,
};
use serde_json::{json, Value};
use uuid::Uuid;

const INDEX_NAME: &str = "memos";

pub struct ElasticsearchClient {
    client: Elasticsearch,
}

impl ElasticsearchClient {
    pub async fn new(uri: &str) -> AppResult<Self> {
        let transport = Transport::single_node(uri).map_err(|e| {
            AppError::DatabaseError(format!("Failed to create Elasticsearch transport: {}", e))
        })?;
        let client = Elasticsearch::new(transport);

        Ok(Self { client })
    }

    async fn ensure_index(&self) -> AppResult<()> {
        Self::initialize_index(&self.client).await
    }

    async fn initialize_index(client: &Elasticsearch) -> AppResult<()> {
        let exists = client
            .indices()
            .exists(elasticsearch::indices::IndicesExistsParts::Index(&[
                INDEX_NAME,
            ]))
            .send()
            .await
            .map_err(|e| {
                AppError::DatabaseError(format!("Failed to check index existence: {}", e))
            })?
            .status_code()
            .is_success();

        if !exists {
            let mapping = json!({
                "mappings": {
                    "properties": {
                        "id": { "type": "keyword" },
                        "title": {
                            "type": "text",
                            "analyzer": "standard",
                            "fields": {
                                "keyword": { "type": "keyword" }
                            }
                        },
                        "content": {
                            "type": "text",
                            "analyzer": "standard"
                        },
                        "tags": { "type": "keyword" },
                        "user_id": { "type": "keyword" },
                        "created_at": { "type": "date" },
                        "updated_at": { "type": "date" },
                        "version": { "type": "integer" }
                    }
                },
                "settings": {
                    "number_of_shards": 1,
                    "number_of_replicas": 0
                }
            });

            client
                .indices()
                .create(elasticsearch::indices::IndicesCreateParts::Index(
                    INDEX_NAME,
                ))
                .body(mapping)
                .send()
                .await
                .map_err(|e| AppError::DatabaseError(format!("Failed to create index: {}", e)))?;
        }

        Ok(())
    }

    pub async fn index_memo(&self, memo: &Memo) -> AppResult<()> {
        self.ensure_index().await?;

        let doc = json!({
            "id": memo.id.to_string(),
            "title": memo.title,
            "content": memo.content,
            "tags": memo.tags,
            "user_id": memo.user_id.to_string(),
            "created_at": memo.created_at,
            "updated_at": memo.updated_at,
            "version": memo.version
        });
        let memo_id = memo.id.to_string();

        let response = self
            .client
            .index(IndexParts::IndexId(INDEX_NAME, &memo_id))
            .body(doc)
            .refresh(Refresh::True)
            .send()
            .await
            .map_err(|e| AppError::DatabaseError(format!("Failed to index memo: {}", e)))?;

        if !response.status_code().is_success() {
            return Err(AppError::DatabaseError(format!(
                "Elasticsearch rejected memo index request with status {}",
                response.status_code()
            )));
        }

        Ok(())
    }

    pub async fn search_memos(
        &self,
        query: &str,
        tag: Option<String>,
        user_id: Uuid,
        page: usize,
        limit: usize,
    ) -> AppResult<MemoSearchPage> {
        self.ensure_index().await?;

        let mut should_clauses: Vec<Value> = vec![];

        if !query.is_empty() {
            should_clauses.extend(vec![
                json!({
                    "match": {
                        "title": {
                            "query": query,
                            "boost": 2.0
                        }
                    }
                }),
                json!({
                    "match": {
                        "content": query
                    }
                }),
            ]);
        }

        let mut must_clauses = vec![json!({
            "term": {
                "user_id": user_id.to_string()
            }
        })];

        if let Some(tag_value) = tag {
            must_clauses.push(json!({
                "term": {
                    "tags": tag_value
                }
            }));
        }

        let offset = (page - 1).saturating_mul(limit);

        let query_body = json!({
            "from": offset,
            "size": limit,
            "track_total_hits": true,
            "query": {
                "bool": {
                    "must": must_clauses,
                    "should": should_clauses,
                    "minimum_should_match": if query.is_empty() { 0 } else { 1 }
                }
            },
            "sort": [
                { "updated_at": { "order": "desc" } }
            ]
        });

        let response = self
            .client
            .search(SearchParts::Index(&[INDEX_NAME]))
            .body(query_body)
            .send()
            .await
            .map_err(|e| AppError::DatabaseError(format!("Failed to execute search: {}", e)))?;

        let search_hits = response.json::<Value>().await.map_err(|e| {
            AppError::DatabaseError(format!("Failed to parse search response: {}", e))
        })?;

        let total = search_hits["hits"]["total"]["value"]
            .as_u64()
            .ok_or_else(|| {
                AppError::DatabaseError(
                    "Invalid search total in Elasticsearch response".to_string(),
                )
            })
            .and_then(|value| {
                usize::try_from(value).map_err(|_| {
                    AppError::DatabaseError(
                        "Elasticsearch search total exceeds platform limits".to_string(),
                    )
                })
            })?;
        let hits = search_hits["hits"]["hits"]
            .as_array()
            .ok_or_else(|| AppError::DatabaseError("Invalid search response format".to_string()))?;

        let memos = hits
            .iter()
            .filter_map(|hit| {
                let source = hit["_source"].as_object()?;
                let id = Uuid::parse_str(source["id"].as_str()?).ok()?;
                let user_id = Uuid::parse_str(source["user_id"].as_str()?).ok()?;

                Some(Memo {
                    id,
                    title: source["title"].as_str()?.to_string(),
                    content: source["content"].as_str()?.to_string(),
                    tags: source["tags"]
                        .as_array()?
                        .iter()
                        .filter_map(|t| t.as_str().map(String::from))
                        .collect(),
                    user_id,
                    created_at: chrono::DateTime::parse_from_rfc3339(
                        source["created_at"].as_str()?,
                    )
                    .ok()?
                    .with_timezone(&chrono::Utc),
                    updated_at: chrono::DateTime::parse_from_rfc3339(
                        source["updated_at"].as_str()?,
                    )
                    .ok()?
                    .with_timezone(&chrono::Utc),
                    version: source["version"].as_i64()? as i32,
                })
            })
            .collect();

        Ok(MemoSearchPage {
            items: memos,
            total,
        })
    }

    pub async fn delete_memo(&self, id: Uuid) -> AppResult<()> {
        self.ensure_index().await?;

        let query_body = json!({
            "query": {
                "term": {
                    "id": id.to_string()
                }
            }
        });

        let response = self
            .client
            .delete_by_query(DeleteByQueryParts::Index(&[INDEX_NAME]))
            .body(query_body)
            .refresh(true)
            .send()
            .await
            .map_err(|e| AppError::DatabaseError(format!("Failed to delete memo: {}", e)))?;

        if !response.status_code().is_success() {
            return Err(AppError::DatabaseError(format!(
                "Elasticsearch rejected memo delete request with status {}",
                response.status_code()
            )));
        }

        Ok(())
    }

    pub async fn health_check(&self) -> AppResult<bool> {
        let response = self
            .client
            .cat()
            .health()
            .send()
            .await
            .map_err(|e| AppError::DatabaseError(format!("Health check failed: {}", e)))?;

        Ok(response.status_code().is_success())
    }
}

#[async_trait]
impl MemoSearchProjection for ElasticsearchClient {
    async fn index_memo(&self, memo: &Memo) -> AppResult<()> {
        ElasticsearchClient::index_memo(self, memo).await
    }

    async fn search_memos(
        &self,
        query: &str,
        tag: Option<String>,
        user_id: Uuid,
        page: usize,
        limit: usize,
    ) -> AppResult<MemoSearchPage> {
        ElasticsearchClient::search_memos(self, query, tag, user_id, page, limit).await
    }

    async fn delete_memo(&self, id: Uuid) -> AppResult<()> {
        ElasticsearchClient::delete_memo(self, id).await
    }
}

#[async_trait]
impl HealthProbe for ElasticsearchClient {
    async fn check(&self) -> bool {
        match self.health_check().await {
            Ok(healthy) => healthy,
            Err(error) => {
                log::warn!("Elasticsearch health check failed: {error}");
                false
            }
        }
    }
}
