use crate::{
    application::{
        crypto::HighMemoCryptography,
        crypto_migration_batch::validate_page_size,
        crypto_search_rotation::{HighSearchOfflineWindowGuard, HighSearchOfflineWindowPermit},
        high_memo_routing::{
            HighMemoDataRoute, HighMemoDataRouteSnapshot, HighMemoPlaintextRetirementState,
        },
        high_search_routing::{HighSearchQueryRoute, HighSearchQueryRouteSnapshot},
    },
    config::{
        AppConfig, AuthoritativeBackend, HighMemoCryptoConfig, HighSearchConfig, SearchBackend,
    },
    error::{AppError, AppResult},
};

use super::{
    high_memo_aws_runtime::HighMemoStagingRuntimeHandle,
    high_search_aws_runtime::HighSearchRuntimeHandle,
    high_search_maintenance_mongodb::{
        HighSearchMaintenanceStatus, MongoHighSearchMaintenanceGuard,
        MongoHighSearchMaintenanceRecovery,
    },
    persistence::{
        manticore::ManticoreClient,
        mongodb::MongoDbAuthoritativeStore,
        ports::{HighEncryptedMemoIntegritySource, LegacyMemoPlaintextCacheMaintenance},
        redis::RedisCache,
    },
};

const MILLIS_PER_HOUR: i64 = 60 * 60 * 1000;
const MAX_SOAK_HOURS: u64 = 24 * 365;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HighMemoRetirementApproval {
    pub post_cutover_backup_verified: bool,
    pub restore_rehearsed: bool,
}

