use std::time::Duration;

use ::mongodb::{
    bson::{doc, Bson, Document},
    error::Error as MongoError,
    options::WriteConcern,
    Client, Collection, Database,
};
use async_trait::async_trait;
use futures::FutureExt;
use tokio::time::sleep;
use uuid::Uuid;

use crate::{
    application::{
        crypto_search_rotation::{HighSearchOfflineWindowGuard, HighSearchOfflineWindowPermit},
        maintenance::{MemoMutationGuard, MemoMutationPermit},
    },
    error::{AppError, AppResult},
};

const STATE_COLLECTION: &str = "high_search_maintenance_state";
const WRITER_LEASES_COLLECTION: &str = "high_search_writer_leases";
const STATE_ID: &str = "global";
const MODE_OPEN: &str = "open";
const MODE_MAINTENANCE: &str = "maintenance";
const DRAIN_POLL_INTERVAL: Duration = Duration::from_millis(25);

#[derive(Debug)]
struct MaintenanceActive;

#[derive(Debug)]
struct RecoverySnapshotMismatch;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HighSearchMaintenanceMode {
    Open,
    Maintenance,
}

impl std::fmt::Display for HighSearchMaintenanceMode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Open => formatter.write_str(MODE_OPEN),
            Self::Maintenance => formatter.write_str(MODE_MAINTENANCE),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HighSearchMaintenanceStatus {
    mode: HighSearchMaintenanceMode,
    holder_token: Option<String>,
    writer_epoch: i64,
    active_writer_leases: u64,
}

impl HighSearchMaintenanceStatus {
    pub fn mode(&self) -> HighSearchMaintenanceMode {
        self.mode
    }

    pub fn holder_token(&self) -> Option<&str> {
        self.holder_token.as_deref()
    }

    pub fn writer_epoch(&self) -> i64 {
        self.writer_epoch
    }

    pub fn active_writer_leases(&self) -> u64 {
        self.active_writer_leases
    }
}

struct WriterAcquireContext {
    state: Collection<Document>,
    writers: Collection<Document>,
    lease_id: String,
}

struct MaintenanceAcquireContext {
    state: Collection<Document>,
    holder_token: String,
}

struct RecoveryContext {
    state: Collection<Document>,
    writers: Collection<Document>,
    expected: HighSearchMaintenanceStatus,
}

/// Operator-only recovery boundary for fail-closed HIGH search maintenance state.
///
/// This type deliberately does not perform time-based lease expiry. Callers must
/// inspect the current state, stop all application replicas, and recover only
/// from the exact observed writer epoch / holder token snapshot.
#[derive(Clone)]
pub struct MongoHighSearchMaintenanceRecovery {
    client: Client,
    state: Collection<Document>,
    writers: Collection<Document>,
}

impl MongoHighSearchMaintenanceRecovery {
    pub async fn connect(uri: &str, database_name: &str) -> AppResult<Self> {
        if uri.trim().is_empty() {
            return Err(AppError::ValidationError(
                "MONGODB_URI must not be empty for maintenance recovery".into(),
            ));
        }
        if database_name.trim().is_empty() {
            return Err(AppError::ValidationError(
                "MONGODB_DATABASE must not be empty for maintenance recovery".into(),
            ));
        }

        let client = Client::with_uri_str(uri)
            .await
            .map_err(|error| maintenance_db_error("connect maintenance recovery client", error))?;
        Ok(Self::from_database(client.database(database_name)))
    }

    fn from_database(database: Database) -> Self {
        Self {
            client: database.client().clone(),
            state: database.collection(STATE_COLLECTION),
            writers: database.collection(WRITER_LEASES_COLLECTION),
        }
    }

