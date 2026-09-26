// Staged search-key boundary. This remains runtime-unreachable until
// SEARCH-HIGH-1 is deployed and protected Manticore projection is wired.
#![allow(dead_code)]

use std::{
    collections::HashMap,
    fmt,
    sync::Arc,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use ring::hkdf;
use tokio::sync::Mutex;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::{
    application::crypto_search::{search_version_identifier_is_valid, MAX_SEARCH_VERSION_ID_CHARS},
    error::{AppError, AppResult},
};

pub(super) const SEARCH_KEY_BYTES: usize = 48;
pub(super) const SEARCH_KEY_SEED_BYTES: usize = 48;
const SEARCH_KEY_DERIVATION_VERSION: &str = "hkdf384-v1";
const SEARCH_KEY_DERIVATION_SALT: &[u8] = b"memo_server:search:root:v1\0";
const SEARCH_USER_KEY_INFO: &[u8] = b"memo_server:search:user-key:v1\0";

pub(super) struct SecretSearchKeySeed(Zeroizing<[u8; SEARCH_KEY_SEED_BYTES]>);

impl SecretSearchKeySeed {
    pub(super) fn new(bytes: [u8; SEARCH_KEY_SEED_BYTES]) -> Self {
        Self(Zeroizing::new(bytes))
    }

    pub(super) fn expose(&self) -> &[u8; SEARCH_KEY_SEED_BYTES] {
        &self.0
    }
}

impl fmt::Debug for SecretSearchKeySeed {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretSearchKeySeed([REDACTED])")
    }
}

pub(super) struct ResolvedSearchKeySeed {
    pub(super) plaintext: SecretSearchKeySeed,
    pub(super) key_version: String,
}

impl ResolvedSearchKeySeed {
    fn validate(&self) -> AppResult<()> {
        if !search_version_identifier_is_valid(&self.key_version) {
            return Err(AppError::ServiceUnavailable(format!(
                "Search-key seed provider key version must be 1..={MAX_SEARCH_VERSION_ID_CHARS} ASCII identifier characters"
            )));
        }
        Ok(())
    }
}

impl fmt::Debug for ResolvedSearchKeySeed {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedSearchKeySeed")
            .field("plaintext", &self.plaintext)
            .field("key_version", &self.key_version)
            .finish()
    }
}

#[async_trait]
pub(super) trait SearchKeySeedProvider: Send + Sync {
    /// Resolve one owner-scoped, versioned 384-bit seed for HIGH search.
    ///
    /// A production managed-KMS implementation may derive this seed with a
    /// provider-side HMAC/PRF operation. Long-lived root key material must not
    /// be returned to memo_server, stored in source control, or placed in
    /// ordinary application configuration.
    ///
    /// For a given owner and key_version, returned seed bytes must remain
    /// stable. Any seed-material change is a key rotation and must return a new
    /// application-owned key_version so projection/query generations cannot
    /// silently diverge.
    async fn resolve_search_seed(&self, owner_partition: Uuid) -> AppResult<ResolvedSearchKeySeed>;
}

pub(super) struct SecretSearchKey(Zeroizing<[u8; SEARCH_KEY_BYTES]>);

impl SecretSearchKey {
    pub(super) fn new(bytes: [u8; SEARCH_KEY_BYTES]) -> Self {
        Self(Zeroizing::new(bytes))
    }

    pub(super) fn expose(&self) -> &[u8; SEARCH_KEY_BYTES] {
        &self.0
    }

    fn duplicate(&self) -> Self {
        let mut bytes = [0u8; SEARCH_KEY_BYTES];
        bytes.copy_from_slice(self.expose());
        Self::new(bytes)
    }
}

impl fmt::Debug for SecretSearchKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretSearchKey([REDACTED])")
    }
}

pub(super) struct ResolvedSearchKey {
    pub(super) plaintext: SecretSearchKey,
    pub(super) key_version: String,
}

impl ResolvedSearchKey {
    pub(super) fn validate(&self) -> AppResult<()> {
        if !search_version_identifier_is_valid(&self.key_version) {
            return Err(AppError::ServiceUnavailable(format!(
                "Search-key provider key version must be 1..={MAX_SEARCH_VERSION_ID_CHARS} ASCII identifier characters"
            )));
        }
        Ok(())
    }
}

impl fmt::Debug for ResolvedSearchKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedSearchKey")
            .field("plaintext", &self.plaintext)
            .field("key_version", &self.key_version)
            .finish()
    }
}

