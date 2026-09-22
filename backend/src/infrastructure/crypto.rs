use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{
    application::crypto::HighEncryptedMemoEnvelope,
    domain::memo::entity::Memo,
    error::{AppError, AppResult},
};

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HighMemoPayloadV1 {
    title: String,
    content: String,
    tags: Vec<String>,
    created_at_ms: i64,
    updated_at_ms: i64,
}

/// Serialize semantic memo data that must never remain in plaintext persistence.
///
/// Identity/routing/version fields are deliberately excluded because they are
/// authenticated as outer envelope metadata/AAD.
pub fn serialize_high_memo_payload(memo: &Memo) -> AppResult<Vec<u8>> {
    let payload = HighMemoPayloadV1 {
        title: memo.title.clone(),
        content: memo.content.clone(),
        tags: memo.tags.clone(),
        created_at_ms: memo.created_at.timestamp_millis(),
        updated_at_ms: memo.updated_at.timestamp_millis(),
    };

    serde_json::to_vec(&payload).map_err(|error| {
        AppError::InternalServerError(format!("serialize HIGH memo payload: {error}"))
    })
}

/// Recover a domain memo from authenticated envelope metadata plus decrypted payload bytes.
///
/// This codec performs structural validation only. Runtime crypto policy
/// (active/deployed/rejected suite state) remains the responsibility of the
/// cryptography implementation before plaintext is released to callers.
pub fn deserialize_high_memo_payload(
    envelope: &HighEncryptedMemoEnvelope,
    plaintext: &[u8],
) -> AppResult<Memo> {
    envelope.validate_structure()?;

    let payload: HighMemoPayloadV1 = serde_json::from_slice(plaintext).map_err(|error| {
        AppError::DatabaseError(format!("deserialize HIGH memo payload: {error}"))
    })?;

    let created_at = DateTime::<Utc>::from_timestamp_millis(payload.created_at_ms).ok_or_else(|| {
        AppError::DatabaseError(format!(
            "Invalid HIGH memo created_at milliseconds: {}",
            payload.created_at_ms
        ))
    })?;
    let updated_at = DateTime::<Utc>::from_timestamp_millis(payload.updated_at_ms).ok_or_else(|| {
        AppError::DatabaseError(format!(
            "Invalid HIGH memo updated_at milliseconds: {}",
            payload.updated_at_ms
        ))
    })?;

    if updated_at < created_at {
        return Err(AppError::DatabaseError(
            "HIGH memo updated_at must not precede created_at".into(),
        ));
    }

    let memo = Memo {
        id: envelope.memo_id,
        title: payload.title,
        content: payload.content,
        tags: payload.tags,
        user_id: envelope.owner_partition,
        created_at,
        updated_at,
        version: envelope.version,
    };

    if !memo.validate() {
        return Err(AppError::DatabaseError(
            "Decrypted HIGH memo payload violates domain invariants".into(),
        ));
    }

    Ok(memo)
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;
    use crate::application::crypto::{MEMO_HIGH_SCHEMA_VERSION, MEMO_HIGH_SUITE_ID};

    fn envelope_for(memo: &Memo) -> HighEncryptedMemoEnvelope {
        HighEncryptedMemoEnvelope {
            memo_id: memo.id,
            owner_partition: memo.user_id,
            ciphertext: vec![0xAA; 32],
            nonce: vec![0xBB; 12],
            wrapped_dek: vec![0xCC; 48],
            version: memo.version,
            crypto_suite_id: MEMO_HIGH_SUITE_ID.into(),
            key_version: "kms-key-v1".into(),
            schema_version: MEMO_HIGH_SCHEMA_VERSION,
        }
    }

    #[test]
    fn payload_round_trip_preserves_semantic_memo_fields() {
        let mut memo = Memo::new(
            "Encrypted title".into(),
            "Encrypted content".into(),
            vec!["secret".into(), "private".into()],
            Uuid::new_v4(),
        );
        memo.update(Some("Encrypted title v2".into()), None, None);

        let bytes = serialize_high_memo_payload(&memo).unwrap();
        let restored = deserialize_high_memo_payload(&envelope_for(&memo), &bytes).unwrap();

        assert_eq!(restored.id, memo.id);
        assert_eq!(restored.user_id, memo.user_id);
        assert_eq!(restored.title, memo.title);
        assert_eq!(restored.content, memo.content);
        assert_eq!(restored.tags, memo.tags);
        assert_eq!(restored.version, memo.version);
        assert_eq!(
            restored.created_at.timestamp_millis(),
            memo.created_at.timestamp_millis()
        );
        assert_eq!(
            restored.updated_at.timestamp_millis(),
            memo.updated_at.timestamp_millis()
        );
    }

    #[test]
    fn payload_excludes_outer_identity_and_version_metadata() {
        let memo = Memo::new(
            "title".into(),
            "content".into(),
            vec!["tag".into()],
            Uuid::new_v4(),
        );

        let bytes = serialize_high_memo_payload(&memo).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let object = json.as_object().unwrap();

        assert!(!object.contains_key("id"));
        assert!(!object.contains_key("memo_id"));
        assert!(!object.contains_key("user_id"));
        assert!(!object.contains_key("owner_partition"));
        assert!(!object.contains_key("version"));
        assert_eq!(object.get("title").unwrap(), "title");
        assert_eq!(object.get("content").unwrap(), "content");
    }

    #[test]
    fn payload_rejects_unknown_fields() {
        let memo = Memo::new("title".into(), "content".into(), vec![], Uuid::new_v4());
        let envelope = envelope_for(&memo);
        let invalid = br#"{
            "title":"title",
            "content":"content",
            "tags":[],
            "created_at_ms":0,
            "updated_at_ms":0,
            "unexpected":"field"
        }"#;

        assert!(matches!(
            deserialize_high_memo_payload(&envelope, invalid),
            Err(AppError::DatabaseError(_))
        ));
    }

    #[test]
    fn payload_rejects_domain_invalid_content() {
        let memo = Memo::new("title".into(), "content".into(), vec![], Uuid::new_v4());
        let envelope = envelope_for(&memo);
        let invalid = br#"{
            "title":"",
            "content":"content",
            "tags":[],
            "created_at_ms":0,
            "updated_at_ms":0
        }"#;

        assert!(matches!(
            deserialize_high_memo_payload(&envelope, invalid),
            Err(AppError::DatabaseError(_))
        ));
    }

    #[test]
    fn payload_rejects_timestamp_regression() {
        let memo = Memo::new("title".into(), "content".into(), vec![], Uuid::new_v4());
        let envelope = envelope_for(&memo);
        let invalid = br#"{
            "title":"title",
            "content":"content",
            "tags":[],
            "created_at_ms":2,
            "updated_at_ms":1
        }"#;

        assert!(matches!(
            deserialize_high_memo_payload(&envelope, invalid),
            Err(AppError::DatabaseError(_))
        ));
    }
}