    pub async fn inspect(&self) -> AppResult<HighSearchMaintenanceStatus> {
        let state = self
            .state
            .find_one(doc! { "_id": STATE_ID })
            .await
            .map_err(|error| maintenance_db_error("inspect maintenance state", error))?
            .ok_or_else(|| {
                AppError::ServiceUnavailable(
                    "HIGH search maintenance state is not initialized".into(),
                )
            })?;

        let mode = match state.get_str("mode") {
            Ok(MODE_OPEN) => HighSearchMaintenanceMode::Open,
            Ok(MODE_MAINTENANCE) => HighSearchMaintenanceMode::Maintenance,
            Ok(other) => {
                return Err(AppError::ServiceUnavailable(format!(
                    "HIGH search maintenance state has unknown mode `{other}`"
                )))
            }
            Err(_) => {
                return Err(AppError::ServiceUnavailable(
                    "HIGH search maintenance state is missing a valid mode".into(),
                ))
            }
        };

        let writer_epoch = state.get_i64("writer_epoch").map_err(|_| {
            AppError::ServiceUnavailable(
                "HIGH search maintenance state is missing a valid writer_epoch".into(),
            )
        })?;

        let holder_token = match state.get("holder_token") {
            None | Some(Bson::Null) => None,
            Some(Bson::String(value)) => Some(value.clone()),
            Some(_) => {
                return Err(AppError::ServiceUnavailable(
                    "HIGH search maintenance state has an invalid holder_token".into(),
                ))
            }
        };

        match (mode, holder_token.as_deref()) {
            (HighSearchMaintenanceMode::Open, None)
            | (HighSearchMaintenanceMode::Maintenance, Some(_)) => {}
            (HighSearchMaintenanceMode::Open, Some(_)) => {
                return Err(AppError::ServiceUnavailable(
                    "open HIGH search maintenance state unexpectedly retains a holder token".into(),
                ))
            }
            (HighSearchMaintenanceMode::Maintenance, None) => {
                return Err(AppError::ServiceUnavailable(
                    "active HIGH search maintenance state is missing its holder token".into(),
                ))
            }
        }

        let active_writer_leases = self
            .writers
            .count_documents(doc! {})
            .await
            .map_err(|error| maintenance_db_error("inspect memo writer leases", error))?;

        Ok(HighSearchMaintenanceStatus {
            mode,
            holder_token,
            writer_epoch,
            active_writer_leases,
        })
    }

    /// Clear fail-closed writer leases and reopen the maintenance barrier from
    /// one exact operator-observed snapshot.
    ///
    /// The transaction matches mode, writer epoch, and maintenance holder token
    /// (when present). Any intervening writer acquisition or barrier ownership
    /// change aborts recovery instead of clearing state from a newer generation.
    pub async fn recover_stale_state(
        &self,
        expected: &HighSearchMaintenanceStatus,
    ) -> AppResult<HighSearchMaintenanceStatus> {
        let mut session = self
            .client
            .start_session()
            .await
            .map_err(|error| maintenance_db_error("start maintenance recovery session", error))?;
        let context = RecoveryContext {
            state: self.state.clone(),
            writers: self.writers.clone(),
            expected: expected.clone(),
        };

        let result = session
            .start_transaction()
            .write_concern(WriteConcern::majority())
            .and_run(context, |session, context| {
                async move {
                    context
                        .writers
                        .delete_many(doc! {})
                        .session(&mut *session)
                        .await?;

                    let mut filter = doc! {
                        "_id": STATE_ID,
                        "mode": context.expected.mode.to_string(),
                        "writer_epoch": context.expected.writer_epoch,
                    };
                    match context.expected.holder_token.as_deref() {
                        Some(holder_token) => {
                            filter.insert("holder_token", holder_token);
                        }
                        None => {
                            filter.insert("holder_token", Bson::Null);
                        }
                    }

                    let update = context
                        .state
                        .update_one(
                            filter,
                            doc! {
                                "$set": { "mode": MODE_OPEN },
                                "$unset": { "holder_token": "" },
                                "$inc": { "writer_epoch": 1_i64 },
                            },
                        )
                        .session(&mut *session)
                        .await?;

                    if update.matched_count != 1 {
                        return Err(MongoError::custom(RecoverySnapshotMismatch));
                    }

                    Ok(())
                }
                .boxed()
            })
            .await;

        match result {
            Ok(()) => self.inspect().await,
            Err(error) if error.get_custom::<RecoverySnapshotMismatch>().is_some() => {
                Err(AppError::Conflict(
                    "HIGH search maintenance recovery snapshot changed; inspect again before retrying"
                        .into(),
                ))
            }
            Err(error) => Err(maintenance_db_error(
                "recover HIGH search maintenance state",
                error,
            )),
        }
    }
}

#[derive(Clone)]
pub(crate) struct MongoHighSearchMaintenanceGuard {
    client: Client,
    state: Collection<Document>,
    writers: Collection<Document>,
}

