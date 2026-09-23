use async_trait::async_trait;
use uuid::Uuid;

use crate::{
    application::crypto_search::{HighSearchToken, SEARCH_HIGH_SUITE_ID},
    error::{AppError, AppResult},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HighSearchProjectionDocument {
    pub memo_id: Uuid,
    pub owner_partition: Uuid,
    pub version: i32,
    pub analysis_version: String,
    pub search_key_version: String,
    pub content_tokens: Vec<HighSearchToken>,
    pub tag_tokens: Vec<HighSearchToken>,
}

impl HighSearchProjectionDocument {
    pub fn validate(&self) -> AppResult<()> {
        if self.version <= 0 {
            return Err(AppError::ValidationError(
                "HIGH search projection version must be positive".into(),
            ));
        }
        validate_analysis_version(&self.analysis_version)?;
        validate_key_version(&self.search_key_version)?;

        for token in self.content_tokens.iter().chain(&self.tag_tokens) {
            validate_projection_token(token, &self.search_key_version)?;
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HighSearchProjectionQuery {
    pub content_tokens: Vec<HighSearchToken>,
    pub tag_token: Option<HighSearchToken>,
    pub analysis_version: Option<String>,
    pub search_key_version: Option<String>,
}

impl HighSearchProjectionQuery {
    pub fn validate(&self) -> AppResult<()> {
        if let Some(analysis_version) = self.analysis_version.as_deref() {
            validate_analysis_version(analysis_version)?;
        }
        if let Some(key_version) = self.search_key_version.as_deref() {
            validate_key_version(key_version)?;
        }

        for token in self.content_tokens.iter().chain(self.tag_token.iter()) {
            token.validate()?;
            if let Some(key_version) = self.search_key_version.as_deref() {
                if token.key_version != key_version {
                    return Err(AppError::ValidationError(
                        "HIGH search query token key version does not match the query key version"
                            .into(),
                    ));
                }
            }
        }

        if !self.content_tokens.is_empty() || self.tag_token.is_some() {
            if self.analysis_version.is_none() {
                return Err(AppError::ValidationError(
                    "HIGH search query with tokens requires an analysis version".into(),
                ));
            }
            if self.search_key_version.is_none() {
                return Err(AppError::ValidationError(
                    "HIGH search query with tokens requires a search key version".into(),
                ));
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HighSearchProjectionPage {
    pub memo_ids: Vec<Uuid>,
    pub total: usize,
}

#[async_trait]
pub trait HighMemoSearchProjection: Send + Sync {
    async fn replace_document(&self, document: &HighSearchProjectionDocument) -> AppResult<()>;

    async fn search_memo_ids(
        &self,
        owner_partition: Uuid,
        query: &HighSearchProjectionQuery,
        page: usize,
        limit: usize,
    ) -> AppResult<HighSearchProjectionPage>;

    async fn delete_document(&self, owner_partition: Uuid, memo_id: Uuid) -> AppResult<()>;
}

fn validate_analysis_version(analysis_version: &str) -> AppResult<()> {
    if analysis_version.trim().is_empty() {
        return Err(AppError::ValidationError(
            "HIGH search analysis version must not be empty".into(),
        ));
    }
    Ok(())
}

fn validate_key_version(key_version: &str) -> AppResult<()> {
    if key_version.trim().is_empty() {
        return Err(AppError::ValidationError(
            "HIGH search key version must not be empty".into(),
        ));
    }
    Ok(())
}

fn validate_projection_token(token: &HighSearchToken, key_version: &str) -> AppResult<()> {
    token.validate()?;
    if token.suite_id != SEARCH_HIGH_SUITE_ID || token.key_version != key_version {
        return Err(AppError::ValidationError(
            "HIGH search projection tokens must use the document search suite and key version"
                .into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::crypto_search::SEARCH_HIGH_TOKEN_BYTES;

    fn token(key_version: &str, byte: &str) -> HighSearchToken {
        HighSearchToken {
            value: byte.repeat(SEARCH_HIGH_TOKEN_BYTES),
            key_version: key_version.into(),
            suite_id: SEARCH_HIGH_SUITE_ID.into(),
        }
    }

    #[test]
    fn document_requires_homogeneous_search_key_version() {
        let document = HighSearchProjectionDocument {
            memo_id: Uuid::new_v4(),
            owner_partition: Uuid::new_v4(),
            version: 1,
            analysis_version: "analysis-v1".into(),
            search_key_version: "search-v1".into(),
            content_tokens: vec![token("search-v1", "ab")],
            tag_tokens: vec![token("search-v2", "cd")],
        };

        assert!(document.validate().is_err());
    }

    #[test]
    fn tokenized_query_requires_explicit_analysis_and_key_versions() {
        let query = HighSearchProjectionQuery {
            content_tokens: vec![token("search-v1", "ab")],
            tag_token: None,
            analysis_version: None,
            search_key_version: None,
        };

        assert!(query.validate().is_err());
    }

    #[test]
    fn empty_query_can_span_current_projection_documents() {
        let query = HighSearchProjectionQuery {
            content_tokens: vec![],
            tag_token: None,
            analysis_version: None,
            search_key_version: None,
        };

        assert!(query.validate().is_ok());
    }
}