#[async_trait]
pub(super) trait SearchKeyProvider: Send + Sync {
    /// Resolve the independent per-user HIGH search key.
    ///
    /// Implementations must not reuse memo-encryption DEKs or return a key
    /// shared across owner partitions.
    async fn resolve_search_key(&self, owner_partition: Uuid) -> AppResult<ResolvedSearchKey>;
}

/// Deterministically derives one 384-bit search key per owner partition from a
/// provider-resolved owner-scoped seed using HKDF-SHA-384.
///
/// This keeps blind tokens stable for the same owner/seed version and adds
/// local protocol/domain separation after the external key boundary. A future
/// managed-KMS adapter can produce the seed with a provider-side HMAC/PRF
/// operation, keeping long-lived root key material outside memo_server.
pub(super) struct HkdfSearchKeyProvider {
    seeds: Arc<dyn SearchKeySeedProvider>,
}

impl HkdfSearchKeyProvider {
    pub(super) fn new(seeds: Arc<dyn SearchKeySeedProvider>) -> Self {
        Self { seeds }
    }
}

#[async_trait]
impl SearchKeyProvider for HkdfSearchKeyProvider {
    async fn resolve_search_key(&self, owner_partition: Uuid) -> AppResult<ResolvedSearchKey> {
        let seed = self.seeds.resolve_search_seed(owner_partition).await?;
        seed.validate()?;

        let key_version = format!("{SEARCH_KEY_DERIVATION_VERSION}:{}", seed.key_version);
        if !search_version_identifier_is_valid(&key_version) {
            return Err(AppError::ServiceUnavailable(format!(
                "Derived search-key version must be 1..={MAX_SEARCH_VERSION_ID_CHARS} ASCII identifier characters"
            )));
        }

        let salt = hkdf::Salt::new(hkdf::HKDF_SHA384, SEARCH_KEY_DERIVATION_SALT);
        let prk = salt.extract(seed.plaintext.expose());
        let owner: &[u8] = owner_partition.as_bytes();
        let info = [SEARCH_USER_KEY_INFO, key_version.as_bytes(), owner];
        let okm = prk.expand(&info, hkdf::HKDF_SHA384).map_err(|_| {
            AppError::InternalServerError("Failed to derive HIGH per-user search key".into())
        })?;

        let mut key_bytes = [0u8; SEARCH_KEY_BYTES];
        okm.fill(&mut key_bytes).map_err(|_| {
            AppError::InternalServerError("Failed to materialize HIGH per-user search key".into())
        })?;

        let resolved = ResolvedSearchKey {
            plaintext: SecretSearchKey::new(key_bytes),
            key_version,
        };
        resolved.validate()?;
        Ok(resolved)
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct SearchKeyCachePolicy {
    ttl: Duration,
    max_entries: usize,
}

impl SearchKeyCachePolicy {
    pub(super) fn new(ttl: Duration, max_entries: usize) -> AppResult<Self> {
        if ttl.is_zero() {
            return Err(AppError::ServiceUnavailable(
                "HIGH search-key cache TTL must be greater than zero".into(),
            ));
        }
        if max_entries == 0 {
            return Err(AppError::ServiceUnavailable(
                "HIGH search-key cache max_entries must be greater than zero".into(),
            ));
        }
        Ok(Self { ttl, max_entries })
    }
}

struct CachedSearchKey {
    plaintext: SecretSearchKey,
    key_version: String,
    expires_at: Instant,
}

impl CachedSearchKey {
    fn from_resolved(resolved: &ResolvedSearchKey, expires_at: Instant) -> Self {
        Self {
            plaintext: resolved.plaintext.duplicate(),
            key_version: resolved.key_version.clone(),
            expires_at,
        }
    }

    fn to_resolved(&self) -> ResolvedSearchKey {
        ResolvedSearchKey {
            plaintext: self.plaintext.duplicate(),
            key_version: self.key_version.clone(),
        }
    }
}

/// Bounded in-process cache for final owner-scoped HIGH search keys.
///
/// Only the derived search key is cached. Root/seed material remains behind the
/// wrapped provider. Expiry and eviction drop zeroizing key storage.
struct SearchKeyCacheState {
    entries: HashMap<Uuid, CachedSearchKey>,
    resolution_gates: HashMap<Uuid, Arc<Mutex<()>>>,
    epoch: u64,
}

pub(super) struct CachingSearchKeyProvider {
    inner: Arc<dyn SearchKeyProvider>,
    policy: SearchKeyCachePolicy,
    state: Mutex<SearchKeyCacheState>,
}

impl CachingSearchKeyProvider {
    pub(super) fn new(inner: Arc<dyn SearchKeyProvider>, policy: SearchKeyCachePolicy) -> Self {
        Self {
            inner,
            policy,
            state: Mutex::new(SearchKeyCacheState {
                entries: HashMap::new(),
                resolution_gates: HashMap::new(),
                epoch: 0,
            }),
        }
    }

    pub(super) async fn invalidate(&self, owner_partition: Uuid) {
        let mut state = self.state.lock().await;
        state.epoch = state.epoch.wrapping_add(1);
        state.entries.remove(&owner_partition);
    }

    pub(super) async fn clear(&self) {
        let mut state = self.state.lock().await;
        state.epoch = state.epoch.wrapping_add(1);
        state.entries.clear();
    }

    /// Remove entries whose reuse TTL has elapsed.
    ///
    /// Runtime wiring may call this from a periodic maintenance task when the
    /// deployment requires tighter memory-residency cleanup than lazy access
    /// purging alone provides.
    pub(super) async fn purge_expired_entries(&self) {
        let mut state = self.state.lock().await;
        Self::purge_expired(&mut state.entries, Instant::now());
    }

    async fn resolve_at(
        &self,
        owner_partition: Uuid,
        now: Instant,
    ) -> AppResult<ResolvedSearchKey> {
        let (observed_epoch, resolution_gate) = {
            let mut state = self.state.lock().await;
            Self::purge_expired(&mut state.entries, now);
            if let Some(cached) = state.entries.get(&owner_partition) {
                return Ok(cached.to_resolved());
            }

            let observed_epoch = state.epoch;
            let resolution_gate = state
                .resolution_gates
                .entry(owner_partition)
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone();
            (observed_epoch, resolution_gate)
        };

        // Single-flight only the same owner. The global cache-state mutex is
        // never held across provider I/O, so unrelated owners still resolve in
        // parallel. Tokio's mutex guard is cancellation-safe: a cancelled
        // leader releases the owner gate and lets the next waiter continue.
        let resolution_guard = resolution_gate.lock().await;

        let result = async {
            let resolution_now = std::cmp::max(now, Instant::now());
            {
                let mut state = self.state.lock().await;
                Self::purge_expired(&mut state.entries, resolution_now);

                // The request began before an invalidate/clear generation
                // fence. Even if another waiter has since populated a key, a
                // pre-fence request must fail and retry under the new epoch.
                if state.epoch != observed_epoch {
                    return Err(AppError::ServiceUnavailable(
                        "HIGH search-key cache changed during key resolution; retry".into(),
                    ));
                }

                if let Some(cached) = state.entries.get(&owner_partition) {
                    return Ok(cached.to_resolved());
                }
            }

            let resolved = self.inner.resolve_search_key(owner_partition).await?;
            resolved.validate()?;

            let resolved_at = std::cmp::max(now, Instant::now());
            let expires_at = resolved_at.checked_add(self.policy.ttl).ok_or_else(|| {
                AppError::ServiceUnavailable("HIGH search-key cache TTL overflow".into())
            })?;

            let mut state = self.state.lock().await;
            Self::purge_expired(&mut state.entries, resolved_at);

            // Rotation/deployment invalidation is a generation fence. Never
            // return or reinsert key material resolved across an
            // invalidate/clear event.
            if state.epoch != observed_epoch {
                return Err(AppError::ServiceUnavailable(
                    "HIGH search-key cache changed during key resolution; retry".into(),
                ));
            }

            if let Some(cached) = state.entries.get(&owner_partition) {
                return Ok(cached.to_resolved());
            }

            if state.entries.len() >= self.policy.max_entries {
                Self::evict_earliest_expiring(&mut state.entries);
            }
            state.entries.insert(
                owner_partition,
                CachedSearchKey::from_resolved(&resolved, expires_at),
            );
            Ok(resolved)
        }
        .await;

        drop(resolution_guard);
        self.prune_resolution_gate(owner_partition, &resolution_gate)
            .await;
        result
    }

    async fn prune_resolution_gate(&self, owner_partition: Uuid, resolution_gate: &Arc<Mutex<()>>) {
        let mut state = self.state.lock().await;
        let should_remove = state
            .resolution_gates
            .get(&owner_partition)
            .is_some_and(|current| {
                Arc::ptr_eq(current, resolution_gate) && Arc::strong_count(current) == 2
            });
        if should_remove {
            state.resolution_gates.remove(&owner_partition);
        }
    }

    fn purge_expired(entries: &mut HashMap<Uuid, CachedSearchKey>, now: Instant) {
        entries.retain(|_, cached| cached.expires_at > now);
    }

    fn evict_earliest_expiring(entries: &mut HashMap<Uuid, CachedSearchKey>) {
        let victim = entries
            .iter()
            .min_by_key(|(_, cached)| cached.expires_at)
            .map(|(owner, _)| *owner);
        if let Some(owner) = victim {
            entries.remove(&owner);
        }
    }
}

#[async_trait]
impl SearchKeyProvider for CachingSearchKeyProvider {
    async fn resolve_search_key(&self, owner_partition: Uuid) -> AppResult<ResolvedSearchKey> {
        self.resolve_at(owner_partition, Instant::now()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct BlockingResolvedKeyProvider {
        calls: std::sync::atomic::AtomicUsize,
        started: tokio::sync::Notify,
        release: tokio::sync::Notify,
    }

    #[async_trait]
    impl SearchKeyProvider for BlockingResolvedKeyProvider {
        async fn resolve_search_key(&self, owner_partition: Uuid) -> AppResult<ResolvedSearchKey> {
            self.calls
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.started.notify_one();
            self.release.notified().await;

            let mut bytes = [0u8; SEARCH_KEY_BYTES];
            bytes[..16].copy_from_slice(owner_partition.as_bytes());
            bytes[16..32].copy_from_slice(owner_partition.as_bytes());
            bytes[32..48].copy_from_slice(owner_partition.as_bytes());
            Ok(ResolvedSearchKey {
                plaintext: SecretSearchKey::new(bytes),
                key_version: "blocked-cache-v1".into(),
            })
        }
    }

    struct CountingResolvedKeyProvider {
        calls: std::sync::atomic::AtomicUsize,
    }

    #[async_trait]
    impl SearchKeyProvider for CountingResolvedKeyProvider {
        async fn resolve_search_key(&self, owner_partition: Uuid) -> AppResult<ResolvedSearchKey> {
            self.calls
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let mut bytes = [0u8; SEARCH_KEY_BYTES];
            bytes[..16].copy_from_slice(owner_partition.as_bytes());
            bytes[16..32].copy_from_slice(owner_partition.as_bytes());
            bytes[32..48].copy_from_slice(owner_partition.as_bytes());
            Ok(ResolvedSearchKey {
                plaintext: SecretSearchKey::new(bytes),
                key_version: "cache-test-v1".into(),
            })
        }
    }

    struct FixedSeedProvider {
        bytes: [u8; SEARCH_KEY_SEED_BYTES],
        version: String,
    }

    #[async_trait]
    impl SearchKeySeedProvider for FixedSeedProvider {
        async fn resolve_search_seed(
            &self,
            _owner_partition: Uuid,
        ) -> AppResult<ResolvedSearchKeySeed> {
            Ok(ResolvedSearchKeySeed {
                plaintext: SecretSearchKeySeed::new(self.bytes),
                key_version: self.version.clone(),
            })
        }
    }

    fn hkdf_provider(byte: u8, version: &str) -> HkdfSearchKeyProvider {
        HkdfSearchKeyProvider::new(Arc::new(FixedSeedProvider {
            bytes: [byte; SEARCH_KEY_SEED_BYTES],
            version: version.into(),
        }))
    }

    #[test]
    fn search_key_debug_is_redacted() {
        let key = SecretSearchKey::new([0xAB; SEARCH_KEY_BYTES]);
        let debug = format!("{key:?}");

        assert_eq!(debug, "SecretSearchKey([REDACTED])");
        assert!(!debug.contains("171"));
        assert_eq!(key.expose(), &[0xAB; SEARCH_KEY_BYTES]);
    }

    #[test]
    fn resolved_key_requires_version() {
        let valid = ResolvedSearchKey {
            plaintext: SecretSearchKey::new([0x01; SEARCH_KEY_BYTES]),
            key_version: "search-key-v1".into(),
        };
        assert!(valid.validate().is_ok());

        let invalid = ResolvedSearchKey {
            plaintext: SecretSearchKey::new([0x01; SEARCH_KEY_BYTES]),
            key_version: " ".into(),
        };
        assert!(invalid.validate().is_err());

        let invalid = ResolvedSearchKey {
            plaintext: SecretSearchKey::new([0x01; SEARCH_KEY_BYTES]),
            key_version: "search key v1".into(),
        };
        assert!(invalid.validate().is_err());

        let invalid = ResolvedSearchKey {
            plaintext: SecretSearchKey::new([0x01; SEARCH_KEY_BYTES]),
            key_version: "x".repeat(MAX_SEARCH_VERSION_ID_CHARS + 1),
        };
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn search_seed_debug_is_redacted() {
        let seed = SecretSearchKeySeed::new([0xCD; SEARCH_KEY_SEED_BYTES]);
        let debug = format!("{seed:?}");

        assert_eq!(debug, "SecretSearchKeySeed([REDACTED])");
        assert!(!debug.contains("205"));
    }

    #[tokio::test]
    async fn hkdf_search_key_is_stable_for_same_owner_and_seed_version() {
        let provider = hkdf_provider(0x11, "search-seed-v1");
        let owner = Uuid::new_v4();

        let first = provider.resolve_search_key(owner).await.unwrap();
        let second = provider.resolve_search_key(owner).await.unwrap();

        assert_eq!(first.plaintext.expose(), second.plaintext.expose());
        assert_eq!(first.key_version, "hkdf384-v1:search-seed-v1");
    }

    #[tokio::test]
    async fn hkdf_search_key_is_owner_scoped() {
        let provider = hkdf_provider(0x22, "search-seed-v1");

        let first = provider.resolve_search_key(Uuid::new_v4()).await.unwrap();
        let second = provider.resolve_search_key(Uuid::new_v4()).await.unwrap();

        assert_ne!(first.plaintext.expose(), second.plaintext.expose());
    }

    #[tokio::test]
    async fn hkdf_search_key_changes_on_seed_rotation() {
        let owner = Uuid::new_v4();
        let first = hkdf_provider(0x33, "search-seed-v1")
            .resolve_search_key(owner)
            .await
            .unwrap();
        let second = hkdf_provider(0x44, "search-seed-v2")
            .resolve_search_key(owner)
            .await
            .unwrap();

        assert_ne!(first.plaintext.expose(), second.plaintext.expose());
        assert_ne!(first.key_version, second.key_version);
    }

    #[tokio::test]
    async fn hkdf_search_key_rejects_invalid_seed_version() {
        let provider = hkdf_provider(0x55, "search seed v1");

        assert!(matches!(
            provider.resolve_search_key(Uuid::new_v4()).await,
            Err(AppError::ServiceUnavailable(_))
        ));
    }

    #[test]
    fn search_key_cache_policy_requires_positive_bounds() {
        assert!(SearchKeyCachePolicy::new(Duration::ZERO, 1).is_err());
        assert!(SearchKeyCachePolicy::new(Duration::from_secs(1), 0).is_err());
        assert!(SearchKeyCachePolicy::new(Duration::from_secs(1), 1).is_ok());
    }

    #[tokio::test]
    async fn search_key_cache_hits_until_ttl_expires() {
        let inner = Arc::new(CountingResolvedKeyProvider {
            calls: std::sync::atomic::AtomicUsize::new(0),
        });
        let cache = CachingSearchKeyProvider::new(
            inner.clone(),
            SearchKeyCachePolicy::new(Duration::from_secs(60), 8).unwrap(),
        );
        let owner = Uuid::new_v4();
        let start = Instant::now();

        let first = cache.resolve_at(owner, start).await.unwrap();
        let second = cache
            .resolve_at(owner, start + Duration::from_secs(59))
            .await
            .unwrap();

        assert_eq!(first.plaintext.expose(), second.plaintext.expose());
        assert_eq!(inner.calls.load(std::sync::atomic::Ordering::Relaxed), 1);

        cache
            .resolve_at(owner, start + Duration::from_secs(61))
            .await
            .unwrap();
        assert_eq!(inner.calls.load(std::sync::atomic::Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn search_key_cache_enforces_max_entries_and_supports_invalidation() {
        let inner = Arc::new(CountingResolvedKeyProvider {
            calls: std::sync::atomic::AtomicUsize::new(0),
        });
        let cache = CachingSearchKeyProvider::new(
            inner.clone(),
            SearchKeyCachePolicy::new(Duration::from_secs(60), 1).unwrap(),
        );
        let first_owner = Uuid::from_u128(1);
        let second_owner = Uuid::from_u128(2);
        let start = Instant::now();

        cache.resolve_at(first_owner, start).await.unwrap();
        cache
            .resolve_at(second_owner, start + Duration::from_secs(1))
            .await
            .unwrap();
        cache
            .resolve_at(first_owner, start + Duration::from_secs(2))
            .await
            .unwrap();
        assert_eq!(inner.calls.load(std::sync::atomic::Ordering::Relaxed), 3);

        cache.invalidate(first_owner).await;
        cache
            .resolve_at(first_owner, start + Duration::from_secs(3))
            .await
            .unwrap();
        assert_eq!(inner.calls.load(std::sync::atomic::Ordering::Relaxed), 4);

        cache.clear().await;
        cache
            .resolve_at(first_owner, start + Duration::from_secs(4))
            .await
            .unwrap();
        assert_eq!(inner.calls.load(std::sync::atomic::Ordering::Relaxed), 5);
    }

    #[tokio::test]
    async fn concurrent_misses_for_same_owner_single_flight_provider_resolution() {
        let inner = Arc::new(BlockingResolvedKeyProvider {
            calls: std::sync::atomic::AtomicUsize::new(0),
            started: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        });
        let cache = Arc::new(CachingSearchKeyProvider::new(
            inner.clone(),
            SearchKeyCachePolicy::new(Duration::from_secs(60), 8).unwrap(),
        ));
        let owner = Uuid::new_v4();

        let first_started = inner.started.notified();
        let first = {
            let cache = Arc::clone(&cache);
            tokio::spawn(async move { cache.resolve_search_key(owner).await })
        };
        first_started.await;

        let second = {
            let cache = Arc::clone(&cache);
            tokio::spawn(async move { cache.resolve_search_key(owner).await })
        };

        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            inner.calls.load(std::sync::atomic::Ordering::Relaxed),
            1,
            "same-owner cache miss must not duplicate the provider call"
        );

        inner.release.notify_one();

        let first = first.await.unwrap().unwrap();
        let second = second.await.unwrap().unwrap();
        assert_eq!(first.plaintext.expose(), second.plaintext.expose());
        assert_eq!(first.key_version, second.key_version);
        assert_eq!(inner.calls.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert!(cache.state.lock().await.resolution_gates.is_empty());
    }

    #[tokio::test]
    async fn concurrent_misses_for_different_owners_resolve_in_parallel() {
        let inner = Arc::new(BlockingResolvedKeyProvider {
            calls: std::sync::atomic::AtomicUsize::new(0),
            started: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        });
        let cache = Arc::new(CachingSearchKeyProvider::new(
            inner.clone(),
            SearchKeyCachePolicy::new(Duration::from_secs(60), 8).unwrap(),
        ));
        let first_owner = Uuid::new_v4();
        let second_owner = Uuid::new_v4();

        let first_started = inner.started.notified();
        let first = {
            let cache = Arc::clone(&cache);
            tokio::spawn(async move { cache.resolve_search_key(first_owner).await })
        };
        first_started.await;

        let second_started = inner.started.notified();
        let second = {
            let cache = Arc::clone(&cache);
            tokio::spawn(async move { cache.resolve_search_key(second_owner).await })
        };
        second_started.await;

        assert_eq!(
            inner.calls.load(std::sync::atomic::Ordering::Relaxed),
            2,
            "different owners must not share one provider-resolution gate"
        );

        inner.release.notify_one();
        inner.release.notify_one();

        first.await.unwrap().unwrap();
        second.await.unwrap().unwrap();
        assert!(cache.state.lock().await.resolution_gates.is_empty());
    }

    #[tokio::test]
    async fn invalidation_fences_in_flight_key_resolution() {
        let inner = Arc::new(BlockingResolvedKeyProvider {
            calls: std::sync::atomic::AtomicUsize::new(0),
            started: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        });
        let cache = Arc::new(CachingSearchKeyProvider::new(
            inner.clone(),
            SearchKeyCachePolicy::new(Duration::from_secs(60), 8).unwrap(),
        ));
        let owner = Uuid::new_v4();
        let started = inner.started.notified();
        let resolving = {
            let cache = Arc::clone(&cache);
            tokio::spawn(async move { cache.resolve_search_key(owner).await })
        };

        started.await;
        cache.invalidate(owner).await;
        inner.release.notify_one();

        assert!(matches!(
            resolving.await.unwrap(),
            Err(AppError::ServiceUnavailable(_))
        ));
    }
}
