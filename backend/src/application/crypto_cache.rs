use std::time::Duration;

use async_trait::async_trait;
use uuid::Uuid;

use crate::{
    application::crypto::HighEncryptedMemoEnvelope,
    error::AppResult,
};

/// Ciphertext-only cache boundary for HIGH memos.
///
/// The application supplies only encrypted envelopes. Implementations must not
/// accept domain `Memo` values or plaintext-derived cache payloads.
#[async_trait]
pub trait HighEncryptedMemoCache: Send + Sync {
    async fn get_envelope(
        &self,
        owner_partition: Uuid,
        memo_id: Uuid,
    ) -> AppResult<Option<HighEncryptedMemoEnvelope>>;

    async fn set_envelope(
        &self,
        envelope: &HighEncryptedMemoEnvelope,
        expiration: Option<Duration>,
    ) -> AppResult<()>;

    async fn delete_envelope(&self, owner_partition: Uuid, memo_id: Uuid) -> AppResult<()>;

    async fn envelope_exists(&self, owner_partition: Uuid, memo_id: Uuid) -> AppResult<bool>;
}
