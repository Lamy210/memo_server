use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Deserialize)]
pub struct CreateMemoDto {
    pub title: String,
    pub content: String,
    pub tags: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateMemoDto {
    pub title: Option<String>,
    pub content: Option<String>,
    pub tags: Option<Vec<String>>,
    pub version: i32,
}

#[derive(Debug, Serialize)]
pub struct MemoResponse {
    pub id: Uuid,
    pub title: String,
    pub content: String,
    pub tags: Vec<String>,
    pub user_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub version: i32,
}

#[derive(Debug, Serialize)]
pub struct SearchResponse {
    pub items: Vec<MemoResponse>,
    pub total: usize,
    pub page: usize,
    pub total_pages: usize,
}

impl From<crate::domain::memo::entity::Memo> for MemoResponse {
    fn from(memo: crate::domain::memo::entity::Memo) -> Self {
        Self {
            id: memo.id,
            title: memo.title,
            content: memo.content,
            tags: memo.tags,
            user_id: memo.user_id,
            created_at: memo.created_at,
            updated_at: memo.updated_at,
            version: memo.version,
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use serde_json::json;
    use uuid::Uuid;

    use super::{MemoListResponse, MemoResponse};

    #[test]
    fn memo_list_response_serializes_cursor_v1_contract() {
        let user_id = Uuid::new_v4();
        let memo_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();
        let now = Utc::now();
        let response = MemoListResponse {
            pagination: "cursor-v1",
            items: vec![MemoResponse {
                id: memo_id,
                title: "title".into(),
                content: "content".into(),
                tags: vec!["tag".into()],
                user_id,
                created_at: now,
                updated_at: now,
                version: 1,
            }],
            limit: 20,
            next_cursor: Some(format!("v1.{memo_id}")),
        };

        let value = serde_json::to_value(response).unwrap();
        assert_eq!(value["pagination"], json!("cursor-v1"));
        assert_eq!(value["limit"], json!(20));
        assert_eq!(value["next_cursor"], json!(format!("v1.{memo_id}")));
        assert_eq!(value["items"][0]["id"], json!(memo_id));
    }
}
