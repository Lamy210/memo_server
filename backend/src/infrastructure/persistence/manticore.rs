use std::fmt::Write as _;

use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use tokio::sync::OnceCell;
use uuid::Uuid;

use crate::{
    application::health::HealthProbe,
    domain::memo::entity::Memo,
    error::{AppError, AppResult},
};

use super::ports::{MemoSearchHitPage, MemoSearchProjection};

const TABLE_NAME: &str = "memos";
const CREATE_TABLE_SQL: &str = "CREATE TABLE IF NOT EXISTS memos (id uuid, title text indexed, content text indexed, tag_tokens text indexed, user_id string, updated_at bigint, version int) dict='keywords_32k'";

pub struct ManticoreClient {
    client: Client,
    base_url: String,
    initialized: OnceCell<()>,
}

impl ManticoreClient {
    pub fn new(uri: &str) -> AppResult<Self> {
        let client = Client::builder().build().map_err(|error| {
            AppError::DatabaseError(format!("Failed to create Manticore HTTP client: {error}"))
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
        self.execute_raw_sql(CREATE_TABLE_SQL, "table initialization")
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
                    "Manticore {operation} request failed: {error}"
                ))
            })?;

        let status = response.status();
        let result = response.json::<Value>().await.map_err(|error| {
            AppError::DatabaseError(format!(
                "Failed to parse Manticore {operation} response: {error}"
            ))
        })?;

        if !status.is_success() {
            return Err(AppError::DatabaseError(format!(
                "Manticore {operation} failed with status {status}: {result}"
            )));
        }

        let result_sets = result.as_array().ok_or_else(|| {
            AppError::DatabaseError(format!(
                "Invalid Manticore {operation} response format"
            ))
        })?;

        if let Some(error) = result_sets
            .iter()
            .filter_map(|result_set| result_set.get("error").and_then(Value::as_str))
            .find(|error| !error.is_empty())
        {
            return Err(AppError::DatabaseError(format!(
                "Manticore {operation} failed: {error}"
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
                AppError::DatabaseError(format!("Manticore {endpoint} request failed: {error}"))
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
        self.ensure_table().await?;

        let body = json!({
            "table": TABLE_NAME,
            "id": memo.id.to_string(),
            "doc": {
                "title": memo.title,
                "content": memo.content,
                "tag_tokens": encode_tag_tokens(&memo.tags),
                "user_id": memo.user_id.to_string(),
                "updated_at": memo.updated_at.timestamp_millis(),
                "version": memo.version
            }
        });

        self.post_json("replace", &body).await?;
        Ok(())
    }

    pub async fn search_memo_ids(
        &self,
        query: &str,
        tag: Option<String>,
        user_id: Uuid,
        page: usize,
        limit: usize,
    ) -> AppResult<MemoSearchHitPage> {
        self.ensure_table().await?;

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
            .ok_or_else(|| AppError::DatabaseError("Invalid Manticore search total".to_string()))
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
        let memo_ids = hits
            .iter()
            .map(|hit| {
                hit["_id"]
                    .as_str()
                    .ok_or_else(|| {
                        AppError::DatabaseError(
                            "Manticore search hit is missing UUID _id".to_string(),
                        )
                    })
                    .and_then(|value| {
                        Uuid::parse_str(value).map_err(|error| {
                            AppError::DatabaseError(format!(
                                "Invalid memo UUID in Manticore response: {error}"
                            ))
                        })
                    })
            })
            .collect::<AppResult<Vec<_>>>()?;

        Ok(MemoSearchHitPage { memo_ids, total })
    }

    pub async fn delete_memo(&self, id: Uuid) -> AppResult<()> {
        self.ensure_table().await?;

        let body = json!({
            "table": TABLE_NAME,
            "id": id.to_string()
        });
        self.post_json("delete", &body).await?;
        Ok(())
    }

    pub async fn health_check(&self) -> AppResult<bool> {
        self.execute_raw_sql("SELECT 1", "health check").await?;
        Ok(true)
    }
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

    async fn search_memo_ids(
        &self,
        query: &str,
        tag: Option<String>,
        user_id: Uuid,
        page: usize,
        limit: usize,
    ) -> AppResult<MemoSearchHitPage> {
        ManticoreClient::search_memo_ids(self, query, tag, user_id, page, limit).await
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
    fn maximum_unicode_tag_token_exceeds_regular_limit_but_fits_keywords_32k() {
        let token = encode_tag_token(&"🧊".repeat(64));

        assert_eq!(token.len(), 512);
        assert!(token.len() > 42);
        assert!(token.len() < 32 * 1024);
    }

    #[test]
    fn match_query_operators_are_escaped() {
        assert_eq!(
            escape_match_query(r#"hello | @title "memo" -draft \ archive"#),
            r#"hello \| \@title \"memo\" \-draft \\ archive"#
        );
    }
}
