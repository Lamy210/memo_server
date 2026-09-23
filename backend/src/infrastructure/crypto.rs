use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ring::{
    aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN},
    rand::{SecureRandom, SystemRandom},
};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::{
    application::{
        crypto::{
            require_read_suite, require_write_suite, HighEncryptedMemoEnvelope, HighMemoAad,
            HighMemoCryptography, MEMO_HIGH_SCHEMA_VERSION, MEMO_HIGH_SUITE_ID,
        },
        crypto_migration::HighMemoStagingCryptography,
    },
    domain::memo::entity::Memo,
    error::{AppError, AppResult},
    infrastructure::crypto_keys::{DataKeyProvider, GeneratedDataKey},
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

    let created_at =
        DateTime::<Utc>::from_timestamp_millis(payload.created_at_ms).ok_or_else(|| {
            AppError::DatabaseError(format!(
                "Invalid HIGH memo created_at milliseconds: {}",
                payload.created_at_ms
            ))
        })?;
    let updated_at =
        DateTime::<Utc>::from_timestamp_millis(payload.updated_at_ms).ok_or_else(|| {
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

// Staged until a production DataKeyProvider is configured and MEMO-HIGH-1 is deployed.
#[allow(dead_code)]
pub(super) struct RingHighMemoCryptography {
    data_keys: Arc<dyn DataKeyProvider>,
    rng: SystemRandom,
}

#[allow(dead_code)]
impl RingHighMemoCryptography {
    pub(super) fn new(data_keys: Arc<dyn DataKeyProvider>) -> Self {
        Self {
            data_keys,
            rng: SystemRandom::new(),
        }
    }

    async fn encrypt_memo_inner(&self, memo: &Memo) -> AppResult<HighEncryptedMemoEnvelope> {
        if !memo.validate() {
            return Err(AppError::ValidationError(
                "Memo violates domain invariants before HIGH encryption".into(),
            ));
        }

        let aad = HighMemoAad {
            owner_partition: memo.user_id,
            memo_id: memo.id,
            version: memo.version,
            schema_version: MEMO_HIGH_SCHEMA_VERSION,
            crypto_suite_id: MEMO_HIGH_SUITE_ID.into(),
        };
        let aad_bytes = aad.encode()?;

        let generated = self.data_keys.generate_data_key(&aad).await?;
        generated.validate()?;
        let GeneratedDataKey {
            plaintext,
            wrapped_dek,
            key_version,
        } = generated;

        let unbound = UnboundKey::new(&AES_256_GCM, plaintext.expose()).map_err(|_| {
            AppError::InternalServerError("Failed to initialize HIGH AES-256-GCM key".into())
        })?;
        let key = LessSafeKey::new(unbound);

        let mut nonce_bytes = [0u8; NONCE_LEN];
        self.rng.fill(&mut nonce_bytes).map_err(|_| {
            AppError::InternalServerError("Failed to generate HIGH AES-GCM nonce".into())
        })?;

        let mut buffer = Zeroizing::new(serialize_high_memo_payload(memo)?);
        key.seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce_bytes),
            Aad::from(aad_bytes.as_slice()),
            &mut *buffer,
        )
        .map_err(|_| AppError::InternalServerError("Failed to encrypt HIGH memo payload".into()))?;

        let envelope = HighEncryptedMemoEnvelope {
            memo_id: memo.id,
            owner_partition: memo.user_id,
            ciphertext: buffer.to_vec(),
            nonce: nonce_bytes.to_vec(),
            wrapped_dek,
            version: memo.version,
            crypto_suite_id: MEMO_HIGH_SUITE_ID.into(),
            key_version,
            schema_version: MEMO_HIGH_SCHEMA_VERSION,
        };
        envelope.validate_structure()?;
        Ok(envelope)
    }

    async fn decrypt_memo_inner(&self, envelope: &HighEncryptedMemoEnvelope) -> AppResult<Memo> {
        envelope.validate_structure()?;

        let aad = HighMemoAad::from(envelope);
        let aad_bytes = aad.encode()?;
        let plaintext_key = self
            .data_keys
            .unwrap_data_key(&envelope.wrapped_dek, &envelope.key_version, &aad)
            .await?;

        let unbound = UnboundKey::new(&AES_256_GCM, plaintext_key.expose()).map_err(|_| {
            AppError::InternalServerError("Failed to initialize HIGH AES-256-GCM key".into())
        })?;
        let key = LessSafeKey::new(unbound);

        let nonce_bytes: [u8; NONCE_LEN] = envelope
            .nonce
            .as_slice()
            .try_into()
            .map_err(|_| AppError::DatabaseError("Invalid HIGH AES-GCM nonce length".into()))?;

        let mut buffer = Zeroizing::new(envelope.ciphertext.clone());
        let plaintext = key
            .open_in_place(
                Nonce::assume_unique_for_key(nonce_bytes),
                Aad::from(aad_bytes.as_slice()),
                &mut buffer,
            )
            .map_err(|_| {
                AppError::DatabaseError(
                    "HIGH memo authentication failed; ciphertext was not released".into(),
                )
            })?;

        deserialize_high_memo_payload(envelope, plaintext)
    }
}

#[async_trait]
impl HighMemoStagingCryptography for RingHighMemoCryptography {
    async fn encrypt_for_staging(&self, memo: &Memo) -> AppResult<HighEncryptedMemoEnvelope> {
        self.encrypt_memo_inner(memo).await
    }

    async fn decrypt_staged(&self, envelope: &HighEncryptedMemoEnvelope) -> AppResult<Memo> {
        self.decrypt_memo_inner(envelope).await
    }
}

#[async_trait]
impl HighMemoCryptography for RingHighMemoCryptography {
    async fn encrypt_memo(&self, memo: &Memo) -> AppResult<HighEncryptedMemoEnvelope> {
        require_write_suite(MEMO_HIGH_SUITE_ID)?;
        self.encrypt_memo_inner(memo).await
    }

    async fn decrypt_memo(&self, envelope: &HighEncryptedMemoEnvelope) -> AppResult<Memo> {
        require_read_suite(&envelope.crypto_suite_id)?;
        self.decrypt_memo_inner(envelope).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU8, Ordering};

    use async_trait::async_trait;
    use uuid::Uuid;

    use super::*;
    use crate::application::crypto::{MEMO_HIGH_SCHEMA_VERSION, MEMO_HIGH_SUITE_ID};
    use crate::infrastructure::crypto_keys::{GeneratedDataKey, SecretDataKey, DATA_KEY_BYTES};

    struct TestDataKeyProvider {
        next_byte: AtomicU8,
    }

    impl TestDataKeyProvider {
        fn new() -> Self {
            Self {
                next_byte: AtomicU8::new(1),
            }
        }
    }

    #[async_trait]
    impl DataKeyProvider for TestDataKeyProvider {
        async fn generate_data_key(&self, _aad: &HighMemoAad) -> AppResult<GeneratedDataKey> {
            let byte = self.next_byte.fetch_add(1, Ordering::Relaxed);
            let key = [byte; DATA_KEY_BYTES];

            Ok(GeneratedDataKey {
                plaintext: SecretDataKey::new(key),
                wrapped_dek: key.to_vec(),
                key_version: "test-key-v1".into(),
            })
        }

        async fn unwrap_data_key(
            &self,
            wrapped_dek: &[u8],
            key_version: &str,
            _aad: &HighMemoAad,
        ) -> AppResult<SecretDataKey> {
            if key_version != "test-key-v1" {
                return Err(AppError::DatabaseError(
                    "Unexpected test key version".into(),
                ));
            }

            let key: [u8; DATA_KEY_BYTES] = wrapped_dek
                .try_into()
                .map_err(|_| AppError::DatabaseError("Invalid test wrapped DEK length".into()))?;
            Ok(SecretDataKey::new(key))
        }
    }

    fn cryptography() -> RingHighMemoCryptography {
        RingHighMemoCryptography::new(Arc::new(TestDataKeyProvider::new()))
    }

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

    #[tokio::test]
    async fn ring_encrypt_rejects_invalid_domain_state() {
        let cryptography = cryptography();
        let memo = Memo::new("".into(), "content".into(), vec![], Uuid::new_v4());

        assert!(matches!(
            cryptography.encrypt_memo_inner(&memo).await,
            Err(AppError::ValidationError(_))
        ));
    }

    #[tokio::test]
    async fn planned_suite_is_available_only_to_non_authoritative_staging() {
        let cryptography = cryptography();
        let memo = Memo::new("title".into(), "content".into(), vec![], Uuid::new_v4());

        let envelope = cryptography.encrypt_for_staging(&memo).await.unwrap();
        let restored = cryptography.decrypt_staged(&envelope).await.unwrap();

        assert_eq!(restored.id, memo.id);
        assert_eq!(restored.title, memo.title);
        assert!(matches!(
            cryptography.encrypt_memo(&memo).await,
            Err(AppError::ServiceUnavailable(_))
        ));
    }

    #[tokio::test]
    async fn planned_suite_blocks_public_runtime_encryption() {
        let cryptography = cryptography();
        let memo = Memo::new("title".into(), "content".into(), vec![], Uuid::new_v4());

        assert!(matches!(
            cryptography.encrypt_memo(&memo).await,
            Err(AppError::ServiceUnavailable(_))
        ));
    }

    #[tokio::test]
    async fn ring_aead_round_trip_uses_fresh_dek_and_nonce() {
        let cryptography = cryptography();
        let memo = Memo::new(
            "Encrypted title".into(),
            "Encrypted content".into(),
            vec!["secret".into()],
            Uuid::new_v4(),
        );

        let first = cryptography.encrypt_memo_inner(&memo).await.unwrap();
        let second = cryptography.encrypt_memo_inner(&memo).await.unwrap();

        assert_ne!(first.wrapped_dek, second.wrapped_dek);
        assert_ne!(first.nonce, second.nonce);
        assert_ne!(first.ciphertext, second.ciphertext);

        let restored = cryptography.decrypt_memo_inner(&first).await.unwrap();
        assert_eq!(restored.id, memo.id);
        assert_eq!(restored.user_id, memo.user_id);
        assert_eq!(restored.title, memo.title);
        assert_eq!(restored.content, memo.content);
        assert_eq!(restored.tags, memo.tags);
        assert_eq!(restored.version, memo.version);
    }

    async fn assert_tampered_envelope_rejected(
        cryptography: &RingHighMemoCryptography,
        envelope: HighEncryptedMemoEnvelope,
    ) {
        assert!(matches!(
            cryptography.decrypt_memo_inner(&envelope).await,
            Err(AppError::DatabaseError(_))
        ));
    }

    #[tokio::test]
    async fn ring_aead_rejects_ciphertext_mutation() {
        let cryptography = cryptography();
        let memo = Memo::new("title".into(), "content".into(), vec![], Uuid::new_v4());
        let mut envelope = cryptography.encrypt_memo_inner(&memo).await.unwrap();

        envelope.ciphertext[0] ^= 0x01;

        assert_tampered_envelope_rejected(&cryptography, envelope).await;
    }

    #[tokio::test]
    async fn ring_aead_rejects_owner_substitution() {
        let cryptography = cryptography();
        let memo = Memo::new("title".into(), "content".into(), vec![], Uuid::new_v4());
        let mut envelope = cryptography.encrypt_memo_inner(&memo).await.unwrap();

        envelope.owner_partition = Uuid::new_v4();

        assert_tampered_envelope_rejected(&cryptography, envelope).await;
    }

    #[tokio::test]
    async fn ring_aead_rejects_memo_id_substitution() {
        let cryptography = cryptography();
        let memo = Memo::new("title".into(), "content".into(), vec![], Uuid::new_v4());
        let mut envelope = cryptography.encrypt_memo_inner(&memo).await.unwrap();

        envelope.memo_id = Uuid::new_v4();

        assert_tampered_envelope_rejected(&cryptography, envelope).await;
    }

    #[tokio::test]
    async fn ring_aead_rejects_version_substitution() {
        let cryptography = cryptography();
        let memo = Memo::new("title".into(), "content".into(), vec![], Uuid::new_v4());
        let mut envelope = cryptography.encrypt_memo_inner(&memo).await.unwrap();

        envelope.version += 1;

        assert_tampered_envelope_rejected(&cryptography, envelope).await;
    }

    #[tokio::test]
    async fn ring_aead_rejects_nonce_mutation() {
        let cryptography = cryptography();
        let memo = Memo::new("title".into(), "content".into(), vec![], Uuid::new_v4());
        let mut envelope = cryptography.encrypt_memo_inner(&memo).await.unwrap();

        envelope.nonce[0] ^= 0x01;

        assert_tampered_envelope_rejected(&cryptography, envelope).await;
    }
}
