use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    domain::memo::entity::Memo,
    error::{AppError, AppResult},
};

#[async_trait]
pub trait HighMemoCryptography: Send + Sync {
    /// Protect one logical memo version using the active HIGH suite.
    ///
    /// Implementations own payload serialization, fresh per-version DEK
    /// generation, AEAD, and key wrapping. Callers decide when protection is
    /// required but do not depend on a KMS or cipher implementation.
    async fn encrypt_memo(&self, memo: &Memo) -> AppResult<HighEncryptedMemoEnvelope>;

    /// Recover one HIGH memo after validating its suite/schema/AAD contract.
    async fn decrypt_memo(&self, envelope: &HighEncryptedMemoEnvelope) -> AppResult<Memo>;
}

pub const MEMO_HIGH_SUITE_ID: &str = "MEMO-HIGH-1";
pub const MEMO_HIGH_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuiteLifecycle {
    Active,
    ReadOnly,
    Deprecated,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuiteImplementationStatus {
    Planned,
    Deployed,
    Retired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoCryptoSuite {
    pub id: &'static str,
    pub lifecycle: SuiteLifecycle,
    pub implementation_status: SuiteImplementationStatus,
    pub nonce_size_bytes: usize,
    pub tag_size_bytes: usize,
}

pub const MEMO_HIGH_1: MemoCryptoSuite = MemoCryptoSuite {
    id: MEMO_HIGH_SUITE_ID,
    lifecycle: SuiteLifecycle::Active,
    implementation_status: SuiteImplementationStatus::Planned,
    nonce_size_bytes: 12,
    tag_size_bytes: 16,
};

pub fn memo_crypto_suite(id: &str) -> Option<&'static MemoCryptoSuite> {
    match id {
        MEMO_HIGH_SUITE_ID => Some(&MEMO_HIGH_1),
        _ => None,
    }
}

pub fn require_read_suite(id: &str) -> AppResult<&'static MemoCryptoSuite> {
    let suite = known_suite(id)?;

    if suite.lifecycle == SuiteLifecycle::Rejected {
        return Err(AppError::DatabaseError(format!(
            "Rejected memo crypto suite: {id}"
        )));
    }

    if suite.implementation_status != SuiteImplementationStatus::Deployed {
        return Err(AppError::ServiceUnavailable(format!(
            "Memo crypto suite {id} is not deployed"
        )));
    }

    Ok(suite)
}

pub fn require_write_suite(id: &str) -> AppResult<&'static MemoCryptoSuite> {
    let suite = known_suite(id)?;

    if suite.lifecycle != SuiteLifecycle::Active {
        return Err(AppError::DatabaseError(format!(
            "Memo crypto suite {id} is not active for new writes"
        )));
    }

    if suite.implementation_status != SuiteImplementationStatus::Deployed {
        return Err(AppError::ServiceUnavailable(format!(
            "Memo crypto suite {id} is not deployed"
        )));
    }

    Ok(suite)
}

fn known_suite(id: &str) -> AppResult<&'static MemoCryptoSuite> {
    memo_crypto_suite(id)
        .ok_or_else(|| AppError::DatabaseError(format!("Unknown memo crypto suite: {id}")))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HighMemoAad {
    pub owner_partition: Uuid,
    pub memo_id: Uuid,
    pub version: i32,
    pub schema_version: u32,
    pub crypto_suite_id: String,
}

impl HighMemoAad {
    pub fn validate(&self) -> AppResult<&'static MemoCryptoSuite> {
        let suite = known_suite(&self.crypto_suite_id)?;

        if suite.lifecycle == SuiteLifecycle::Rejected {
            return Err(AppError::DatabaseError(format!(
                "Rejected memo crypto suite: {}",
                self.crypto_suite_id
            )));
        }
        if self.version <= 0 {
            return Err(AppError::DatabaseError(format!(
                "Encrypted memo version must be positive, got {}",
                self.version
            )));
        }
        if self.schema_version != MEMO_HIGH_SCHEMA_VERSION {
            return Err(AppError::DatabaseError(format!(
                "Unsupported encrypted memo schema version: {}",
                self.schema_version
            )));
        }

