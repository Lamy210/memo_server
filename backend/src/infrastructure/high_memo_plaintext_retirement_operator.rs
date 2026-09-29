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
        mongodb::MongoDbAuthoritativeStore, ports::LegacyMemoPlaintextRetirementInspector,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HighMemoPlaintextRetirementPlan {
    pub retirement_state: HighMemoPlaintextRetirementState,
    pub plaintext_documents: u64,
    pub readiness: Option<HighMemoRetirementReadinessReport>,
}

pub fn validate_high_memo_plaintext_retirement_plan_config(
    config: &AppConfig,
    minimum_soak_hours: u64,
    encrypted_page_size: usize,
    cache_scan_count: usize,
    expected_memo_generation: i64,
    expected_search_generation: i64,
    approval: HighMemoRetirementApproval,
) -> AppResult<()> {
    validate_high_memo_retirement_config(
        config,
        minimum_soak_hours,
        encrypted_page_size,
        cache_scan_count,
        expected_memo_generation,
        expected_search_generation,
        approval,
    )
}

pub async fn plan_high_memo_plaintext_retirement(
    config: &AppConfig,
    minimum_soak_hours: u64,
    encrypted_page_size: usize,
    cache_scan_count: usize,
    expected_memo_generation: i64,
    expected_search_generation: i64,
    approval: HighMemoRetirementApproval,
) -> AppResult<HighMemoPlaintextRetirementPlan> {
    validate_high_memo_plaintext_retirement_plan_config(
        config,
        minimum_soak_hours,
        encrypted_page_size,
        cache_scan_count,
        expected_memo_generation,
        expected_search_generation,
        approval,
    )?;

    let source =
        MongoDbAuthoritativeStore::new(&config.authoritative_uri, &config.mongodb_database).await?;
    let guard = MongoHighSearchMaintenanceGuard::new(source.database_handle()).await?;
    let permit = guard.acquire_offline_window().await?;

    let result: AppResult<HighMemoPlaintextRetirementPlan> = async {
        let retirement_state = permit.current_plaintext_retirement_state().await?;

        match retirement_state {
            HighMemoPlaintextRetirementState::Retired => {
                let plaintext_documents = source.count_plaintext_memos_for_retirement().await?;
                if plaintext_documents != 0 {
                    Err(AppError::ServiceUnavailable(format!(
                        "MEMO-HIGH-1 retirement state is retired but {plaintext_documents} plaintext memo document(s) remain"
                    )))
                } else {
                    Ok(HighMemoPlaintextRetirementPlan {
                        retirement_state,
                        plaintext_documents,
                        readiness: None,
                    })
                }
            }
            HighMemoPlaintextRetirementState::Available
            | HighMemoPlaintextRetirementState::InProgress => {
                let readiness = verify_high_memo_retirement_readiness_under_permit(
                    config,
                    &source,
                    permit.as_ref(),
                    HighMemoRetirementReadinessRequest {
                        minimum_soak_hours,
                        encrypted_page_size,
                        cache_scan_count,
                        expected_memo_generation,
                        expected_search_generation,
                        approval,
                        expected_plaintext_retirement_state: retirement_state,
                    },
                )
                .await?;
                let plaintext_documents = source.count_plaintext_memos_for_retirement().await?;

                permit.assert_still_enforced().await?;
                let final_state = permit.current_plaintext_retirement_state().await?;
                if final_state != retirement_state {
                    return Err(AppError::ServiceUnavailable(format!(
                        "MEMO-HIGH-1 plaintext retirement state changed during planning: expected {retirement_state}, observed {final_state}"
                    )));
                }

                Ok(HighMemoPlaintextRetirementPlan {
                    retirement_state,
                    plaintext_documents,
                    readiness: Some(readiness),
                })
            }
        }
    }
    .await;

    finish_plan(result, permit).await
}

async fn finish_plan<T>(
    result: AppResult<T>,
    permit: Box<dyn crate::application::crypto_search_rotation::HighSearchOfflineWindowPermit>,
) -> AppResult<T> {
    match (result, permit.release().await) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(primary), Ok(())) => Err(primary),
        (Ok(_), Err(release)) => Err(release),
        (Err(primary), Err(release)) => Err(AppError::ServiceUnavailable(format!(
            "MEMO-HIGH-1 plaintext retirement planning failed and maintenance release also failed; primary={primary}; release={release}"
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

    #[test]
    fn planner_reuses_retirement_readiness_contract() {
        assert!(validate_high_memo_plaintext_retirement_plan_config(
            &configured(),
            24,
            500,
            1000,
            1,
            1,
            HighMemoRetirementApproval {
                post_cutover_backup_verified: true,
                restore_rehearsed: true,
            },
        )
        .is_ok());

        assert!(validate_high_memo_plaintext_retirement_plan_config(
            &configured(),
            0,
            500,
            1000,
            1,
            1,
            HighMemoRetirementApproval {
                post_cutover_backup_verified: true,
                restore_rehearsed: true,
            },
        )
        .is_err());
    }
}
