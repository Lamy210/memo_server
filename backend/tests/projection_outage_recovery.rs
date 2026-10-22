use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use async_trait::async_trait;
use memo_app_backend::{
    application::maintenance::{
        MemoMutationGuard, MemoMutationPermit, UnrestrictedMemoMutationGuard,
    },
    domain::memo::{entity::Memo, repository::MemoRepository},
    error::{AppError, AppResult},
    infrastructure::{
        persistence::ports::{
            MemoAuthoritativeStore, MemoCache, MemoSearchHitPage, MemoSearchProjection,
            ProjectionIntent, ProjectionTarget,
        },
        reconciliation::ProjectionReconciler,
        repositories::memo::MemoRepositoryImpl,
    },
};
use uuid::Uuid;

/// A deterministic transactional-outbox stand-in: a mutation and its intent are
/// recorded together. This is not a replacement for MongoDB integration tests.
#[derive(Default)]
struct DurableStore {
    current: Mutex<Option<Memo>>,
    pending: Mutex<Vec<ProjectionIntent>>,
}

impl DurableStore {
    fn pending(&self) -> Vec<ProjectionIntent> {
        self.pending.lock().unwrap().clone()
    }

    fn record(&self, owner: Uuid, id: Uuid, target: ProjectionTarget) -> ProjectionIntent {
        let event = ProjectionIntent::new(owner, id, target);
        self.pending.lock().unwrap().push(event.clone());
        event
    }
}

#[async_trait]
impl MemoAuthoritativeStore for DurableStore {
    async fn find_by_id(&self, owner: Uuid, id: Uuid) -> AppResult<Option<Memo>> {
        Ok(self
            .current
            .lock()
            .unwrap()
            .as_ref()
            .filter(|memo| memo.user_id == owner && memo.id == id)
            .cloned())
    }

    async fn find_many_by_ids(&self, owner: Uuid, ids: &[Uuid]) -> AppResult<Vec<Memo>> {
        Ok(self
            .current
            .lock()
            .unwrap()
            .as_ref()
            .filter(|memo| memo.user_id == owner && ids.contains(&memo.id))
            .cloned()
            .into_iter()
            .collect())
    }

    async fn save_with_projection_intent(&self, memo: &Memo) -> AppResult<ProjectionIntent> {
        *self.current.lock().unwrap() = Some(memo.clone());
        Ok(self.record(
            memo.user_id,
            memo.id,
            ProjectionTarget::Version(memo.version),
        ))
    }

    async fn delete_with_projection_intent(
        &self,
        owner: Uuid,
        id: Uuid,
    ) -> AppResult<ProjectionIntent> {
        *self.current.lock().unwrap() = None;
        Ok(self.record(owner, id, ProjectionTarget::Deleted))
    }

    async fn exists(&self, owner: Uuid, id: Uuid) -> AppResult<bool> {
        Ok(self.find_by_id(owner, id).await?.is_some())
    }

    async fn enqueue_projection_intent(
        &self,
        owner: Uuid,
        id: Uuid,
        target: ProjectionTarget,
    ) -> AppResult<ProjectionIntent> {
        Ok(self.record(owner, id, target))
    }

    async fn list_projection_intents(&self) -> AppResult<Vec<ProjectionIntent>> {
        Ok(self.pending())
    }

    async fn acknowledge_projection_intent(&self, event: &ProjectionIntent) -> AppResult<()> {
        self.pending
            .lock()
            .unwrap()
            .retain(|pending| pending.event_id != event.event_id);
        Ok(())
    }
}

#[derive(Default)]
struct RecoverableCache {
    online: AtomicBool,
    rows: Mutex<HashMap<(Uuid, Uuid), Memo>>,
}

impl RecoverableCache {
    fn set_online(&self, online: bool) {
        self.online.store(online, Ordering::Relaxed);
    }

    fn ensure_online(&self) -> AppResult<()> {
        if self.online.load(Ordering::Relaxed) {
            Ok(())
        } else {
            Err(AppError::ServiceUnavailable("injected cache outage".into()))
        }
    }
}

#[async_trait]
impl MemoCache for RecoverableCache {
    async fn get_memo(&self, owner: Uuid, id: Uuid) -> AppResult<Option<Memo>> {
        self.ensure_online()?;
        Ok(self.rows.lock().unwrap().get(&(owner, id)).cloned())
    }