        Ok(suite)
    }

    /// Build the canonical associated-data encoding for HIGH memo payloads.
    ///
    /// This binds ciphertext to the fields required by the accepted security
    /// architecture without depending on JSON/BSON serialization details.
    pub fn encode(&self) -> AppResult<Vec<u8>> {
        self.validate()?;

        let suite_id = self.crypto_suite_id.as_bytes();
        let suite_id_len = u32::try_from(suite_id.len()).map_err(|_| {
            AppError::DatabaseError("Memo crypto suite identifier is too large".into())
        })?;

        let mut aad = Vec::with_capacity(64 + suite_id.len());
        aad.extend_from_slice(b"memo_server:high:aad:v1\0");
        aad.extend_from_slice(self.owner_partition.as_bytes());
        aad.extend_from_slice(self.memo_id.as_bytes());
        aad.extend_from_slice(&self.version.to_be_bytes());
        aad.extend_from_slice(&self.schema_version.to_be_bytes());
        aad.extend_from_slice(&suite_id_len.to_be_bytes());
        aad.extend_from_slice(suite_id);
        Ok(aad)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HighEncryptedMemoEnvelope {
    pub memo_id: Uuid,
    pub owner_partition: Uuid,
    /// AEAD ciphertext with the authentication tag appended.
    pub ciphertext: Vec<u8>,
    pub nonce: Vec<u8>,
    pub wrapped_dek: Vec<u8>,
    pub version: i32,
    pub crypto_suite_id: String,
    pub key_version: String,
    pub schema_version: u32,
}

impl From<&HighEncryptedMemoEnvelope> for HighMemoAad {
    fn from(envelope: &HighEncryptedMemoEnvelope) -> Self {
        Self {
            owner_partition: envelope.owner_partition,
            memo_id: envelope.memo_id,
            version: envelope.version,
            schema_version: envelope.schema_version,
            crypto_suite_id: envelope.crypto_suite_id.clone(),
        }
    }
}

impl HighEncryptedMemoEnvelope {
    pub fn validate_structure(&self) -> AppResult<&'static MemoCryptoSuite> {
        let suite = HighMemoAad::from(self).validate()?;

        if self.nonce.len() != suite.nonce_size_bytes {
            return Err(AppError::DatabaseError(format!(
                "Encrypted memo nonce length for {} must be {} bytes, got {}",
                suite.id,
                suite.nonce_size_bytes,
                self.nonce.len()
            )));
        }
        if self.ciphertext.len() < suite.tag_size_bytes {
            return Err(AppError::DatabaseError(format!(
                "Encrypted memo ciphertext for {} is shorter than its {}-byte authentication tag",
                suite.id, suite.tag_size_bytes
            )));
        }
        if self.wrapped_dek.is_empty() {
            return Err(AppError::DatabaseError(
                "Encrypted memo wrapped DEK must not be empty".into(),
            ));
        }
        if self.key_version.trim().is_empty() {
            return Err(AppError::DatabaseError(
                "Encrypted memo key version must not be empty".into(),
            ));
        }

        Ok(suite)
    }

    pub fn validate_for_read(&self) -> AppResult<&'static MemoCryptoSuite> {
        self.validate_structure()?;
        require_read_suite(&self.crypto_suite_id)
    }

    pub fn validate_for_new_write(&self) -> AppResult<&'static MemoCryptoSuite> {
        self.validate_structure()?;
        require_write_suite(&self.crypto_suite_id)
    }

    pub fn aad(&self) -> AppResult<Vec<u8>> {
        HighMemoAad::from(self).encode()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope() -> HighEncryptedMemoEnvelope {
        HighEncryptedMemoEnvelope {
            memo_id: Uuid::new_v4(),
            owner_partition: Uuid::new_v4(),
            ciphertext: vec![0xAA; 32],
            nonce: vec![0xBB; 12],
            wrapped_dek: vec![0xCC; 48],
            version: 1,
            crypto_suite_id: MEMO_HIGH_SUITE_ID.into(),
            key_version: "kms-key-v1".into(),
            schema_version: MEMO_HIGH_SCHEMA_VERSION,
        }
    }

    #[test]
    fn high_envelope_structure_matches_suite_contract() {
        let envelope = envelope();
        assert_eq!(envelope.validate_structure().unwrap(), &MEMO_HIGH_1);
    }

    #[test]
    fn planned_suite_is_not_runtime_eligible() {
        let envelope = envelope();

        assert!(matches!(
            envelope.validate_for_read(),
            Err(AppError::ServiceUnavailable(_))
        ));
        assert!(matches!(
            envelope.validate_for_new_write(),
            Err(AppError::ServiceUnavailable(_))
        ));
    }

    #[test]
    fn unknown_suite_fails_closed() {
        let mut envelope = envelope();
        envelope.crypto_suite_id = "MEMO-HIGH-UNKNOWN".into();

        assert!(matches!(
            envelope.validate_structure(),
            Err(AppError::DatabaseError(_))
        ));
    }

    #[test]
    fn unknown_schema_version_fails_closed() {
        let mut envelope = envelope();
        envelope.schema_version = MEMO_HIGH_SCHEMA_VERSION + 1;

        assert!(matches!(
            envelope.validate_structure(),
            Err(AppError::DatabaseError(_))
        ));
    }

    #[test]
    fn invalid_nonce_length_fails_closed() {
        let mut envelope = envelope();
        envelope.nonce.pop();

        assert!(matches!(
            envelope.validate_structure(),
            Err(AppError::DatabaseError(_))
        ));
    }

    #[test]
    fn aad_is_bound_to_owner_memo_version_schema_and_suite() {
        let base = envelope();
        let base_aad = base.aad().unwrap();

        let mut changed_owner = base.clone();
        changed_owner.owner_partition = Uuid::new_v4();
        assert_ne!(base_aad, changed_owner.aad().unwrap());

        let mut changed_memo = base.clone();
        changed_memo.memo_id = Uuid::new_v4();
        assert_ne!(base_aad, changed_memo.aad().unwrap());

        let mut changed_version = base.clone();
        changed_version.version += 1;
        assert_ne!(base_aad, changed_version.aad().unwrap());

        let mut changed_schema = base.clone();
        changed_schema.schema_version += 1;
        assert!(changed_schema.aad().is_err());

        assert!(base_aad
            .windows(MEMO_HIGH_SUITE_ID.len())
            .any(|window| window == MEMO_HIGH_SUITE_ID.as_bytes()));

        let mut changed_suite = base;
        changed_suite.crypto_suite_id = "MEMO-HIGH-2".into();
        assert!(changed_suite.aad().is_err());
    }
}
