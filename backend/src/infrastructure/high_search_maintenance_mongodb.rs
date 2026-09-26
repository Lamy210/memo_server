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

struct WriterAcquireContext {
    state: Collection<Document>,
    writers: Collection<Document>,
    lease_id: String,
}

struct MaintenanceAcquireContext {
    state: Collection<Document>,
    holder_token: String,
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
                released: false,
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
                released: false,
            }),
            Err(error) if error.get_custom::<MaintenanceActive>().is_some() => {
                Err(AppError::Conflict(
                    "HIGH search maintenance window is already active".into(),
                ))
            }
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
    released: bool,
}

impl MongoMemoMutationPermit {
    async fn release_inner(&mut self) -> AppResult<()> {
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

        self.released = true;
        Ok(())
    }
}

#[async_trait]
impl MemoMutationPermit for MongoMemoMutationPermit {
    async fn release(mut self: Box<Self>) -> AppResult<()> {
        self.release_inner().await
    }
}

impl Drop for MongoMemoMutationPermit {
    fn drop(&mut self) {
        if self.released {
            return;
        }

        let writers = self.writers.clone();
        let lease_id = self.lease_id.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                if let Err(error) = writers.delete_one(doc! { "_id": lease_id }).await {
                    log::error!(
                        "Failed to clean up dropped HIGH search writer lease; maintenance remains fail-closed: {error}"
                    );
                }
            });
        }
    }
}

struct MongoHighSearchOfflineWindowPermit {
    state: Collection<Document>,
    holder_token: String,
    released: bool,
}

impl MongoHighSearchOfflineWindowPermit {
    async fn release_inner(&mut self) -> AppResult<()> {
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

        self.released = true;
        Ok(())
    }
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

    async fn release(mut self: Box<Self>) -> AppResult<()> {
        self.release_inner().await
    }
}

impl Drop for MongoHighSearchOfflineWindowPermit {
    fn drop(&mut self) {
        if self.released {
            return;
        }

        let state = self.state.clone();
        let holder_token = self.holder_token.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                if let Err(error) = state
                    .update_one(
                        doc! {
                            "_id": STATE_ID,
                            "mode": MODE_MAINTENANCE,
                            "holder_token": holder_token,
                        },
                        doc! {
                            "$set": { "mode": MODE_OPEN },
                            "$unset": { "holder_token": "" },
                        },
                    )
                    .await
                {
                    log::error!(
                        "Failed to clean up dropped HIGH search maintenance barrier; writes remain fail-closed: {error}"
                    );
                }
            });
        }
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
}