    async fn set_memo(&self, memo: &Memo, _ttl: Option<Duration>) -> AppResult<()> {
        self.ensure_online()?;
        self.rows
            .lock()
            .unwrap()
            .insert((memo.user_id, memo.id), memo.clone());
        Ok(())
    }

    async fn delete_memo(&self, owner: Uuid, id: Uuid) -> AppResult<()> {
        self.ensure_online()?;
        self.rows.lock().unwrap().remove(&(owner, id));
        Ok(())
    }

    async fn memo_exists(&self, owner: Uuid, id: Uuid) -> AppResult<bool> {
        Ok(self.get_memo(owner, id).await?.is_some())
    }
}

#[derive(Default)]
struct RecoverableSearch {
    online: AtomicBool,
    rows: Mutex<HashMap<Uuid, Memo>>,
}

impl RecoverableSearch {
    fn set_online(&self, online: bool) {
        self.online.store(online, Ordering::Relaxed);
    }

    fn ensure_online(&self) -> AppResult<()> {
        if self.online.load(Ordering::Relaxed) {
            Ok(())
        } else {
            Err(AppError::ServiceUnavailable(
                "injected search outage".into(),
            ))
        }
    }
}

#[async_trait]
impl MemoSearchProjection for RecoverableSearch {
    async fn index_memo(&self, memo: &Memo) -> AppResult<()> {
        self.ensure_online()?;
        self.rows.lock().unwrap().insert(memo.id, memo.clone());
        Ok(())
    }

    async fn search_memo_ids(
        &self,
        _query: &str,
        _tag: Option<String>,
        _owner: Uuid,
        _page: usize,
        _limit: usize,
    ) -> AppResult<MemoSearchHitPage> {
        self.ensure_online()?;
        Ok(MemoSearchHitPage {
            memo_ids: Vec::new(),
            total: 0,
        })
    }

    async fn delete_memo(&self, id: Uuid) -> AppResult<()> {
        self.ensure_online()?;
        self.rows.lock().unwrap().remove(&id);
        Ok(())
    }
}

/// An unavailable distributed maintenance writer lease must not retain an
/// older cache value after the authoritative mutation has committed.
struct UnavailableMutationGuard;

#[async_trait]
impl MemoMutationGuard for UnavailableMutationGuard {
    async fn acquire_mutation(&self) -> AppResult<Box<dyn MemoMutationPermit>> {
        Err(AppError::ServiceUnavailable(
            "injected maintenance writer outage".into(),
        ))
    }
}

#[tokio::test]
async fn committed_update_and_delete_evict_cache_even_when_reconciliation_is_blocked() {
    let owner = Uuid::new_v4();
    let store = Arc::new(DurableStore::default());
    let cache = Arc::new(RecoverableCache::default());
    let search = Arc::new(RecoverableSearch::default());
    cache.set_online(true);
    search.set_online(true);

    let mut memo = Memo::new("version 1".into(), "body".into(), vec![], owner);
    store.save_with_projection_intent(&memo).await.unwrap();
    cache.set_memo(&memo, None).await.unwrap();

    let guard = Arc::new(UnavailableMutationGuard);
    let reconciler = Arc::new(ProjectionReconciler::new(
        store.clone(),
        cache.clone(),
        search.clone(),
        None,
        guard,
    ));
    let repository = MemoRepositoryImpl::new(store.clone(), cache.clone(), search, reconciler);

    memo.update(Some("version 2".into()), None, None);
    repository.save(&memo).await.unwrap();

    // The synchronous projector could not acquire a writer lease. The old
    // cached version must nevertheless be invalidated before save returns.
    assert!(cache.get_memo(owner, memo.id).await.unwrap().is_none());
    assert_eq!(
        repository
            .find_by_id(owner, memo.id)
            .await
            .unwrap()
            .unwrap()
            .version,
        2
    );
    assert_eq!(
        cache.get_memo(owner, memo.id).await.unwrap().unwrap().title,
        "version 2"
    );

    repository.delete(owner, memo.id).await.unwrap();

    // Neither GET nor EXISTS may resurrect a deleted memo from Valkey.
    assert!(cache.get_memo(owner, memo.id).await.unwrap().is_none());
    assert!(repository
        .find_by_id(owner, memo.id)
        .await
        .unwrap()
        .is_none());
    assert!(!repository.exists(owner, memo.id).await.unwrap());
    // The failed secondary projection remains durable for later replay.
    assert_eq!(store.pending().len(), 3);
}

