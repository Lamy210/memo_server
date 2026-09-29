use crate::{
    application::{
        crypto_search_rotation::HighSearchOfflineWindowGuard,
        high_memo_routing::HighMemoPlaintextRetirementState,
    },
    config::AppConfig,
    error::{AppError, AppResult},
};

use super::{
    high_memo_retirement_operator::{
        validate_high_memo_retirement_config, verify_high_memo_retirement_readiness_under_permit,
        HighMemoRetirementApproval, HighMemoRetirementReadinessReport,
        HighMemoRetirementReadinessRequest,
    },
    high_search_maintenance_mongodb::MongoHighSearchMaintenanceGuard,
    persistence::{
        mongodb::MongoDbAuthoritativeStore,
        ports::{
            LegacyMemoPlaintextRetirementAdmin, LegacyMemoPlaintextRetirementInspector,
        },
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HighMemoPlaintextRetirementRequest {
    pub minimum_soak_hours: u64,
    pub encrypted_page_size: usize,
    pub cache_scan_count: usize,
    pub expected_memo_generation: i64,
    pub expected_search_generation: i64,
    pub expected_plaintext_documents: u64,
    pub legacy_backup_retention_reviewed: bool,
    pub approval: HighMemoRetirementApproval,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HighMemoPlaintextRetirementReport {
    pub initial_retirement_state: HighMemoPlaintextRetirementState,
    pub final_retirement_state: HighMemoPlaintextRetirementState,
    pub observed_plaintext_documents: u64,
    pub deleted_plaintext_documents: u64,
    pub readiness: Option<HighMemoRetirementReadinessReport>,
}

pub fn validate_high_memo_plaintext_retirement_request(
    config: &AppConfig,
    request: HighMemoPlaintextRetirementRequest,
) -> AppResult<()> {
    validate_high_memo_retirement_config(
        config,
        request.minimum_soak_hours,
        request.encrypted_page_size,
        request.cache_scan_count,
        request.expected_memo_generation,
        request.expected_search_generation,
        request.approval,
    )?;

    if !request.legacy_backup_retention_reviewed {
        return Err(AppError::ValidationError(
            "MEMO-HIGH-1 destructive retirement requires review of legacy plaintext backup retention/disposal"
                .into(),
        ));
    }

    Ok(())
}

pub async fn retire_high_memo_plaintext(
    config: &AppConfig,
    request: HighMemoPlaintextRetirementRequest,
) -> AppResult<HighMemoPlaintextRetirementReport> {
    validate_high_memo_plaintext_retirement_request(config, request)?;

    let source =
        MongoDbAuthoritativeStore::new(&config.authoritative_uri, &config.mongodb_database).await?;
    let guard = MongoHighSearchMaintenanceGuard::new(source.database_handle()).await?;
    let permit = guard.acquire_offline_window().await?;

    let result = async {
        let initial_retirement_state = permit.current_plaintext_retirement_state().await?;

        if initial_retirement_state == HighMemoPlaintextRetirementState::Retired {
            let remaining = source.count_plaintext_memos_for_retirement().await?;
            if remaining != 0 {
                return Err(AppError::ServiceUnavailable(format!(
                    "MEMO-HIGH-1 retirement state is retired but {remaining} plaintext memo document(s) remain"
                )));
            }

            return Ok(HighMemoPlaintextRetirementReport {
                initial_retirement_state,
                final_retirement_state: HighMemoPlaintextRetirementState::Retired,
                observed_plaintext_documents: 0,
                deleted_plaintext_documents: 0,
                readiness: None,
            });
        }

        let readiness = verify_high_memo_retirement_readiness_under_permit(
            config,
            &source,
            permit.as_ref(),
            HighMemoRetirementReadinessRequest {
                minimum_soak_hours: request.minimum_soak_hours,
                encrypted_page_size: request.encrypted_page_size,
                cache_scan_count: request.cache_scan_count,
                expected_memo_generation: request.expected_memo_generation,
                expected_search_generation: request.expected_search_generation,
                approval: request.approval,
                expected_plaintext_retirement_state: initial_retirement_state,
            },
        )
        .await?;

        let observed_plaintext_documents = source.count_plaintext_memos_for_retirement().await?;
        if observed_plaintext_documents != request.expected_plaintext_documents {
            return Err(AppError::Conflict(format!(
                "MEMO-HIGH-1 plaintext retirement count changed since planning: expected {}, observed {observed_plaintext_documents}; rerun the non-destructive planner",
                request.expected_plaintext_documents
            )));
        }

        if initial_retirement_state == HighMemoPlaintextRetirementState::Available {
            let begun = permit.begin_plaintext_retirement().await?;
            if begun != HighMemoPlaintextRetirementState::InProgress {
                return Err(AppError::ServiceUnavailable(format!(
                    "MEMO-HIGH-1 plaintext retirement did not enter in_progress; observed {begun}"
                )));
            }
        }

        permit.assert_still_enforced().await?;
        let fenced_state = permit.current_plaintext_retirement_state().await?;
        if fenced_state != HighMemoPlaintextRetirementState::InProgress {
            return Err(AppError::ServiceUnavailable(format!(
                "MEMO-HIGH-1 plaintext retirement fence changed before deletion; observed {fenced_state}"
            )));
        }

        let deleted_plaintext_documents = source.delete_all_plaintext_memos_for_retirement().await?;
        let remaining_plaintext_documents = source.count_plaintext_memos_for_retirement().await?;

        if deleted_plaintext_documents != observed_plaintext_documents
            || remaining_plaintext_documents != 0
        {
            return Err(AppError::Conflict(format!(
                "MEMO-HIGH-1 plaintext retirement deletion did not converge exactly; observed_before={observed_plaintext_documents}; deleted={deleted_plaintext_documents}; remaining={remaining_plaintext_documents}; retirement remains in_progress"
            )));
        }

        permit.assert_still_enforced().await?;
        let state_before_finish = permit.current_plaintext_retirement_state().await?;
        if state_before_finish != HighMemoPlaintextRetirementState::InProgress {
            return Err(AppError::ServiceUnavailable(format!(
                "MEMO-HIGH-1 plaintext retirement state changed before completion; observed {state_before_finish}"
            )));
        }

        let final_retirement_state = permit.finish_plaintext_retirement().await?;
        if final_retirement_state != HighMemoPlaintextRetirementState::Retired {
            return Err(AppError::ServiceUnavailable(format!(
                "MEMO-HIGH-1 plaintext retirement did not reach retired; observed {final_retirement_state}"
            )));
        }

        let final_plaintext_documents = source.count_plaintext_memos_for_retirement().await?;
        if final_plaintext_documents != 0 {
            return Err(AppError::ServiceUnavailable(format!(
                "MEMO-HIGH-1 plaintext retirement reached retired but {final_plaintext_documents} plaintext memo document(s) remain"
            )));
        }
        permit.assert_still_enforced().await?;

        Ok(HighMemoPlaintextRetirementReport {
            initial_retirement_state,
            final_retirement_state,
            observed_plaintext_documents,
            deleted_plaintext_documents,
            readiness: Some(readiness),
        })
    }
    .await;

    finish_retirement(result, permit).await
}

async fn finish_retirement<T>(
    result: AppResult<T>,
    permit: Box<dyn crate::application::crypto_search_rotation::HighSearchOfflineWindowPermit>,
) -> AppResult<T> {
    match (result, permit.release().await) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(primary), Ok(())) => Err(primary),
        (Ok(_), Err(release)) => Err(release),
        (Err(primary), Err(release)) => Err(AppError::ServiceUnavailable(format!(
            "MEMO-HIGH-1 plaintext retirement failed and maintenance release also failed; primary={primary}; release={release}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        AuthConfig, AuthoritativeBackend, HighMemoCryptoConfig, HighSearchConfig,
        HighSearchShadowConfig, SearchBackend,
    };
    use std::collections::BTreeMap;

    fn configured() -> AppConfig {
        AppConfig {
            authoritative_backend: AuthoritativeBackend::MongoDb,
            authoritative_uri: "not-a-mongodb-uri".into(),
            mongodb_database: "memo_app".into(),
            redis_uri: "redis://unused".into(),
            search_backend: SearchBackend::Manticore,
            search_uri: "not-a-manticore-uri".into(),
            high_memo_crypto: HighMemoCryptoConfig::AwsKms {
                region: "ap-northeast-1".into(),
                active_key_version: "memo-key-v1".into(),
                key_versions: BTreeMap::from([(
                    "memo-key-v1".into(),
                    "arn:aws:kms:ap-northeast-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab"
                        .into(),
                )]),
            },
            high_search: HighSearchConfig::AwsKms {
                key_arn:
                    "arn:aws:kms:ap-northeast-1:111122223333:key/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
                        .into(),
                region: "ap-northeast-1".into(),
                provider_seed_version: "seed-v1".into(),
                cache_ttl_seconds: 60,
                cache_max_entries: 64,
                cache_sweep_seconds: 30,
                max_document_content_terms: 128,
                max_query_content_terms: 32,
                max_normalized_term_bytes: 128,
            },
            high_search_shadow: HighSearchShadowConfig::Disabled,
            port: 8080,
            auth: AuthConfig::Development,
        }
    }

    fn request() -> HighMemoPlaintextRetirementRequest {
        HighMemoPlaintextRetirementRequest {
            minimum_soak_hours: 168,
            encrypted_page_size: 500,
            cache_scan_count: 1000,
            expected_memo_generation: 5,
            expected_search_generation: 9,
            expected_plaintext_documents: 42,
            legacy_backup_retention_reviewed: true,
            approval: HighMemoRetirementApproval {
                post_cutover_backup_verified: true,
                restore_rehearsed: true,
            },
        }
    }

    #[test]
    fn destructive_request_reuses_all_readiness_validation() {
        assert!(validate_high_memo_plaintext_retirement_request(&configured(), request()).is_ok());

        let mut invalid = request();
        invalid.minimum_soak_hours = 0;
        assert!(validate_high_memo_plaintext_retirement_request(&configured(), invalid).is_err());

        let mut invalid = request();
        invalid.approval.restore_rehearsed = false;
        assert!(validate_high_memo_plaintext_retirement_request(&configured(), invalid).is_err());

        let mut invalid = request();
        invalid.legacy_backup_retention_reviewed = false;
        assert!(validate_high_memo_plaintext_retirement_request(&configured(), invalid).is_err());
    }
}
