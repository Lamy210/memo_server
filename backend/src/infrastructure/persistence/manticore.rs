use std::fmt::Write as _;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    application::health::HealthProbe,
    domain::memo::{entity::Memo, repository::MemoSearchPage},
    error::{AppError, AppResult},
};

use super::ports::MemoSearchProjection;

const TABLE_NAME: &str = "memos";
const CREATE_TABLE_SQL: &str = "CREATE TABLE IF NOT EXISTS memos (id uuid, title text, content text, tag_tokens text indexed, tags_json string, user_id string, created_at bigint, updated_at bigint, version int) dict='keywords_32k'";

pub struct ManticoreClient {
    client: Client,
    base_url: String,
}

impl ManticoreClient {
    pub async fn new(uri: &str) -> AppResult<Self> {
        let client = Client::builder().build().map_err(|error| {
            AppError::DatabaseError(format!("Failed to create Manticore HTTP client: {error}"))
        })?;
        let client = Self {
            client,
            base_url: uri.trim_end_matches('/').to_string(),
        };
        client.initialize_table().await?;
        Ok(client)
    }

    async fn initialize_table(&self) -> AppResult<()> {
        let response = self
            .client
            .post(format!("{}/sql?mode=raw", self.base_url))
            .header(reqwest::header::CONTENT_TYPE, "text/plain")
            .body(CREATE_TABLE_SQL)
            .send()
            .await
            .map_err(|error| {
                AppError::DatabaseError(format!("Failed to initialize Manticore table: {error}"))
            })?;

        if !response.status().is_success() {
            return Err(AppError::DatabaseError(format!(
                "Manticore rejected table initialization with status {}",
                response.status()
            )));
        }

        let result = response.json::<Value>().await.map_err(|error| {
            AppError::DatabaseError(format!(
                "Failed to parse Manticore table initialization response: {error}"
            ))
        })?;
        let result_sets = result.as_array().ok_or_else(|| {
            AppError::DatabaseError(
                "Invalid Manticore table initialization response format".to_string(),
            )
        })?;

        if let Some(error) = result_sets
            .iter()
            .filter_map(|result_set| result_set.get("error").and_then(Value::as_str))
            .find(|error| !error.is_empty())
        {
            return Err(AppError::DatabaseError(format!(
                "Manticore table initialization failed: {error}"
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
                    "Manticore {endpoint} request failed: {error}"
                ))
            })?;

        if !response.status().is_success() {
            return Err(AppError::DatabaseError(format!(
                "Manticore {endpoint} request failed with status {}",
                response.status()
            )));
        }

        response.json::<Value>().await.map_err(|error| {
            AppError::DatabaseError(format!(
                "Failed to parse Manticore {endpoint} response: {error}"
            ))
        })
    }

    pub async fn index_memo(&self, memo: &Memo) -> AppResult<()> {
        let tags_json = serde_json::to_string(&memo.tags).map_err(|error| {
            AppError::DatabaseError(format!("Failed to serialize memo tags: {error}"))
        })?;
        let body = json!({
            "table": TABLE_NAME,
            "id": memo.id.to_string(),
            "doc": {
                "title": memo.title,
                "content": memo.content,
                "tag_tokens": encode_tag_tokens(&memo.tags),
                "tags_json": tags_json,
                "user_id": memo.user_id.to_string(),
                "created_at": memo.created_at.timestamp_millis(),
                "updated_at": memo.updated_at.timestamp_millis(),
                "version": memo.version
            }
        });

        self.post_json("replace", &body).await?;
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
        let mut must = vec![json!({
            "equals": {
                "user_id": user_id.to_string()
            }
        })];

        if !query.is_empty() {
            must.push(json!({
                "match": {
                    "title,content": {
                        "query": escape_match_query(query),
                        "operator": "or"
                    }
                }
            }));
        }

        if let Some(tag) = tag.as_deref() {
            must.push(json!({
                "match": {
                    "tag_tokens": encode_tag_token(tag)
                }
            }));
        }

        let offset = (page - 1).saturating_mul(limit);
        let body = json!({
            "table": TABLE_NAME,
            "query": {
                "bool": {
                    "must": must
                }
            },
            "_source": [
                "title",
                "content",
                "tags_json",
                "user_id",
                "created_at",
                "updated_at",
                "version"
            ],
            "sort": [
                { "updated_at": "desc" }
            ],
            "offset": offset,
            "limit": limit,
            "options": {
                "field_weights": {
                    "title": 2,
                    "content": 1
                }
            }
        });

        let search_result = self.post_json("search", &body).await?;
        let total = search_result["hits"]["total"]
            .as_u64()
            .ok_or_else(|| {
                AppError::DatabaseError("Invalid Manticore search total".to_string())
            })
            .and_then(|value| {
                usize::try_from(value).map_err(|_| {
                    AppError::DatabaseError(
                        "Manticore search total exceeds platform limits".to_string(),
                    )
                })
            })?;

        let hits = search_result["hits"]["hits"].as_array().ok_or_else(|| {
            AppError::DatabaseError("Invalid Manticore search response format".to_string())
        })?;
        let items = hits
            .iter()
            .map(parse_memo_hit)
            .collect::<AppResult<Vec<_>>>()?;

        Ok(MemoSearchPage { items, total })
    }

