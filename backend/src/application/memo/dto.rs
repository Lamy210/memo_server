use serde::{Deserialize, Serialize};
use uuid::Uuid;
use chrono::{DateTime, Utc};

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