#[tokio::test]
async fn deleted_memo_does_not_exist_when_cache_eviction_was_unavailable() {
    let owner = Uuid::new_v4();
    let other_owner = Uuid::new_v4();
    let store = Arc::new(DurableStore::default());
    let cache = Arc::new(RecoverableCache::default());
    let search = Arc::new(RecoverableSearch::default());
    cache.set_online(true);
    search.set_online(true);

    let memo = Memo::new("cached".into(), "body".into(), vec![], owner);
    store.save_with_projection_intent(&memo).await.unwrap();
    cache.set_memo(&memo, None).await.unwrap();
    let reconciler = Arc::new(ProjectionReconciler::new(
        store.clone(),
        cache.clone(),
        search.clone(),
        None,
        Arc::new(UnavailableMutationGuard),
    ));
    let repository = MemoRepositoryImpl::new(store.clone(), cache.clone(), search, reconciler);
    assert!(repository.exists(owner, memo.id).await.unwrap());
    assert!(!repository.exists(other_owner, memo.id).await.unwrap());

    // Authoritative deletion commits, but cache eviction cannot reach Valkey.
    // A later cache recovery exposes the original row until outbox replay.
    cache.set_online(false);
    repository.delete(owner, memo.id).await.unwrap();
    assert!(store.find_by_id(owner, memo.id).await.unwrap().is_none());
    cache.set_online(true);
    assert!(cache.memo_exists(owner, memo.id).await.unwrap());

    // A positive cache hint is not proof of authoritative existence.
    assert!(!repository.exists(owner, memo.id).await.unwrap());
    assert!(!repository.exists(other_owner, memo.id).await.unwrap());
    assert_eq!(store.pending().len(), 2);
}

#[tokio::test]
async fn replay_after_cache_and_search_outages_converges_create_update_delete() {
    let owner = Uuid::new_v4();
    let store = Arc::new(DurableStore::default());
    let cache = Arc::new(RecoverableCache::default());
    let search = Arc::new(RecoverableSearch::default());
    let guard = Arc::new(UnrestrictedMemoMutationGuard);
    let reconciler = Arc::new(ProjectionReconciler::new(
        store.clone(),
        cache.clone(),
        search.clone(),
        None,
        guard.clone(),
    ));
    let repository =
        MemoRepositoryImpl::new(store.clone(), cache.clone(), search.clone(), reconciler);

    // Both projections are offline. Writes still succeed because the durable
    // authoritative state and outbox are the source of truth.
    let mut memo = Memo::new("v1".into(), "body".into(), vec![], owner);
    repository.save(&memo).await.unwrap();
    assert_eq!(store.pending().len(), 1);
    memo.update(Some("v2".into()), None, None);
    repository.save(&memo).await.unwrap();
    assert_eq!(store.pending().len(), 2);
    assert_eq!(
        store
            .find_by_id(owner, memo.id)
            .await
            .unwrap()
            .unwrap()
            .version,
        2
    );

    // Simulate a process restart: no in-memory retry state is carried over.
    // Replay newest-first, then the older intent. Neither may revert to v1.
    cache.set_online(true);
    search.set_online(true);
    let restarted = ProjectionReconciler::new(
        store.clone(),
        cache.clone(),
        search.clone(),
        None,
        guard.clone(),
    );
    for event in store.pending().iter().rev() {
        restarted.reconcile_now(event).await;
    }
    assert!(store.pending().is_empty());
    assert_eq!(
        cache.get_memo(owner, memo.id).await.unwrap().unwrap().title,
        "v2"
    );
    assert_eq!(
        search.rows.lock().unwrap().get(&memo.id).unwrap().version,
        2
    );

    // A delete while projections are offline must also remain in the outbox.
    cache.set_online(false);
    search.set_online(false);
    repository.delete(owner, memo.id).await.unwrap();
    assert_eq!(store.pending().len(), 1);
    assert!(store.find_by_id(owner, memo.id).await.unwrap().is_none());

    cache.set_online(true);
    search.set_online(true);
    let restarted =
        ProjectionReconciler::new(store.clone(), cache.clone(), search.clone(), None, guard);
    for event in store.pending() {
        restarted.reconcile_now(&event).await;
    }
    assert!(store.pending().is_empty());
    assert!(cache.get_memo(owner, memo.id).await.unwrap().is_none());
    assert!(!search.rows.lock().unwrap().contains_key(&memo.id));
}
