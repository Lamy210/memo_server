// src/infrastructure/persistence/elasticsearch.rs

use elasticsearch::{
    Elasticsearch,
    http::transport::Transport,
    SearchParts,
    DeleteByQueryParts,
    IndexParts,
    params::Refresh,
};
use serde_json::{json, Value};
use uuid::Uuid;
use crate::error::{AppError, AppResult};
use crate::domain::memo::entity::Memo;

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

        // インデックスの初期化
        Self::initialize_index(&client).await?;

        Ok(Self { client })
    }

    async fn initialize_index(client: &Elasticsearch) -> AppResult<()> {
        let exists = client
            .indices()
            .exists(elasticsearch::indices::IndicesExistsParts::Index(&[INDEX_NAME]))
            .send()
            .await
            .map_err(|e| AppError::DatabaseError(format!("Failed to check index existence: {}", e)))?
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
                    "number_of_replicas": 1
                }
            });

            client
                .indices()
                .create(elasticsearch::indices::IndicesCreateParts::Index(INDEX_NAME))
                .body(mapping)
                .send()
                .await
                .map_err(|e| AppError::DatabaseError(format!("Failed to create index: {}", e)))?;
        }

        Ok(())
    }

    pub async fn index_memo(&self, memo: &Memo) -> AppResult<()> {
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

        self.client
            .index(IndexParts::Index(INDEX_NAME))
            .id(memo.id.to_string())
            .document(&doc)
            .refresh(Refresh::True)
            .send()
            .await
            .map_err(|e| AppError::DatabaseError(format!("Failed to index memo: {}", e)))?;

        Ok(())
    }

    pub async fn search_memos(
        &self,
        query: &str,
        tag: Option<String>,
        user_id: Uuid,
    ) -> AppResult<Vec<Memo>> {
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
                })
            ]);
        }

        let mut must_clauses = vec![
            json!({
                "term": {
                    "user_id": user_id.to_string()
                }
            })
        ];

        if let Some(tag_value) = tag {
            must_clauses.push(json!({
                "term": {
                    "tags": tag_value
                }
            }));
        }

        let query_body = json!({
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

        let response = self.client
            .search(SearchParts::Index(&[INDEX_NAME]))
            .body(query_body)
            .size(100)
            .send()
            .await
            .map_err(|e| AppError::DatabaseError(format!("Failed to execute search: {}", e)))?;

        let search_hits = response.json::<Value>().await.map_err(|e| {
            AppError::DatabaseError(format!("Failed to parse search response: {}", e))
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
                        source["created_at"].as_str()?
                    ).ok()?.with_timezone(&chrono::Utc),
                    updated_at: chrono::DateTime::parse_from_rfc3339(
                        source["updated_at"].as_str()?
                    ).ok()?.with_timezone(&chrono::Utc),
                    version: source["version"].as_i64()? as i32,
                })
            })
            .collect();

        Ok(memos)
    }

    pub async fn delete_memo(&self, id: Uuid) -> AppResult<()> {
        let query_body = json!({
            "query": {
                "term": {
                    "id": id.to_string()
                }
            }
        });

        self.client
            .delete_by_query(DeleteByQueryParts::Index(&[INDEX_NAME]))
            .body(query_body)
            .refresh(true)
            .send()
            .await
            .map_err(|e| AppError::DatabaseError(format!("Failed to delete memo: {}", e)))?;

        Ok(())
    }

    pub async fn health_check(&self) -> AppResult<bool> {
        let response = self.client
            .cat()
            .health()
            .send()
            .await
            .map_err(|e| AppError::DatabaseError(format!("Health check failed: {}", e)))?;

        Ok(response.status_code().is_success())
    }
}