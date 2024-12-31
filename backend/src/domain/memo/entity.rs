// src/domain/memo/entity.rs
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use scylla::{
    frame::value::Value,
    serialize::{SerializeRow, CqlSerializeError},
    cql_to_rust::FromRowError,
    macros::FromRow,
};

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Memo {
    pub id: Uuid,
    pub title: String,
    pub content: String,
    pub tags: Vec<String>,
    pub user_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub version: i32,
}

impl Memo {
    pub fn new(title: String, content: String, tags: Vec<String>, user_id: Uuid) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            title,
            content,
            tags,
            user_id,
            created_at: now,
            updated_at: now,
            version: 1,
        }
    }

    pub fn update(&mut self, title: Option<String>, content: Option<String>, tags: Option<Vec<String>>) {
        if let Some(title) = title {
            self.title = title;
        }
        if let Some(content) = content {
            self.content = content;
        }
        if let Some(tags) = tags {
            self.tags = tags;
        }
        self.updated_at = Utc::now();
        self.version += 1;
    }

    pub fn validate(&self) -> bool {
        !self.title.trim().is_empty() 
            && !self.content.trim().is_empty()
            && self.tags.len() <= 10
            && self.tags.iter().all(|tag| !tag.trim().is_empty())
    }
}

impl SerializeRow for Memo {
    fn serialize<'b>(
        &self,
        buf: &'b mut Vec<u8>,
    ) -> Result<&'b mut Vec<u8>, CqlSerializeError> {
        self.id.serialize(buf)?;
        self.title.serialize(buf)?;
        self.content.serialize(buf)?;
        self.tags.serialize(buf)?;
        self.user_id.serialize(buf)?;
        self.created_at.timestamp_millis().serialize(buf)?;
        self.updated_at.timestamp_millis().serialize(buf)?;
        self.version.serialize(buf)?;
        Ok(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memo_creation() {
        let memo = Memo::new(
            "Test Title".to_string(),
            "Test Content".to_string(),
            vec!["test".to_string()],
            Uuid::new_v4(),
        );

        assert!(memo.validate());
        assert_eq!(memo.version, 1);
    }

    #[test]
    fn test_memo_update() {
        let mut memo = Memo::new(
            "Original Title".to_string(),
            "Original Content".to_string(),
            vec!["original".to_string()],
            Uuid::new_v4(),
        );

        let original_updated_at = memo.updated_at;
        std::thread::sleep(std::time::Duration::from_millis(1));

        memo.update(
            Some("New Title".to_string()),
            None,
            Some(vec!["new".to_string()]),
        );

        assert_eq!(memo.title, "New Title");
        assert_eq!(memo.content, "Original Content");
        assert_eq!(memo.tags, vec!["new"]);
        assert_eq!(memo.version, 2);
        assert!(memo.updated_at > original_updated_at);
    }
}