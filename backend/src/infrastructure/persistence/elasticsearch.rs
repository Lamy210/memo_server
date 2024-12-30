// src/infrastructure/persistence/elasticsearch.rs

use elasticsearch::{
    Elasticsearch,
    http::transport::Transport,
    indices::{IndicesCreateParts, IndicesExistsParts},
    BulkParts, BulkOperation, DeleteParts, GetParts, IndexParts, SearchParts,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use crate::error::{AppError, AppResult};
use uuid::Uuid;
use crate::domain::memo::entity::Memo;

pub struct ElasticsearchClient {
    client: Elasticsearch,
}

impl ElasticsearchClient {
    pub async fn new(url: &str) -> AppResult<Self> {
        let transport = Transport::single_node(url)
            .map_err(|e| AppError::DatabaseError(e.to_string()))?;
        
        let client = Elasticsearch::new(transport);
        
        // インデックスの初期化
        Self::initialize_indices(&client).await?;

        Ok(Self { client })
    }

    async fn initialize_indices(client: &Elasticsearch) -> AppResult<()> {
        let index_exists = client
            .indices()
            .exists(IndicesExistsParts::Index(&["memos"]))
            .send()
            .await
            .map_err(|e| AppError::DatabaseError(e.to_string()))?
            .status_code()
            .is_success();

        if !index_exists {
            let memo_mapping = json!({
                "mappings": {
                    "properties": {
                        "id": { "type": "keyword" },
                        "title": { "type": "text", "analyzer": "standard" },
                        "content": { "type": "text", "analyzer": "standard" },
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
                .create(IndicesCreateParts::Index("memos"))
                .body(memo_mapping)
                .send()
                .await
                .map_err(|e| AppError::DatabaseError(e.to_string()))?;
        }

        Ok(())
    }

    pub async fn index_memo<T: Serialize>(&self, id: Uuid, document: &T) -> AppResult<()> {
        self.client
            .index(IndexParts::IndexId("memos", &id.to_string()))
            .body(document)
            .send()
            .await
            .map_err(|e| AppError::DatabaseError(e.to_string()))?;

        Ok(())
    }

    pub async fn get_memo<T: for<'de> Deserialize<'de>>(
        &self,
        id: Uuid,
    ) -> AppResult<Option<T>> {
        let response = self
            .client
            .get(GetParts::IndexId("memos", &id.to_string()))
            .send()
            .await
            .map_err(|e| AppError::DatabaseError(e.to_string()))?;

        if response.status_code().is_success() {
            let document = response
                .json::<serde_json::Value>()
                .await
                .map_err(|e| AppError::DatabaseError(e.to_string()))?;

            if let Some(source) = document.get("_source") {
                let parsed: T = serde_json::from_value(source.clone())
                    .map_err(|e| AppError::DatabaseError(e.to_string()))?;
                Ok(Some(parsed))
            } else {
                Ok(None)
            }
        } else {
            Ok(None)
        }
    }

    pub async fn delete_memo(&self, id: Uuid) -> AppResult<()> {
        self.client
            .delete(DeleteParts::IndexId("memos", &id.to_string()))
            .send()
            .await
            .map_err(|e| AppError::DatabaseError(e.to_string()))?;

        Ok(())
    }

    pub async fn search_memos(
        &self,
        query: &str,
        tag: Option<String>,
        user_id: Uuid,
    ) -> AppResult<Vec<Memo>> {
        let should_conditions = vec![
            json!({
                "multi_match": {
                    "query": query,
                    "fields": ["title^2", "content"],
                    "type": "best_fields",
                    "fuzziness": "AUTO"
                }
            })
        ];

        let mut filter_conditions = vec![
            json!({
                "term": {
                    "user_id": user_id.to_string()
                }
            })
        ];

        if let Some(tag) = tag {
            filter_conditions.push(json!({
                "term": {
                    "tags": tag
                }
            }));
        }

        let query_body = json!({
            "query": {
                "bool": {
                    "must": {
                        "bool": {
                            "should": should_conditions,
                            "minimum_should_match": 1
                        }
                    },
                    "filter": filter_conditions
                }
            },
            "sort": [
                { "updated_at": { "order": "desc" } }
            ],
            "track_total_hits": true
        });

        let response = self
            .client
            .search(SearchParts::Index(&["memos"]))
            .body(query_body)
            .send()
            .await
            .map_err(|e| AppError::DatabaseError(e.to_string()))?;

        if response.status_code().is_success() {
            let search_response = response
                .json::<serde_json::Value>()
                .await
                .map_err(|e| AppError::DatabaseError(e.to_string()))?;

            if let Some(hits) = search_response["hits"]["hits"].as_array() {
                let mut results = Vec::with_capacity(hits.len());
                for hit in hits {
                    if let Some(source) = hit["_source"].as_object() {
                        let parsed: Memo = serde_json::from_value(source.clone())
                            .map_err(|e| AppError::DatabaseError(e.to_string()))?;
                        results.push(parsed);
                    }
                }
                Ok(results)
            } else {
                Ok(vec![])
            }
        } else {
            Err(AppError::DatabaseError("Search request failed".into()))
        }
    }

    pub async fn bulk_index(
        &self,
        documents: Vec<(Uuid, Memo)>,
    ) -> AppResult<()> {
        let mut bulk_operations = Vec::with_capacity(documents.len());

        for (id, doc) in documents {
            let index_op = BulkOperation::Index {
                index: "memos",
                id: id.to_string(),
                document: doc,
            };
            bulk_operations.push(index_op);
        }

        self.client
            .bulk(BulkParts::Index("memos"))
            .body(bulk_operations)
            .send()
            .await
            .map_err(|e| AppError::DatabaseError(e.to_string()))?;

        Ok(())
    }

    pub async fn health_check(&self) -> AppResult<bool> {
        let response = self
            .client
            .cat()
            .health()
            .format("json")
            .send()
            .await
            .map_err(|e| AppError::DatabaseError(e.to_string()))?;

        Ok(response.status_code().is_success())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio;

    #[tokio::test]
    async fn test_elasticsearch_connection() {
        let es_client = ElasticsearchClient::new("http://localhost:9200").await;
        assert!(es_client.is_ok(), "Should connect to Elasticsearch successfully");
    }
}