impl MongoHighSearchMaintenanceGuard {
    pub(crate) async fn new(database: Database) -> AppResult<Self> {
        let guard = Self {
            client: database.client().clone(),
            state: database.collection(STATE_COLLECTION),
            writers: database.collection(WRITER_LEASES_COLLECTION),
        };
        guard.initialize_state().await?;
        Ok(guard)
    }

    async fn initialize_state(&self) -> AppResult<()> {
        let result = self
            .state
            .update_one(
                doc! { "_id": STATE_ID },
                doc! {
                    "$setOnInsert": {
                        "mode": MODE_OPEN,
                        "holder_token": Bson::Null,
                        "writer_epoch": 0_i64,
                    }
                },
            )
            .upsert(true)
            .await;

        match result {
            Ok(_) => Ok(()),
            Err(error) => {
                // Concurrent application startups may race the singleton
                // upsert. Accept that race only if the singleton now exists.
                if self
                    .state
                    .find_one(doc! { "_id": STATE_ID })
                    .await
                    .map_err(|lookup| maintenance_db_error("verify maintenance state", lookup))?
                    .is_some()
                {
                    Ok(())
                } else {
                    Err(maintenance_db_error(
                        "initialize HIGH search maintenance state",
                        error,
                    ))
                }
            }
        }
    }

    async fn acquire_writer_lease(&self) -> AppResult<MongoMemoMutationPermit> {
        let lease_id = Uuid::new_v4().to_string();
        let mut session = self
            .client
            .start_session()
            .await
            .map_err(|error| maintenance_db_error("start writer lease session", error))?;
        let context = WriterAcquireContext {
            state: self.state.clone(),
            writers: self.writers.clone(),
            lease_id: lease_id.clone(),
        };

        let result = session
            .start_transaction()
            .write_concern(WriteConcern::majority())
            .and_run(context, |session, context| {
                async move {
                    // Writer acquisition and maintenance acquisition both
                    // write the singleton gate. MongoDB therefore resolves a
                    // race before the writer lease transaction can commit.
                    let gate = context
                        .state
                        .update_one(
                            doc! { "_id": STATE_ID, "mode": MODE_OPEN },
                            doc! { "$inc": { "writer_epoch": 1_i64 } },
                        )
                        .session(&mut *session)
                        .await?;

                    if gate.matched_count != 1 {
                        return Err(MongoError::custom(MaintenanceActive));
                    }

                    context
                        .writers
                        .insert_one(doc! { "_id": context.lease_id.clone() })
                        .session(&mut *session)
                        .await?;
                    Ok(())
                }
                .boxed()
            })
            .await;

        match result {
            Ok(()) => Ok(MongoMemoMutationPermit {
                writers: self.writers.clone(),
                lease_id,
            }),
            Err(error) if error.get_custom::<MaintenanceActive>().is_some() => {
                Err(AppError::ServiceUnavailable(
                    "memo mutations are temporarily frozen by HIGH search maintenance".into(),
                ))
            }
            Err(error) => Err(maintenance_db_error("acquire memo writer lease", error)),
        }
    }

    async fn acquire_maintenance_barrier(&self) -> AppResult<MongoHighSearchOfflineWindowPermit> {
        let holder_token = Uuid::new_v4().to_string();
        let mut session = self
            .client
            .start_session()
            .await
            .map_err(|error| maintenance_db_error("start maintenance session", error))?;
        let context = MaintenanceAcquireContext {
            state: self.state.clone(),
            holder_token: holder_token.clone(),
        };

        let result = session
            .start_transaction()
            .write_concern(WriteConcern::majority())
            .and_run(context, |session, context| {
                async move {
                    let gate = context
                        .state
                        .update_one(
                            doc! { "_id": STATE_ID, "mode": MODE_OPEN },
                            doc! {
                                "$set": {
                                    "mode": MODE_MAINTENANCE,
                                    "holder_token": context.holder_token.clone(),
                                }
                            },
                        )
                        .session(&mut *session)
                        .await?;

                    if gate.matched_count != 1 {
                        return Err(MongoError::custom(MaintenanceActive));
                    }
                    Ok(())
                }
                .boxed()
            })
            .await;

        match result {
            Ok(()) => Ok(MongoHighSearchOfflineWindowPermit {
                state: self.state.clone(),
                holder_token,
            }),
            Err(error) if error.get_custom::<MaintenanceActive>().is_some() => Err(
                AppError::Conflict("HIGH search maintenance window is already active".into()),
            ),
            Err(error) => Err(maintenance_db_error(
                "acquire HIGH search maintenance barrier",
                error,
            )),
        }
    }

