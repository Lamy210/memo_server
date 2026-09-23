use std::fmt;

use async_trait::async_trait;
use uuid::Uuid;

use crate::error::{AppError, AppResult};

pub const SEARCH_HIGH_SUITE_ID: &str = "SEARCH-HIGH-1";
pub const SEARCH_HIGH_TOKEN_BYTES: usize = 48;
pub const SEARCH_HIGH_TOKEN_HEX_CHARS: usize = SEARCH_HIGH_TOKEN_BYTES * 2;

/// Opaque blind-index token suitable for a rebuildable HIGH search projection.
///
/// The token is sensitive derived metadata: Debug intentionally redacts its
/// value even though it is not plaintext memo content.
#[derive(Clone, PartialEq, Eq)]
pub struct HighSearchToken {
    pub value: String,
    pub key_version: String,
    pub suite_id: String,
}

impl HighSearchToken {
    pub fn validate(&self) -> AppResult<()> {
        if self.suite_id != SEARCH_HIGH_SUITE_ID {
            return Err(AppError::DatabaseError(format!(
                "Unknown HIGH search token suite: {}",
                self.suite_id
            )));
        }
        if self.key_version.trim().is_empty() {
            return Err(AppError::DatabaseError(
                "HIGH search token key version must not be empty".into(),
            ));
        }
        if self.value.len() != SEARCH_HIGH_TOKEN_HEX_CHARS
            || !self
                .value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(AppError::DatabaseError(
                "HIGH search token must be a SHA-384-sized hexadecimal value".into(),
            ));
        }
        Ok(())
    }
}

impl fmt::Debug for HighSearchToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HighSearchToken")
            .field("value", &"[REDACTED]")
            .field("key_version", &self.key_version)
            .field("suite_id", &self.suite_id)
            .finish()
    }
}

/// Cryptographic boundary for deterministic HIGH blind-index tokens.
///
/// Callers own tokenization/normalization policy. This boundary accepts exactly
/// one already-normalized term and binds it to the internal owner partition.
#[async_trait]
pub trait HighSearchTokenCryptography: Send + Sync {
    async fn derive_token(
        &self,
        owner_partition: Uuid,
        normalized_term: &str,
    ) -> AppResult<HighSearchToken>;
}

pub fn validate_normalized_search_term(term: &str) -> AppResult<()> {
    if term.is_empty() {
        return Err(AppError::ValidationError(
            "HIGH search token input must not be empty".into(),
        ));
    }
    if term.trim() != term {
        return Err(AppError::ValidationError(
            "HIGH search token input must be pre-normalized without surrounding whitespace".into(),
        ));
    }
    if term.contains('\0') {
        return Err(AppError::ValidationError(
            "HIGH search token input must not contain NUL".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_token_validation_is_strict() {
        let valid = HighSearchToken {
            value: "ab".repeat(SEARCH_HIGH_TOKEN_BYTES),
            key_version: "search-key-v1".into(),
            suite_id: SEARCH_HIGH_SUITE_ID.into(),
        };
        assert!(valid.validate().is_ok());

        let mut invalid = valid.clone();
        invalid.value.pop();
        assert!(invalid.validate().is_err());

        let mut invalid = valid.clone();
        invalid.value.replace_range(0..1, "z");
        assert!(invalid.validate().is_err());

        let mut invalid = valid.clone();
        invalid.value.replace_range(0..1, "A");
        assert!(invalid.validate().is_err());

        let mut invalid = valid;
        invalid.suite_id = "SEARCH-HIGH-UNKNOWN".into();
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn search_token_debug_redacts_value() {
        let token = HighSearchToken {
            value: "ab".repeat(SEARCH_HIGH_TOKEN_BYTES),
            key_version: "search-key-v1".into(),
            suite_id: SEARCH_HIGH_SUITE_ID.into(),
        };
        let debug = format!("{token:?}");

        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains(&token.value));
    }

    #[test]
    fn normalized_term_contract_rejects_ambiguous_input() {
        assert!(validate_normalized_search_term("memo").is_ok());
        assert!(validate_normalized_search_term("").is_err());
        assert!(validate_normalized_search_term(" memo").is_err());
        assert!(validate_normalized_search_term("memo ").is_err());
        assert!(validate_normalized_search_term("me\0mo").is_err());
    }
}