    pub async fn delete_memo(&self, id: Uuid) -> AppResult<()> {
        let body = json!({
            "table": TABLE_NAME,
            "id": id.to_string()
        });
        self.post_json("delete", &body).await?;
        Ok(())
    }

    pub async fn health_check(&self) -> AppResult<bool> {
        let response = self
            .client
            .get(format!("{}/sql", self.base_url))
            .query(&[("query", "SELECT 1")])
            .send()
            .await
            .map_err(|error| {
                AppError::DatabaseError(format!("Manticore health check failed: {error}"))
            })?;

        Ok(response.status().is_success())
    }
}

fn parse_memo_hit(hit: &Value) -> AppResult<Memo> {
    let source = hit["_source"].as_object().ok_or_else(|| {
        AppError::DatabaseError("Manticore hit is missing _source".to_string())
    })?;

    let id = hit["_id"]
        .as_str()
        .ok_or_else(|| AppError::DatabaseError("Manticore hit is missing UUID _id".to_string()))
        .and_then(|value| {
            Uuid::parse_str(value).map_err(|error| {
                AppError::DatabaseError(format!("Invalid memo UUID in Manticore response: {error}"))
            })
        })?;
    let user_id = source
        .get("user_id")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::DatabaseError("Manticore hit is missing user_id".to_string()))
        .and_then(|value| {
            Uuid::parse_str(value).map_err(|error| {
                AppError::DatabaseError(format!("Invalid user UUID in Manticore response: {error}"))
            })
        })?;
    let tags = source
        .get("tags_json")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::DatabaseError("Manticore hit is missing tags_json".to_string()))
        .and_then(|value| {
            serde_json::from_str::<Vec<String>>(value).map_err(|error| {
                AppError::DatabaseError(format!("Invalid tags JSON in Manticore response: {error}"))
            })
        })?;

    let created_at = timestamp_from_source(source.get("created_at"), "created_at")?;
    let updated_at = timestamp_from_source(source.get("updated_at"), "updated_at")?;
    let version = source
        .get("version")
        .and_then(Value::as_i64)
        .ok_or_else(|| AppError::DatabaseError("Manticore hit is missing version".to_string()))
        .and_then(|value| {
            i32::try_from(value).map_err(|_| {
                AppError::DatabaseError("Manticore memo version exceeds i32 range".to_string())
            })
        })?;

    Ok(Memo {
        id,
        title: source
            .get("title")
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::DatabaseError("Manticore hit is missing title".to_string()))?
            .to_string(),
        content: source
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::DatabaseError("Manticore hit is missing content".to_string()))?
            .to_string(),
        tags,
        user_id,
        created_at,
        updated_at,
        version,
    })
}

fn timestamp_from_source(value: Option<&Value>, field: &str) -> AppResult<DateTime<Utc>> {
    let milliseconds = value
        .and_then(Value::as_i64)
        .ok_or_else(|| AppError::DatabaseError(format!("Manticore hit is missing {field}")))?;

    DateTime::<Utc>::from_timestamp_millis(milliseconds).ok_or_else(|| {
        AppError::DatabaseError(format!("Invalid {field} timestamp in Manticore response"))
    })
}

fn escape_match_query(query: &str) -> String {
    const SPECIAL: &[char] = &[
        '!', '"', '$', '\'', '(', ')', '-', '/', '<', '@', '\\', '^', '|', '~',
    ];

    let mut escaped = String::with_capacity(query.len());
    for character in query.chars() {
        if SPECIAL.contains(&character) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

fn encode_tag_tokens(tags: &[String]) -> String {
    tags.iter()
        .map(|tag| encode_tag_token(tag))
        .collect::<Vec<_>>()
        .join(" ")
}

fn encode_tag_token(tag: &str) -> String {
    let mut encoded = String::with_capacity(tag.len().saturating_mul(2));
    for byte in tag.as_bytes() {
        let _ = write!(&mut encoded, "{byte:02x}");
    }
    encoded
}

#[async_trait]
impl MemoSearchProjection for ManticoreClient {
    async fn index_memo(&self, memo: &Memo) -> AppResult<()> {
        ManticoreClient::index_memo(self, memo).await
    }

    async fn search_memos(
        &self,
        query: &str,
        tag: Option<String>,
        user_id: Uuid,
        page: usize,
        limit: usize,
    ) -> AppResult<MemoSearchPage> {
        ManticoreClient::search_memos(self, query, tag, user_id, page, limit).await
    }

    async fn delete_memo(&self, id: Uuid) -> AppResult<()> {
        ManticoreClient::delete_memo(self, id).await
    }
}

#[async_trait]
impl HealthProbe for ManticoreClient {
    async fn check(&self) -> bool {
        match self.health_check().await {
            Ok(healthy) => healthy,
            Err(error) => {
                log::warn!("Manticore health check failed: {error}");
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_tokens_are_unambiguous_ascii_words() {
        let one = encode_tag_token("a b");
        let two = encode_tag_token("ab");

        assert_ne!(one, two);
        assert!(one.chars().all(|character| character.is_ascii_hexdigit()));
        assert_eq!(
            encode_tag_tokens(&["bug".into(), "high priority".into()]),
            "627567 68696768207072696f72697479"
        );
    }

    #[test]
    fn match_query_operators_are_escaped() {
        assert_eq!(
            escape_match_query(r#"hello | @title "memo" -draft \ archive"#),
            r#"hello \| \@title \"memo\" \-draft \\ archive"#
        );
    }
}