    async fn active_writer_count(&self) -> AppResult<u64> {
        self.writers
            .count_documents(doc! {})
            .await
            .map_err(|error| maintenance_db_error("count active memo writer leases", error))
    }
}

struct MongoMemoMutationPermit {
    writers: Collection<Document>,
    lease_id: String,
}

#[async_trait]
impl MemoMutationPermit for MongoMemoMutationPermit {
    async fn release(self: Box<Self>) -> AppResult<()> {
        let result = self
            .writers
            .delete_one(doc! { "_id": self.lease_id.clone() })
            .await
            .map_err(|error| maintenance_db_error("release memo writer lease", error))?;

        if result.deleted_count != 1 {
            return Err(AppError::ServiceUnavailable(
                "memo writer lease disappeared before release".into(),
            ));
        }

        Ok(())
    }
}

struct MongoHighSearchOfflineWindowPermit {
    state: Collection<Document>,
    holder_token: String,
}

#[async_trait]
impl HighSearchOfflineWindowPermit for MongoHighSearchOfflineWindowPermit {
    async fn assert_still_enforced(&self) -> AppResult<()> {
        let present = self
            .state
            .find_one(doc! {
                "_id": STATE_ID,
                "mode": MODE_MAINTENANCE,
                "holder_token": self.holder_token.clone(),
            })
            .await
            .map_err(|error| maintenance_db_error("verify maintenance barrier", error))?
            .is_some();

        if present {
            Ok(())
        } else {
            Err(AppError::Conflict(
                "HIGH search maintenance barrier ownership was lost".into(),
            ))
        }
    }

    async fn release(self: Box<Self>) -> AppResult<()> {
        let result = self
            .state
            .update_one(
                doc! {
                    "_id": STATE_ID,
                    "mode": MODE_MAINTENANCE,
                    "holder_token": self.holder_token.clone(),
                },
                doc! {
                    "$set": { "mode": MODE_OPEN },
                    "$unset": { "holder_token": "" },
                },
            )
            .await
            .map_err(|error| maintenance_db_error("release maintenance barrier", error))?;

        if result.matched_count != 1 {
            return Err(AppError::ServiceUnavailable(
                "HIGH search maintenance barrier ownership was lost before release".into(),
            ));
        }

        Ok(())
    }
}

#[async_trait]
impl MemoMutationGuard for MongoHighSearchMaintenanceGuard {
    async fn acquire_mutation(&self) -> AppResult<Box<dyn MemoMutationPermit>> {
        Ok(Box::new(self.acquire_writer_lease().await?))
    }
}

#[async_trait]
impl HighSearchOfflineWindowGuard for MongoHighSearchMaintenanceGuard {
    async fn acquire_offline_window(&self) -> AppResult<Box<dyn HighSearchOfflineWindowPermit>> {
        let permit = self.acquire_maintenance_barrier().await?;

        loop {
            permit.assert_still_enforced().await?;
            if self.active_writer_count().await? == 0 {
                return Ok(Box::new(permit));
            }
            sleep(DRAIN_POLL_INTERVAL).await;
        }
    }
}