impl HighMemoRetirementApproval {
    fn validate(self) -> AppResult<()> {
        if !self.post_cutover_backup_verified {
            return Err(AppError::ValidationError(
                "MEMO-HIGH-1 retirement readiness requires a verified post-cutover backup".into(),
            ));
        }
        if !self.restore_rehearsed {
            return Err(AppError::ValidationError(
                "MEMO-HIGH-1 retirement readiness requires a restore rehearsal".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HighMemoRetirementStatus {
    pub memo_route: HighMemoDataRouteSnapshot,
    pub search_route: HighSearchQueryRouteSnapshot,
    pub plaintext_retirement_state: HighMemoPlaintextRetirementState,
    pub memo_route_changed_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HighMemoRetirementReadinessReport {
    pub memo_route: HighMemoDataRouteSnapshot,
    pub search_route: HighSearchQueryRouteSnapshot,
    pub plaintext_retirement_state: HighMemoPlaintextRetirementState,
    pub memo_route_changed_at_ms: i64,
    pub observed_at_ms: i64,
    pub minimum_soak_hours: u64,
    pub observed_soak_hours: u64,
    pub encrypted_memos_verified: u64,
    pub pending_projection_intents: u64,
    pub legacy_cache_keys: u64,
    pub legacy_search_documents: u64,
}

pub fn validate_high_memo_retirement_config(
    config: &AppConfig,
    minimum_soak_hours: u64,
    encrypted_page_size: usize,
    cache_scan_count: usize,
    expected_memo_generation: i64,
    expected_search_generation: i64,
    approval: HighMemoRetirementApproval,
) -> AppResult<()> {
    approval.validate()?;
    validate_soak_hours(minimum_soak_hours)?;
    validate_page_size(encrypted_page_size)?;
    validate_cache_scan_count(cache_scan_count)?;
    validate_generation("memo", expected_memo_generation)?;
    validate_generation("search", expected_search_generation)?;

    if config.authoritative_backend != AuthoritativeBackend::MongoDb {
        return Err(AppError::ServiceUnavailable(
            "MEMO-HIGH-1 retirement readiness requires MongoDB authoritative storage".into(),
        ));
    }
    if config.search_backend != SearchBackend::Manticore {
        return Err(AppError::ServiceUnavailable(
            "MEMO-HIGH-1 retirement readiness requires Manticore Search".into(),
        ));
    }
    if !matches!(config.high_memo_crypto, HighMemoCryptoConfig::AwsKms { .. }) {
        return Err(AppError::ServiceUnavailable(
            "MEMO-HIGH-1 retirement readiness requires HIGH_MEMO_CRYPTO_MODE=aws-kms".into(),
        ));
    }
    if !matches!(config.high_search, HighSearchConfig::AwsKms { .. }) {
        return Err(AppError::ServiceUnavailable(
            "MEMO-HIGH-1 retirement readiness requires protected HIGH search runtime".into(),
        ));
    }

    Ok(())
}

pub async fn inspect_high_memo_retirement_status(
    config: &AppConfig,
) -> AppResult<HighMemoRetirementStatus> {
    if config.authoritative_backend != AuthoritativeBackend::MongoDb {
        return Err(AppError::ServiceUnavailable(
            "MEMO-HIGH-1 retirement inspection requires MongoDB authoritative storage".into(),
        ));
    }

    let recovery = MongoHighSearchMaintenanceRecovery::connect(
        &config.authoritative_uri,
        &config.mongodb_database,
    )
    .await?;
    let status = recovery.inspect().await?;
    Ok(status_from_maintenance(&status))
}

struct RetirementVerificationResources<'a> {
    source: &'a MongoDbAuthoritativeStore,
    cache: &'a RedisCache,
    legacy_search: &'a ManticoreClient,
    recovery: &'a MongoHighSearchMaintenanceRecovery,
    cryptography: &'a dyn HighMemoCryptography,
}

#[derive(Debug, Clone, Copy)]
struct RetirementVerificationRequest {
    minimum_soak_hours: u64,
    encrypted_page_size: usize,
    cache_scan_count: usize,
    expected_memo_generation: i64,
    expected_search_generation: i64,
    expected_plaintext_retirement_state: HighMemoPlaintextRetirementState,
}

pub async fn verify_high_memo_retirement_readiness(
    config: &AppConfig,
    minimum_soak_hours: u64,
    encrypted_page_size: usize,
    cache_scan_count: usize,
    expected_memo_generation: i64,
    expected_search_generation: i64,
    approval: HighMemoRetirementApproval,
) -> AppResult<HighMemoRetirementReadinessReport> {
    validate_high_memo_retirement_config(
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

    let result = verify_high_memo_retirement_readiness_under_permit(
        config,
        &source,
        permit.as_ref(),
        minimum_soak_hours,
        encrypted_page_size,
        cache_scan_count,
        expected_memo_generation,
        expected_search_generation,
        approval,
        HighMemoPlaintextRetirementState::Available,
    )
    .await;

    finish_readiness_verification(result, permit).await
}

pub(crate) async fn verify_high_memo_retirement_readiness_under_permit(
    config: &AppConfig,
    source: &MongoDbAuthoritativeStore,
    permit: &dyn HighSearchOfflineWindowPermit,
    minimum_soak_hours: u64,
    encrypted_page_size: usize,
    cache_scan_count: usize,
    expected_memo_generation: i64,
    expected_search_generation: i64,
    approval: HighMemoRetirementApproval,
    expected_plaintext_retirement_state: HighMemoPlaintextRetirementState,
) -> AppResult<HighMemoRetirementReadinessReport> {
    validate_high_memo_retirement_config(
        config,
        minimum_soak_hours,
        encrypted_page_size,
        cache_scan_count,
        expected_memo_generation,
        expected_search_generation,
        approval,
    )?;

    // Preflight the entire configured historical/current KMS key ring and
    // protected search runtime before destructive plaintext work.
    let runtime = HighMemoStagingRuntimeHandle::build(&config.high_memo_crypto).await?;
    let cryptography = runtime.request_cryptography().ok_or_else(|| {
        AppError::ServiceUnavailable(
            "MEMO-HIGH-1 retirement readiness could not obtain the configured request cryptography runtime"
                .into(),
        )
    })?;
    let search_runtime =
        HighSearchRuntimeHandle::build(&config.high_search, &config.search_uri).await?;
    if search_runtime.query_reader().is_none() {
        return Err(AppError::ServiceUnavailable(
            "MEMO-HIGH-1 retirement readiness could not obtain the protected search runtime".into(),
        ));
    }

    let cache = RedisCache::new(&config.redis_uri)?;
    let legacy_search = ManticoreClient::new(&config.search_uri)?;
    let recovery = MongoHighSearchMaintenanceRecovery::connect(
        &config.authoritative_uri,
        &config.mongodb_database,
    )
    .await?;

    verify_under_permit(
        RetirementVerificationResources {
            source,
            cache: &cache,
            legacy_search: &legacy_search,
            recovery: &recovery,
            cryptography: cryptography.as_ref(),
        },
        permit,
        RetirementVerificationRequest {
            minimum_soak_hours,
            encrypted_page_size,
            cache_scan_count,
            expected_memo_generation,
            expected_search_generation,
            expected_plaintext_retirement_state,
        },
    )
    .await
}

async fn verify_under_permit(
    resources: RetirementVerificationResources<'_>,
    permit: &dyn HighSearchOfflineWindowPermit,
    request: RetirementVerificationRequest,
) -> AppResult<HighMemoRetirementReadinessReport> {
    let RetirementVerificationResources {
        source,
        cache,
        legacy_search,
        recovery,
        cryptography,
    } = resources;
    let RetirementVerificationRequest {
        minimum_soak_hours,
        encrypted_page_size,
        cache_scan_count,
        expected_memo_generation,
        expected_search_generation,
        expected_plaintext_retirement_state,
    } = request;

    permit.assert_still_enforced().await?;

    let memo_route = permit.current_memo_route().await?;
    let expected_memo = HighMemoDataRouteSnapshot {
        route: HighMemoDataRoute::Encrypted,
        generation: expected_memo_generation,
    };
    if memo_route != expected_memo {
        return Err(AppError::Conflict(format!(
            "MEMO-HIGH-1 retirement requires memo route encrypted generation {expected_memo_generation}; observed {} generation {}",
            memo_route.route, memo_route.generation
        )));
    }

    let search_route = permit.current_query_route().await?;
    let expected_search = HighSearchQueryRouteSnapshot {
        route: HighSearchQueryRoute::Protected,
        generation: expected_search_generation,
    };
    if search_route != expected_search {
        return Err(AppError::Conflict(format!(
            "MEMO-HIGH-1 retirement requires protected search generation {expected_search_generation}; observed {} generation {}",
            search_route.route, search_route.generation
        )));
    }

    let status = recovery.inspect().await?;
    if status.memo_route() != memo_route.route
        || status.memo_route_generation() != memo_route.generation
        || status.query_route() != search_route.route
        || status.query_route_generation() != search_route.generation
    {
        return Err(AppError::ServiceUnavailable(
            "MEMO-HIGH-1 retirement route status changed during maintenance verification".into(),
        ));
    }

    let plaintext_retirement_state = status.memo_plaintext_retirement_state();
    if plaintext_retirement_state != expected_plaintext_retirement_state {
        return Err(AppError::Conflict(format!(
            "MEMO-HIGH-1 retirement readiness requires plaintext retirement state {expected_plaintext_retirement_state}; observed {plaintext_retirement_state}"
        )));
    }

    let changed_at_ms = status.memo_route_changed_at_ms().ok_or_else(|| {
        AppError::Conflict(
            "MEMO-HIGH-1 encrypted route has no recorded transition timestamp; retirement cannot prove soak duration"
                .into(),
        )
    })?;
    let observed_at_ms = recovery.server_time_ms().await?;
    let observed_soak_hours =
        validate_and_measure_soak(changed_at_ms, observed_at_ms, minimum_soak_hours)?;

    let encrypted_memos_verified =
        verify_encrypted_authoritative_integrity(source, cryptography, encrypted_page_size).await?;

    let pending_projection_intents = source.count_projection_intents_for_cutover().await?;
    if pending_projection_intents != 0 {
        return Err(AppError::Conflict(format!(
            "MEMO-HIGH-1 retirement requires an empty projection outbox; {pending_projection_intents} intent(s) remain"
        )));
    }

    let legacy_cache = cache
        .inspect_legacy_plaintext_memo_cache(cache_scan_count)
        .await?;
    if legacy_cache.legacy_keys != 0 {
        return Err(AppError::Conflict(format!(
            "MEMO-HIGH-1 retirement found {} legacy plaintext memo cache key(s)",
            legacy_cache.legacy_keys
        )));
    }

    let legacy_search_documents = legacy_search.count_legacy_documents().await?;
    if legacy_search_documents != 0 {
        return Err(AppError::Conflict(format!(
            "MEMO-HIGH-1 retirement found {legacy_search_documents} legacy plaintext search document(s)"
        )));
    }

    permit.assert_still_enforced().await?;
    let final_memo = permit.current_memo_route().await?;
    let final_search = permit.current_query_route().await?;
    if final_memo != memo_route || final_search != search_route {
        return Err(AppError::ServiceUnavailable(
            "MEMO-HIGH-1 retirement route changed during final readiness revalidation".into(),
        ));
    }

    Ok(HighMemoRetirementReadinessReport {
        memo_route,
        search_route,
        plaintext_retirement_state,
        memo_route_changed_at_ms: changed_at_ms,
        observed_at_ms,
        minimum_soak_hours,
        observed_soak_hours,
        encrypted_memos_verified,
        pending_projection_intents,
        legacy_cache_keys: legacy_cache.legacy_keys,
        legacy_search_documents,
    })
}

async fn verify_encrypted_authoritative_integrity(
    source: &dyn HighEncryptedMemoIntegritySource,
    cryptography: &dyn HighMemoCryptography,
    page_size: usize,
) -> AppResult<u64> {
    validate_page_size(page_size)?;

    let expected_count = source.count_encrypted_memos_for_integrity().await?;
    let mut cursor = None;
    let mut verified = 0_u64;

    loop {
        let page = source
            .page_encrypted_memos_for_integrity(cursor, page_size)
            .await?;
        if page.is_empty() {
            break;
        }

        let mut previous = cursor;
        for envelope in page {
            if previous.is_some_and(|value| envelope.memo_id <= value) {
                return Err(AppError::DatabaseError(
                    "Encrypted memo integrity traversal did not advance monotonically".into(),
                ));
            }

            envelope.validate_structure()?;
            let memo = cryptography.decrypt_memo(&envelope).await?;
            if memo.id != envelope.memo_id
                || memo.user_id != envelope.owner_partition
                || memo.version != envelope.version
                || !memo.validate()
            {
                return Err(AppError::DatabaseError(
                    "Encrypted memo integrity verification found inconsistent decrypted identity/version"
                        .into(),
                ));
            }

            previous = Some(envelope.memo_id);
            cursor = previous;
            verified = verified.checked_add(1).ok_or_else(|| {
                AppError::DatabaseError("Encrypted memo integrity count overflow".into())
            })?;
        }
    }

    let final_count = source.count_encrypted_memos_for_integrity().await?;
    if verified != expected_count || final_count != expected_count {
        return Err(AppError::Conflict(format!(
            "Encrypted memo integrity traversal count mismatch: expected {expected_count}, verified {verified}, final {final_count}"
        )));
    }

    Ok(verified)
}

async fn finish_readiness_verification<T>(
    result: AppResult<T>,
    permit: Box<dyn HighSearchOfflineWindowPermit>,
) -> AppResult<T> {
    match (result, permit.release().await) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(primary), Ok(())) => Err(primary),
        (Ok(_), Err(release)) => Err(release),
        (Err(primary), Err(release)) => Err(AppError::ServiceUnavailable(format!(
            "MEMO-HIGH-1 retirement readiness failed and maintenance release also failed; primary={primary}; release={release}"
        ))),
    }
}

fn status_from_maintenance(status: &HighSearchMaintenanceStatus) -> HighMemoRetirementStatus {
    HighMemoRetirementStatus {
        memo_route: HighMemoDataRouteSnapshot {
            route: status.memo_route(),
            generation: status.memo_route_generation(),
        },
        search_route: HighSearchQueryRouteSnapshot {
            route: status.query_route(),
            generation: status.query_route_generation(),
        },
        plaintext_retirement_state: status.memo_plaintext_retirement_state(),
        memo_route_changed_at_ms: status.memo_route_changed_at_ms(),
    }
}

fn validate_and_measure_soak(
    changed_at_ms: i64,
    observed_at_ms: i64,
    minimum_soak_hours: u64,
) -> AppResult<u64> {
    let minimum_ms = i64::try_from(minimum_soak_hours)
        .ok()
        .and_then(|hours| hours.checked_mul(MILLIS_PER_HOUR))
        .ok_or_else(|| AppError::ValidationError("MEMO-HIGH-1 soak duration overflow".into()))?;
    let elapsed_ms = observed_at_ms.checked_sub(changed_at_ms).ok_or_else(|| {
        AppError::ServiceUnavailable(
            "MEMO-HIGH-1 route transition time is later than the verifier clock".into(),
        )
    })?;
    if elapsed_ms < 0 {
        return Err(AppError::ServiceUnavailable(
            "MEMO-HIGH-1 route transition time is later than the verifier clock".into(),
        ));
    }
    if elapsed_ms < minimum_ms {
        return Err(AppError::Conflict(format!(
            "MEMO-HIGH-1 retirement soak is incomplete: observed {} hour(s), required {minimum_soak_hours}",
            elapsed_ms / MILLIS_PER_HOUR
        )));
    }

    Ok((elapsed_ms / MILLIS_PER_HOUR) as u64)
}

fn validate_soak_hours(hours: u64) -> AppResult<()> {
    if !(1..=MAX_SOAK_HOURS).contains(&hours) {
        return Err(AppError::ValidationError(format!(
            "MEMO-HIGH-1 minimum soak hours must be in 1..={MAX_SOAK_HOURS}"
        )));
    }
    Ok(())
}

fn validate_cache_scan_count(scan_count: usize) -> AppResult<()> {
    if !(1..=10_000).contains(&scan_count) {
        return Err(AppError::ValidationError(
            "legacy memo cache scan_count must be in 1..=10000".into(),
        ));
    }
    Ok(())
}

fn validate_generation(label: &str, generation: i64) -> AppResult<()> {
    if generation < 0 {
        return Err(AppError::ValidationError(format!(
            "MEMO-HIGH-1 expected {label} route generation must be non-negative"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        application::crypto::{
            HighEncryptedMemoEnvelope, MEMO_HIGH_SCHEMA_VERSION, MEMO_HIGH_SUITE_ID,
        },
        config::{AuthConfig, HighSearchShadowConfig},
        domain::memo::entity::Memo,
    };
    use chrono::{TimeZone, Utc};
    use std::collections::BTreeMap;
    use uuid::Uuid;

    fn approved() -> HighMemoRetirementApproval {
        HighMemoRetirementApproval {
            post_cutover_backup_verified: true,
            restore_rehearsed: true,
        }
    }

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
    fn retirement_requires_operator_attestations_and_valid_bounds() {
        assert!(validate_high_memo_retirement_config(
            &configured(),
            24,
            500,
            1000,
            1,
            1,
            approved(),
        )
        .is_ok());

        let mut approval = approved();
        approval.post_cutover_backup_verified = false;
        assert!(
            validate_high_memo_retirement_config(&configured(), 24, 500, 1000, 1, 1, approval,)
                .is_err()
        );

        let mut approval = approved();
        approval.restore_rehearsed = false;
        assert!(
            validate_high_memo_retirement_config(&configured(), 24, 500, 1000, 1, 1, approval,)
                .is_err()
        );

        assert!(validate_high_memo_retirement_config(
            &configured(),
            0,
            500,
            1000,
            1,
            1,
            approved(),
        )
        .is_err());
        assert!(
            validate_high_memo_retirement_config(&configured(), 24, 500, 0, 1, 1, approved(),)
                .is_err()
        );
        assert!(
            validate_high_memo_retirement_config(&configured(), 24, 0, 1000, 1, 1, approved(),)
                .is_err()
        );
    }

    struct FakeIntegritySource {
        envelopes: Vec<HighEncryptedMemoEnvelope>,
        reported_count: u64,
    }

    #[async_trait::async_trait]
    impl HighEncryptedMemoIntegritySource for FakeIntegritySource {
        async fn count_encrypted_memos_for_integrity(&self) -> AppResult<u64> {
            Ok(self.reported_count)
        }

        async fn page_encrypted_memos_for_integrity(
            &self,
            after: Option<Uuid>,
            limit: usize,
        ) -> AppResult<Vec<HighEncryptedMemoEnvelope>> {
            let mut envelopes = self.envelopes.clone();
            envelopes.sort_by_key(|envelope| envelope.memo_id);
            Ok(envelopes
                .into_iter()
                .filter(|envelope| after.is_none_or(|cursor| envelope.memo_id > cursor))
                .take(limit)
                .collect())
        }
    }

    struct FakeIntegrityCrypto;

    #[async_trait::async_trait]
    impl HighMemoCryptography for FakeIntegrityCrypto {
        async fn encrypt_memo(&self, _memo: &Memo) -> AppResult<HighEncryptedMemoEnvelope> {
            Err(AppError::InternalServerError(
                "integrity test does not encrypt".into(),
            ))
        }

        async fn decrypt_memo(&self, envelope: &HighEncryptedMemoEnvelope) -> AppResult<Memo> {
            serde_json::from_slice(&envelope.ciphertext).map_err(|error| {
                AppError::DatabaseError(format!("integrity test decrypt failed: {error}"))
            })
        }
    }

    fn integrity_memo(id: u128) -> Memo {
        Memo {
            id: Uuid::from_u128(id),
            title: format!("memo-{id}"),
            content: "content".into(),
            tags: vec!["integrity".into()],
            user_id: Uuid::from_u128(10_000 + id),
            created_at: Utc.timestamp_millis_opt(1_700_000_000_000).unwrap(),
            updated_at: Utc.timestamp_millis_opt(1_700_000_001_000).unwrap(),
            version: 1,
        }
    }

    fn integrity_envelope(memo: &Memo) -> HighEncryptedMemoEnvelope {
        HighEncryptedMemoEnvelope {
            memo_id: memo.id,
            owner_partition: memo.user_id,
            ciphertext: serde_json::to_vec(memo).unwrap(),
            nonce: vec![0x11; 12],
            wrapped_dek: vec![0x22; 48],
            version: memo.version,
            crypto_suite_id: MEMO_HIGH_SUITE_ID.into(),
            key_version: "memo-key-v1".into(),
            schema_version: MEMO_HIGH_SCHEMA_VERSION,
        }
    }

    #[tokio::test]
    async fn encrypted_integrity_scan_decrypts_every_envelope_with_bounded_pages() {
        let first = integrity_memo(1);
        let second = integrity_memo(2);
        let source = FakeIntegritySource {
            envelopes: vec![integrity_envelope(&second), integrity_envelope(&first)],
            reported_count: 2,
        };

        assert_eq!(
            verify_encrypted_authoritative_integrity(&source, &FakeIntegrityCrypto, 1)
                .await
                .unwrap(),
            2
        );
    }

    #[tokio::test]
    async fn encrypted_integrity_scan_rejects_identity_or_count_mismatch() {
        let first = integrity_memo(1);
        let second = integrity_memo(2);
        let mut wrong_identity = integrity_envelope(&first);
        wrong_identity.ciphertext = serde_json::to_vec(&second).unwrap();

        let source = FakeIntegritySource {
            envelopes: vec![wrong_identity],
            reported_count: 1,
        };
        assert!(matches!(
            verify_encrypted_authoritative_integrity(&source, &FakeIntegrityCrypto, 1).await,
            Err(AppError::DatabaseError(_))
        ));

        let source = FakeIntegritySource {
            envelopes: vec![integrity_envelope(&first)],
            reported_count: 2,
        };
        assert!(matches!(
            verify_encrypted_authoritative_integrity(&source, &FakeIntegrityCrypto, 1).await,
            Err(AppError::Conflict(_))
        ));
    }

    #[test]
    fn soak_measurement_is_fail_closed_for_short_or_future_transition() {
        let changed = 1_700_000_000_000_i64;
        assert_eq!(
            validate_and_measure_soak(changed, changed + 48 * MILLIS_PER_HOUR, 24).unwrap(),
            48
        );
        assert!(matches!(
            validate_and_measure_soak(changed, changed + 23 * MILLIS_PER_HOUR, 24),
            Err(AppError::Conflict(_))
        ));
        assert!(matches!(
            validate_and_measure_soak(changed + 1, changed, 24),
            Err(AppError::ServiceUnavailable(_))
        ));
    }
}