fn maintenance_db_error(operation: &str, error: MongoError) -> AppError {
    AppError::DatabaseError(format!("{operation} failed: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_DATABASE_NAME: &str = "memo_app_maintenance_test";

    #[tokio::test]
    #[ignore = "requires a local MongoDB replica set"]
    async fn mongodb_maintenance_barrier_drains_and_blocks_writers() {
        let uri = std::env::var("MONGODB_TEST_URI")
            .unwrap_or_else(|_| "mongodb://localhost:27017/?replicaSet=rs0".to_string());
        let client = Client::with_uri_str(&uri).await.unwrap();
        let database = client.database(TEST_DATABASE_NAME);
        database.drop().await.unwrap();

        let guard = MongoHighSearchMaintenanceGuard::new(database.clone())
            .await
            .unwrap();
        let first_writer = guard.acquire_mutation().await.unwrap();

        let maintenance_guard = guard.clone();
        let maintenance_task =
            tokio::spawn(async move { maintenance_guard.acquire_offline_window().await });

        loop {
            let state = guard
                .state
                .find_one(doc! { "_id": STATE_ID })
                .await
                .unwrap()
                .unwrap();
            if state.get_str("mode").ok() == Some(MODE_MAINTENANCE) {
                break;
            }
            tokio::task::yield_now().await;
        }

        assert!(matches!(
            guard.acquire_mutation().await,
            Err(AppError::ServiceUnavailable(_))
        ));
        assert!(!maintenance_task.is_finished());

        first_writer.release().await.unwrap();
        let maintenance = maintenance_task.await.unwrap().unwrap();
        maintenance.assert_still_enforced().await.unwrap();

        assert!(matches!(
            guard.acquire_mutation().await,
            Err(AppError::ServiceUnavailable(_))
        ));

        maintenance.release().await.unwrap();
        let writer_after_release = guard.acquire_mutation().await.unwrap();
        writer_after_release.release().await.unwrap();

        database.drop().await.unwrap();
    }

    #[tokio::test]
    #[ignore = "requires a local MongoDB replica set"]
    async fn mongodb_maintenance_recovery_requires_exact_snapshot_and_reopens_state() {
        let uri = std::env::var("MONGODB_TEST_URI")
            .unwrap_or_else(|_| "mongodb://localhost:27017/?replicaSet=rs0".to_string());
        let client = Client::with_uri_str(&uri).await.unwrap();
        let database = client.database("memo_app_maintenance_recovery_test");
        database.drop().await.unwrap();

        let guard = MongoHighSearchMaintenanceGuard::new(database.clone())
            .await
            .unwrap();
        let recovery = MongoHighSearchMaintenanceRecovery::from_database(database.clone());

        // Simulate a cancelled mutation: dropping without explicit release
        // intentionally leaves a fail-closed writer lease behind.
        let abandoned_writer = guard.acquire_mutation().await.unwrap();
        drop(abandoned_writer);

        let open_snapshot = recovery.inspect().await.unwrap();
        assert_eq!(open_snapshot.mode(), HighSearchMaintenanceMode::Open);
        assert_eq!(open_snapshot.active_writer_leases(), 1);

        let mut stale_snapshot = open_snapshot.clone();
        stale_snapshot.writer_epoch -= 1;
        assert!(matches!(
            recovery.recover_stale_state(&stale_snapshot).await,
            Err(AppError::Conflict(_))
        ));
        assert_eq!(recovery.inspect().await.unwrap().active_writer_leases(), 1);

        let recovered = recovery.recover_stale_state(&open_snapshot).await.unwrap();
        assert_eq!(recovered.mode(), HighSearchMaintenanceMode::Open);
        assert_eq!(recovered.active_writer_leases(), 0);
        assert_eq!(recovered.writer_epoch(), open_snapshot.writer_epoch() + 1);

        // Simulate cancellation while maintenance owns the barrier. The task
        // drops its local permit without calling release, so the shared barrier
        // must remain closed for explicit operator recovery.
        let active_writer = guard.acquire_mutation().await.unwrap();
        let maintenance_guard = guard.clone();
        let maintenance_task =
            tokio::spawn(async move { maintenance_guard.acquire_offline_window().await });

        loop {
            let status = recovery.inspect().await.unwrap();
            if status.mode() == HighSearchMaintenanceMode::Maintenance {
                break;
            }
            tokio::task::yield_now().await;
        }

        maintenance_task.abort();
        let _ = maintenance_task.await;
        active_writer.release().await.unwrap();

        let maintenance_snapshot = recovery.inspect().await.unwrap();
        assert_eq!(
            maintenance_snapshot.mode(),
            HighSearchMaintenanceMode::Maintenance
        );
        assert!(maintenance_snapshot.holder_token().is_some());
        assert_eq!(maintenance_snapshot.active_writer_leases(), 0);
        assert!(matches!(
            guard.acquire_mutation().await,
            Err(AppError::ServiceUnavailable(_))
        ));

        let recovered = recovery
            .recover_stale_state(&maintenance_snapshot)
            .await
            .unwrap();
        assert_eq!(recovered.mode(), HighSearchMaintenanceMode::Open);
        assert!(recovered.holder_token().is_none());
        assert_eq!(
            recovered.writer_epoch(),
            maintenance_snapshot.writer_epoch() + 1
        );

        let writer_after_recovery = guard.acquire_mutation().await.unwrap();
        writer_after_recovery.release().await.unwrap();

        database.drop().await.unwrap();
    }
}
