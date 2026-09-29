use std::time::Duration;

use ::mongodb::{
    bson::{doc, Bson, Document},
    error::Error as MongoError,
    options::{ReturnDocument, WriteConcern},
    Client, Collection, Database,
};
use async_trait::async_trait;
use futures::FutureExt;
use tokio::time::sleep;
use uuid::Uuid;

use crate::{
    application::{
        crypto_search_rotation::{HighSearchOfflineWindowGuard, HighSearchOfflineWindowPermit},
        high_memo_routing::{
            HighMemoDataRoute, HighMemoDataRouteReader, HighMemoDataRouteSnapshot,
            HighMemoPlaintextRetirementState,
        },
        high_search_routing::{
            HighSearchQueryRoute, HighSearchQueryRouteReader, HighSearchQueryRouteSnapshot,
        },
        maintenance::{
            HighMemoAccessGuard, HighMemoAccessPermit, HighSearchQueryGuard, HighSearchQueryPermit,
            MemoMutationGuard, MemoMutationPermit,
        },
    },
    error::{AppError, AppResult},
};

const STATE_COLLECTION: &str = "high_search_maintenance_state";
const WRITER_LEASES_COLLECTION: &str = "high_search_writer_leases";
const QUERY_LEASES_COLLECTION: &str = "high_search_query_leases";
const MEMO_ACCESS_LEASES_COLLECTION: &str = "high_memo_access_leases";
const STATE_ID: &str = "global";
const MODE_OPEN: &str = "open";
const MODE_MAINTENANCE: &str = "maintenance";
const QUERY_ROUTE_FIELD: &str = "query_route";
const QUERY_ROUTE_GENERATION_FIELD: &str = "query_route_generation";
const MEMO_ROUTE_FIELD: &str = "memo_route";
const MEMO_ROUTE_GENERATION_FIELD: &str = "memo_route_generation";
const MEMO_PLAINTEXT_RETIREMENT_STATE_FIELD: &str = "memo_plaintext_retirement_state";
const DRAIN_POLL_INTERVAL: Duration = Duration::from_millis(25);

#[derive(Debug)]
struct MaintenanceActive;

#[derive(Debug)]
struct RecoverySnapshotMismatch;

#[derive(Debug)]
struct InvalidQueryRouteState;

#[derive(Debug)]
struct InvalidMemoRouteState;

#[derive(Debug)]
struct InvalidMemoPlaintextRetirementState;

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
    query_route: HighSearchQueryRoute,
    query_route_generation: i64,
    memo_route: HighMemoDataRoute,
    memo_route_generation: i64,
    memo_plaintext_retirement_state: HighMemoPlaintextRetirementState,
    memo_route_changed_at_ms: Option<i64>,
    active_writer_leases: u64,
    active_query_leases: u64,
    active_memo_access_leases: u64,
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

    pub fn query_route(&self) -> HighSearchQueryRoute {
        self.query_route
    }

    pub fn query_route_generation(&self) -> i64 {
        self.query_route_generation
    }

    pub fn memo_route(&self) -> HighMemoDataRoute {
        self.memo_route
    }

    pub fn memo_route_generation(&self) -> i64 {
        self.memo_route_generation
    }

    pub fn memo_plaintext_retirement_state(&self) -> HighMemoPlaintextRetirementState {
        self.memo_plaintext_retirement_state
    }

    pub fn memo_route_changed_at_ms(&self) -> Option<i64> {
        self.memo_route_changed_at_ms
    }

    pub fn active_writer_leases(&self) -> u64 {
        self.active_writer_leases
    }

    pub fn active_query_leases(&self) -> u64 {
        self.active_query_leases
    }

    pub fn active_memo_access_leases(&self) -> u64 {
        self.active_memo_access_leases
    }
}

struct ActivityAcquireContext {
    state: Collection<Document>,
    leases: Collection<Document>,
    lease_id: String,
}

struct MaintenanceAcquireContext {
    state: Collection<Document>,
    holder_token: String,
}

struct RecoveryContext {
    state: Collection<Document>,
    writers: Collection<Document>,
    queries: Collection<Document>,
    memo_access: Collection<Document>,
    expected: HighSearchMaintenanceStatus,
}

/// Operator-only recovery boundary for fail-closed HIGH search maintenance state.
///
/// This type deliberately does not perform time-based lease expiry. Callers must
/// inspect the current state, stop all application replicas, and recover only
/// from the exact observed activity epoch, search route, memo route, and holder snapshot.
#[derive(Clone)]
pub struct MongoHighSearchMaintenanceRecovery {
    client: Client,
    database: Database,
    state: Collection<Document>,
    writers: Collection<Document>,
    queries: Collection<Document>,
    memo_access: Collection<Document>,
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
            database: database.clone(),
            state: database.collection(STATE_COLLECTION),
            writers: database.collection(WRITER_LEASES_COLLECTION),
            queries: database.collection(QUERY_LEASES_COLLECTION),
            memo_access: database.collection(MEMO_ACCESS_LEASES_COLLECTION),
        }
    }

    pub async fn server_time_ms(&self) -> AppResult<i64> {
        let response = self
            .database
            .run_command(doc! { "hello": 1_i32 })
            .await
            .map_err(|error| maintenance_db_error("read MongoDB server time", error))?;
        response
            .get_datetime("localTime")
            .map(|value| value.timestamp_millis())
            .map_err(|_| {
                AppError::ServiceUnavailable(
                    "MongoDB hello response is missing a valid localTime".into(),
                )
            })
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

        let route_snapshot = app_query_route_snapshot(&state)?;
        let memo_route_snapshot = app_memo_route_snapshot(&state)?;
        let memo_plaintext_retirement_state = app_plaintext_retirement_state(&state)?;
        validate_app_retirement_route_consistency(
            memo_route_snapshot.route,
            memo_plaintext_retirement_state,
        )?;
        let memo_route_changed_at_ms = match state.get("memo_route_changed_at") {
            None | Some(Bson::Null) => None,
            Some(Bson::DateTime(value)) => Some(value.timestamp_millis()),
            Some(_) => {
                return Err(AppError::ServiceUnavailable(
                    "HIGH maintenance state has an invalid memo_route_changed_at".into(),
                ))
            }
        };

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
        let active_query_leases = self
            .queries
            .count_documents(doc! {})
            .await
            .map_err(|error| maintenance_db_error("inspect protected query leases", error))?;
        let active_memo_access_leases = self
            .memo_access
            .count_documents(doc! {})
            .await
            .map_err(|error| maintenance_db_error("inspect memo access leases", error))?;

        Ok(HighSearchMaintenanceStatus {
            mode,
            holder_token,
            writer_epoch,
            query_route: route_snapshot.route,
            query_route_generation: route_snapshot.generation,
            memo_route: memo_route_snapshot.route,
            memo_route_generation: memo_route_snapshot.generation,
            memo_plaintext_retirement_state,
            memo_route_changed_at_ms,
            active_writer_leases,
            active_query_leases,
            active_memo_access_leases,
        })
    }

    /// Clear fail-closed writer leases and reopen the maintenance barrier from
    /// one exact operator-observed snapshot.
    ///
    /// The transaction matches mode, activity epoch, search route/generation,
    /// memo route/generation, plaintext-retirement state, and maintenance
    /// holder token (when present). Any intervening activity admission, route
    /// change, retirement transition, or barrier ownership change aborts
    /// recovery instead of clearing state from a newer generation.
    pub async fn recover_stale_state(
        &self,
        expected: &HighSearchMaintenanceStatus,
    ) -> AppResult<HighSearchMaintenanceStatus> {
        let mut session =
            self.client.start_session().await.map_err(|error| {
                maintenance_db_error("start maintenance recovery session", error)
            })?;
        let context = RecoveryContext {
            state: self.state.clone(),
            writers: self.writers.clone(),
            queries: self.queries.clone(),
            memo_access: self.memo_access.clone(),
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
                    context
                        .queries
                        .delete_many(doc! {})
                        .session(&mut *session)
                        .await?;
                    context
                        .memo_access
                        .delete_many(doc! {})
                        .session(&mut *session)
                        .await?;

                    let mut filter = doc! {
                        "_id": STATE_ID,
                        "mode": context.expected.mode.to_string(),
                        "writer_epoch": context.expected.writer_epoch,
                        "query_route": context.expected.query_route.as_persisted_str(),
                        "query_route_generation": context.expected.query_route_generation,
                        "memo_route": context.expected.memo_route.as_persisted_str(),
                        "memo_route_generation": context.expected.memo_route_generation,
                        "memo_plaintext_retirement_state": context
                            .expected
                            .memo_plaintext_retirement_state
                            .as_persisted_str(),
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
    queries: Collection<Document>,
    memo_access: Collection<Document>,
}

impl MongoHighSearchMaintenanceGuard {
    pub(crate) async fn new(database: Database) -> AppResult<Self> {
        let guard = Self {
            client: database.client().clone(),
            state: database.collection(STATE_COLLECTION),
            writers: database.collection(WRITER_LEASES_COLLECTION),
            queries: database.collection(QUERY_LEASES_COLLECTION),
            memo_access: database.collection(MEMO_ACCESS_LEASES_COLLECTION),
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
                        "query_route": HighSearchQueryRoute::Legacy.as_persisted_str(),
                        "query_route_generation": 0_i64,
                        "memo_route": HighMemoDataRoute::LegacyPlaintext.as_persisted_str(),
                        "memo_route_generation": 0_i64,
                        "memo_plaintext_retirement_state":
                            HighMemoPlaintextRetirementState::Available.as_persisted_str(),
                    }
                },
            )
            .upsert(true)
            .await;

        if let Err(error) = result {
            // Concurrent application startups may race the singleton upsert.
            // Accept that race only if the singleton now exists.
            if self
                .state
                .find_one(doc! { "_id": STATE_ID })
                .await
                .map_err(|lookup| maintenance_db_error("verify maintenance state", lookup))?
                .is_none()
            {
                return Err(maintenance_db_error(
                    "initialize HIGH search maintenance state",
                    error,
                ));
            }
        }

        // Upgrade the pre-routing singleton schema conservatively. A state
        // created before route support can only have served legacy user-visible
        // search, so the only safe automatic backfill is legacy generation 0.
        self.state
            .update_one(
                doc! { "_id": STATE_ID, "query_route": { "$exists": false } },
                doc! { "$set": { "query_route": HighSearchQueryRoute::Legacy.as_persisted_str() } },
            )
            .await
            .map_err(|error| maintenance_db_error("backfill HIGH search query route", error))?;
        self.state
            .update_one(
                doc! { "_id": STATE_ID, "query_route_generation": { "$exists": false } },
                doc! { "$set": { "query_route_generation": 0_i64 } },
            )
            .await
            .map_err(|error| {
                maintenance_db_error("backfill HIGH search query route generation", error)
            })?;
        self.state
            .update_one(
                doc! { "_id": STATE_ID, "memo_route": { "$exists": false } },
                doc! { "$set": { "memo_route": HighMemoDataRoute::LegacyPlaintext.as_persisted_str() } },
            )
            .await
            .map_err(|error| maintenance_db_error("backfill HIGH memo data route", error))?;
        self.state
            .update_one(
                doc! { "_id": STATE_ID, "memo_route_generation": { "$exists": false } },
                doc! { "$set": { "memo_route_generation": 0_i64 } },
            )
            .await
            .map_err(|error| {
                maintenance_db_error("backfill HIGH memo data route generation", error)
            })?;
        self.state
            .update_one(
                doc! {
                    "_id": STATE_ID,
                    "memo_plaintext_retirement_state": { "$exists": false }
                },
                doc! {
                    "$set": {
                        "memo_plaintext_retirement_state":
                            HighMemoPlaintextRetirementState::Available.as_persisted_str()
                    }
                },
            )
            .await
            .map_err(|error| {
                maintenance_db_error("backfill HIGH memo plaintext retirement state", error)
            })?;

        let state = self
            .state
            .find_one(doc! { "_id": STATE_ID })
            .await
            .map_err(|error| maintenance_db_error("verify HIGH search query route state", error))?
            .ok_or_else(|| {
                AppError::ServiceUnavailable(
                    "HIGH search maintenance state disappeared during initialization".into(),
                )
            })?;
        app_query_route_snapshot(&state)?;
        let memo_route = app_memo_route_snapshot(&state)?;
        let retirement = app_plaintext_retirement_state(&state)?;
        validate_app_retirement_route_consistency(memo_route.route, retirement)?;
        Ok(())
    }

    async fn acquire_activity_lease(
        &self,
        leases: Collection<Document>,
        activity: &'static str,
    ) -> AppResult<MongoMaintenanceActivityPermit> {
        let lease_id = Uuid::new_v4().to_string();
        let mut session = self.client.start_session().await.map_err(|error| {
            maintenance_db_error("start maintenance activity lease session", error)
        })?;
        let context = ActivityAcquireContext {
            state: self.state.clone(),
            leases: leases.clone(),
            lease_id: lease_id.clone(),
        };

        let result = session
            .start_transaction()
            .write_concern(WriteConcern::majority())
            .and_run(context, |session, context| {
                async move {
                    // Every admitted activity and maintenance acquisition writes
                    // the singleton gate. Returning the updated document here
                    // also makes the route decision and query-lease admission
                    // one MongoDB transaction rather than a racy two-step read.
                    let gate = context
                        .state
                        .find_one_and_update(
                            doc! { "_id": STATE_ID, "mode": MODE_OPEN },
                            doc! { "$inc": { "writer_epoch": 1_i64 } },
                        )
                        .return_document(ReturnDocument::After)
                        .session(&mut *session)
                        .await?
                        .ok_or_else(|| MongoError::custom(MaintenanceActive))?;
                    let query_route_snapshot = mongo_query_route_snapshot(&gate)?;
                    let memo_route_snapshot = mongo_memo_route_snapshot(&gate)?;
                    let retirement_state = mongo_plaintext_retirement_state(&gate)?;
                    validate_mongo_retirement_route_consistency(
                        memo_route_snapshot.route,
                        retirement_state,
                    )?;

                    context
                        .leases
                        .insert_one(doc! { "_id": context.lease_id.clone() })
                        .session(&mut *session)
                        .await?;
                    Ok((query_route_snapshot, memo_route_snapshot))
                }
                .boxed()
            })
            .await;

        match result {
            Ok((query_route_snapshot, memo_route_snapshot)) => Ok(MongoMaintenanceActivityPermit {
                leases,
                lease_id,
                activity,
                query_route_snapshot,
                memo_route_snapshot,
            }),
            Err(error) if error.get_custom::<MaintenanceActive>().is_some() => {
                Err(AppError::ServiceUnavailable(format!(
                    "{activity} are temporarily frozen by HIGH search maintenance"
                )))
            }
            Err(error) if error.get_custom::<InvalidQueryRouteState>().is_some() => Err(
                AppError::ServiceUnavailable("HIGH search query route state is invalid".into()),
            ),
            Err(error) if error.get_custom::<InvalidMemoRouteState>().is_some() => Err(
                AppError::ServiceUnavailable("HIGH memo data route state is invalid".into()),
            ),
            Err(error)
                if error
                    .get_custom::<InvalidMemoPlaintextRetirementState>()
                    .is_some() =>
            {
                Err(AppError::ServiceUnavailable(
                    "HIGH memo plaintext retirement state is invalid".into(),
                ))
            }
            Err(error) => Err(maintenance_db_error(
                "acquire HIGH search maintenance activity lease",
                error,
            )),
        }
    }

    async fn acquire_writer_lease(&self) -> AppResult<MongoMaintenanceActivityPermit> {
        self.acquire_activity_lease(self.writers.clone(), "memo mutations")
            .await
    }

    async fn acquire_query_lease(&self) -> AppResult<MongoMaintenanceActivityPermit> {
        self.acquire_activity_lease(self.queries.clone(), "protected HIGH search queries")
            .await
    }

    async fn acquire_memo_access_lease(&self) -> AppResult<MongoMaintenanceActivityPermit> {
        self.acquire_activity_lease(self.memo_access.clone(), "memo data-path requests")
            .await
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

    async fn active_query_count(&self) -> AppResult<u64> {
        self.queries
            .count_documents(doc! {})
            .await
            .map_err(|error| maintenance_db_error("count active protected query leases", error))
    }

    async fn active_memo_access_count(&self) -> AppResult<u64> {
        self.memo_access
            .count_documents(doc! {})
            .await
            .map_err(|error| maintenance_db_error("count active memo access leases", error))
    }
}

struct MongoMaintenanceActivityPermit {
    leases: Collection<Document>,
    lease_id: String,
    activity: &'static str,
    query_route_snapshot: HighSearchQueryRouteSnapshot,
    memo_route_snapshot: HighMemoDataRouteSnapshot,
}

impl MongoMaintenanceActivityPermit {
    async fn release_lease(self) -> AppResult<()> {
        let result = self
            .leases
            .delete_one(doc! { "_id": self.lease_id })
            .await
            .map_err(|error| maintenance_db_error("release maintenance activity lease", error))?;

        if result.deleted_count != 1 {
            return Err(AppError::ServiceUnavailable(format!(
                "{} lease disappeared before release",
                self.activity
            )));
        }

        Ok(())
    }
}

#[async_trait]
impl MemoMutationPermit for MongoMaintenanceActivityPermit {
    async fn release(self: Box<Self>) -> AppResult<()> {
        (*self).release_lease().await
    }
}

#[async_trait]
impl HighMemoAccessPermit for MongoMaintenanceActivityPermit {
    fn route_snapshot(&self) -> HighMemoDataRouteSnapshot {
        self.memo_route_snapshot
    }

    async fn release(self: Box<Self>) -> AppResult<()> {
        (*self).release_lease().await
    }
}

#[async_trait]
impl HighSearchQueryPermit for MongoMaintenanceActivityPermit {
    fn route_snapshot(&self) -> HighSearchQueryRouteSnapshot {
        self.query_route_snapshot
    }

    async fn release(self: Box<Self>) -> AppResult<()> {
        (*self).release_lease().await
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

    async fn current_query_route(&self) -> AppResult<HighSearchQueryRouteSnapshot> {
        let state = self
            .state
            .find_one(doc! {
                "_id": STATE_ID,
                "mode": MODE_MAINTENANCE,
                "holder_token": self.holder_token.clone(),
            })
            .await
            .map_err(|error| maintenance_db_error("read HIGH search query route", error))?
            .ok_or_else(|| {
                AppError::Conflict("HIGH search maintenance barrier ownership was lost".into())
            })?;
        app_query_route_snapshot(&state)
    }

    async fn switch_query_route(
        &self,
        expected: HighSearchQueryRouteSnapshot,
        target: HighSearchQueryRoute,
    ) -> AppResult<HighSearchQueryRouteSnapshot> {
        if expected.generation < 0 || expected.generation == i64::MAX {
            return Err(AppError::Conflict(
                "HIGH search query route generation cannot advance safely".into(),
            ));
        }

        if target == expected.route {
            let current = self.current_query_route().await?;
            if current == expected {
                return Ok(current);
            }
            return Err(AppError::Conflict(
                "HIGH search query route changed before idempotent cutover validation".into(),
            ));
        }

        let update = self
            .state
            .update_one(
                doc! {
                    "_id": STATE_ID,
                    "mode": MODE_MAINTENANCE,
                    "holder_token": self.holder_token.clone(),
                    "query_route": expected.route.as_persisted_str(),
                    "query_route_generation": expected.generation,
                },
                doc! {
                    "$set": { "query_route": target.as_persisted_str() },
                    "$inc": { "query_route_generation": 1_i64 },
                },
            )
            .await
            .map_err(|error| maintenance_db_error("switch HIGH search query route", error))?;

        if update.matched_count != 1 {
            return Err(AppError::Conflict(
                "HIGH search query route changed before cutover".into(),
            ));
        }

        self.current_query_route().await
    }

    async fn current_memo_route(&self) -> AppResult<HighMemoDataRouteSnapshot> {
        let state = self
            .state
            .find_one(doc! {
                "_id": STATE_ID,
                "mode": MODE_MAINTENANCE,
                "holder_token": self.holder_token.clone(),
            })
            .await
            .map_err(|error| maintenance_db_error("read HIGH memo data route", error))?
            .ok_or_else(|| {
                AppError::Conflict("HIGH maintenance barrier ownership was lost".into())
            })?;
        app_memo_route_snapshot(&state)
    }

    async fn switch_memo_route(
        &self,
        expected: HighMemoDataRouteSnapshot,
        target: HighMemoDataRoute,
    ) -> AppResult<HighMemoDataRouteSnapshot> {
        if expected.generation < 0 || expected.generation == i64::MAX {
            return Err(AppError::Conflict(
                "HIGH memo data route generation cannot advance safely".into(),
            ));
        }

        if target == expected.route {
            let current = self.current_memo_route().await?;
            if current != expected {
                return Err(AppError::Conflict(
                    "HIGH memo data route changed before idempotent cutover validation".into(),
                ));
            }
            if target == HighMemoDataRoute::LegacyPlaintext
                && !self
                    .current_plaintext_retirement_state()
                    .await?
                    .legacy_rollback_allowed()
            {
                return Err(AppError::Conflict(
                    "MEMO-HIGH-1 plaintext retirement has started; legacy rollback is permanently fenced"
                        .into(),
                ));
            }
            return Ok(current);
        }

        let mut filter = doc! {
            "_id": STATE_ID,
            "mode": MODE_MAINTENANCE,
            "holder_token": self.holder_token.clone(),
            "memo_route": expected.route.as_persisted_str(),
            "memo_route_generation": expected.generation,
        };
        if target == HighMemoDataRoute::LegacyPlaintext {
            filter.insert(
                MEMO_PLAINTEXT_RETIREMENT_STATE_FIELD,
                HighMemoPlaintextRetirementState::Available.as_persisted_str(),
            );
        }

        let update = self
            .state
            .update_one(
                filter,
                doc! {
                    "$set": { "memo_route": target.as_persisted_str() },
                    "$inc": { "memo_route_generation": 1_i64 },
                    "$currentDate": { "memo_route_changed_at": true },
                },
            )
            .await
            .map_err(|error| maintenance_db_error("switch HIGH memo data route", error))?;

        if update.matched_count != 1 {
            return Err(AppError::Conflict(
                "HIGH memo data route changed or plaintext retirement forbids legacy rollback"
                    .into(),
            ));
        }

        self.current_memo_route().await
    }

    async fn current_plaintext_retirement_state(
        &self,
    ) -> AppResult<HighMemoPlaintextRetirementState> {
        let state = self
            .state
            .find_one(doc! {
                "_id": STATE_ID,
                "mode": MODE_MAINTENANCE,
                "holder_token": self.holder_token.clone(),
            })
            .await
            .map_err(|error| {
                maintenance_db_error("read HIGH memo plaintext retirement state", error)
            })?
            .ok_or_else(|| {
                AppError::Conflict("HIGH maintenance barrier ownership was lost".into())
            })?;
        let retirement = app_plaintext_retirement_state(&state)?;
        let memo_route = app_memo_route_snapshot(&state)?;
        validate_app_retirement_route_consistency(memo_route.route, retirement)?;
        Ok(retirement)
    }

    async fn begin_plaintext_retirement(&self) -> AppResult<HighMemoPlaintextRetirementState> {
        let current = self.current_plaintext_retirement_state().await?;
        match current {
            HighMemoPlaintextRetirementState::InProgress => return Ok(current),
            HighMemoPlaintextRetirementState::Retired => {
                return Err(AppError::Conflict(
                    "MEMO-HIGH-1 plaintext retirement is already complete".into(),
                ))
            }
            HighMemoPlaintextRetirementState::Available => {}
        }

        let update = self
            .state
            .update_one(
                doc! {
                    "_id": STATE_ID,
                    "mode": MODE_MAINTENANCE,
                    "holder_token": self.holder_token.clone(),
                    "memo_route": HighMemoDataRoute::Encrypted.as_persisted_str(),
                    MEMO_PLAINTEXT_RETIREMENT_STATE_FIELD:
                        HighMemoPlaintextRetirementState::Available.as_persisted_str(),
                },
                doc! {
                    "$set": {
                        MEMO_PLAINTEXT_RETIREMENT_STATE_FIELD:
                            HighMemoPlaintextRetirementState::InProgress.as_persisted_str()
                    }
                },
            )
            .await
            .map_err(|error| maintenance_db_error("begin HIGH memo plaintext retirement", error))?;

        if update.matched_count != 1 {
            return Err(AppError::Conflict(
                "MEMO-HIGH-1 plaintext retirement requires the encrypted route and available retirement state"
                    .into(),
            ));
        }

        self.current_plaintext_retirement_state().await
    }

    async fn finish_plaintext_retirement(&self) -> AppResult<HighMemoPlaintextRetirementState> {
        let current = self.current_plaintext_retirement_state().await?;
        match current {
            HighMemoPlaintextRetirementState::Retired => return Ok(current),
            HighMemoPlaintextRetirementState::Available => {
                return Err(AppError::Conflict(
                    "MEMO-HIGH-1 plaintext retirement has not started".into(),
                ))
            }
            HighMemoPlaintextRetirementState::InProgress => {}
        }

        let update = self
            .state
            .update_one(
                doc! {
                    "_id": STATE_ID,
                    "mode": MODE_MAINTENANCE,
                    "holder_token": self.holder_token.clone(),
                    "memo_route": HighMemoDataRoute::Encrypted.as_persisted_str(),
                    MEMO_PLAINTEXT_RETIREMENT_STATE_FIELD:
                        HighMemoPlaintextRetirementState::InProgress.as_persisted_str(),
                },
                doc! {
                    "$set": {
                        MEMO_PLAINTEXT_RETIREMENT_STATE_FIELD:
                            HighMemoPlaintextRetirementState::Retired.as_persisted_str()
                    }
                },
            )
            .await
            .map_err(|error| maintenance_db_error("finish HIGH memo plaintext retirement", error))?;

        if update.matched_count != 1 {
            return Err(AppError::Conflict(
                "MEMO-HIGH-1 plaintext retirement state changed before completion".into(),
            ));
        }

        self.current_plaintext_retirement_state().await
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
impl HighMemoAccessGuard for MongoHighSearchMaintenanceGuard {
    async fn acquire_access(&self) -> AppResult<Box<dyn HighMemoAccessPermit>> {
        Ok(Box::new(self.acquire_memo_access_lease().await?))
    }
}

#[async_trait]
impl HighSearchQueryGuard for MongoHighSearchMaintenanceGuard {
    async fn acquire_query(&self) -> AppResult<Box<dyn HighSearchQueryPermit>> {
        Ok(Box::new(self.acquire_query_lease().await?))
    }
}

#[async_trait]
impl HighMemoDataRouteReader for MongoHighSearchMaintenanceGuard {
    async fn current_memo_data_route(&self) -> AppResult<HighMemoDataRouteSnapshot> {
        let state = self
            .state
            .find_one(doc! { "_id": STATE_ID })
            .await
            .map_err(|error| maintenance_db_error("read HIGH memo data route state", error))?
            .ok_or_else(|| {
                AppError::ServiceUnavailable("HIGH maintenance state is not initialized".into())
            })?;
        app_memo_route_snapshot(&state)
    }
}

#[async_trait]
impl HighSearchQueryRouteReader for MongoHighSearchMaintenanceGuard {
    async fn current_query_route(&self) -> AppResult<HighSearchQueryRouteSnapshot> {
        let state = self
            .state
            .find_one(doc! { "_id": STATE_ID })
            .await
            .map_err(|error| maintenance_db_error("read HIGH search query route state", error))?
            .ok_or_else(|| {
                AppError::ServiceUnavailable(
                    "HIGH search maintenance state is not initialized".into(),
                )
            })?;
        app_query_route_snapshot(&state)
    }
}

#[async_trait]
impl HighSearchOfflineWindowGuard for MongoHighSearchMaintenanceGuard {
    async fn acquire_offline_window(&self) -> AppResult<Box<dyn HighSearchOfflineWindowPermit>> {
        let permit = self.acquire_maintenance_barrier().await?;

        loop {
            permit.assert_still_enforced().await?;
            if self.active_writer_count().await? == 0
                && self.active_query_count().await? == 0
                && self.active_memo_access_count().await? == 0
            {
                return Ok(Box::new(permit));
            }
            sleep(DRAIN_POLL_INTERVAL).await;
        }
    }
}

fn mongo_query_route_snapshot(
    state: &Document,
) -> Result<HighSearchQueryRouteSnapshot, MongoError> {
    let route = state
        .get_str(QUERY_ROUTE_FIELD)
        .ok()
        .and_then(HighSearchQueryRoute::from_persisted_str)
        .ok_or_else(|| MongoError::custom(InvalidQueryRouteState))?;
    let generation = state
        .get_i64(QUERY_ROUTE_GENERATION_FIELD)
        .map_err(|_| MongoError::custom(InvalidQueryRouteState))?;
    if generation < 0 {
        return Err(MongoError::custom(InvalidQueryRouteState));
    }
    Ok(HighSearchQueryRouteSnapshot { route, generation })
}

fn app_query_route_snapshot(state: &Document) -> AppResult<HighSearchQueryRouteSnapshot> {
    let route = state
        .get_str(QUERY_ROUTE_FIELD)
        .ok()
        .and_then(HighSearchQueryRoute::from_persisted_str)
        .ok_or_else(|| {
            AppError::ServiceUnavailable("HIGH search query route state is invalid".into())
        })?;
    let generation = state.get_i64(QUERY_ROUTE_GENERATION_FIELD).map_err(|_| {
        AppError::ServiceUnavailable("HIGH search query route generation is invalid".into())
    })?;
    if generation < 0 {
        return Err(AppError::ServiceUnavailable(
            "HIGH search query route generation is invalid".into(),
        ));
    }
    Ok(HighSearchQueryRouteSnapshot { route, generation })
}

fn mongo_memo_route_snapshot(state: &Document) -> Result<HighMemoDataRouteSnapshot, MongoError> {
    let route = state
        .get_str(MEMO_ROUTE_FIELD)
        .ok()
        .and_then(HighMemoDataRoute::from_persisted_str)
        .ok_or_else(|| MongoError::custom(InvalidMemoRouteState))?;
    let generation = state
        .get_i64(MEMO_ROUTE_GENERATION_FIELD)
        .map_err(|_| MongoError::custom(InvalidMemoRouteState))?;
    if generation < 0 {
        return Err(MongoError::custom(InvalidMemoRouteState));
    }
    Ok(HighMemoDataRouteSnapshot { route, generation })
}

fn app_memo_route_snapshot(state: &Document) -> AppResult<HighMemoDataRouteSnapshot> {
    let route = state
        .get_str(MEMO_ROUTE_FIELD)
        .ok()
        .and_then(HighMemoDataRoute::from_persisted_str)
        .ok_or_else(|| {
            AppError::ServiceUnavailable("HIGH memo data route state is invalid".into())
        })?;
    let generation = state.get_i64(MEMO_ROUTE_GENERATION_FIELD).map_err(|_| {
        AppError::ServiceUnavailable("HIGH memo data route generation is invalid".into())
    })?;
    if generation < 0 {
        return Err(AppError::ServiceUnavailable(
            "HIGH memo data route generation is invalid".into(),
        ));
    }
    Ok(HighMemoDataRouteSnapshot { route, generation })
}

fn mongo_plaintext_retirement_state(
    state: &Document,
) -> Result<HighMemoPlaintextRetirementState, MongoError> {
    state
        .get_str(MEMO_PLAINTEXT_RETIREMENT_STATE_FIELD)
        .ok()
        .and_then(HighMemoPlaintextRetirementState::from_persisted_str)
        .ok_or_else(|| MongoError::custom(InvalidMemoPlaintextRetirementState))
}

fn app_plaintext_retirement_state(
    state: &Document,
) -> AppResult<HighMemoPlaintextRetirementState> {
    state
        .get_str(MEMO_PLAINTEXT_RETIREMENT_STATE_FIELD)
        .ok()
        .and_then(HighMemoPlaintextRetirementState::from_persisted_str)
        .ok_or_else(|| {
            AppError::ServiceUnavailable(
                "HIGH memo plaintext retirement state is invalid".into(),
            )
        })
}

fn validate_mongo_retirement_route_consistency(
    memo_route: HighMemoDataRoute,
    retirement: HighMemoPlaintextRetirementState,
) -> Result<(), MongoError> {
    if retirement.legacy_rollback_allowed() || memo_route == HighMemoDataRoute::Encrypted {
        Ok(())
    } else {
        Err(MongoError::custom(InvalidMemoPlaintextRetirementState))
    }
}

fn validate_app_retirement_route_consistency(
    memo_route: HighMemoDataRoute,
    retirement: HighMemoPlaintextRetirementState,
) -> AppResult<()> {
    if retirement.legacy_rollback_allowed() || memo_route == HighMemoDataRoute::Encrypted {
        Ok(())
    } else {
        Err(AppError::ServiceUnavailable(
            "HIGH memo plaintext retirement state requires the encrypted memo route".into(),
        ))
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
    async fn mongodb_maintenance_barrier_drains_writers_queries_and_memo_access() {
        let uri = std::env::var("MONGODB_TEST_URI")
            .unwrap_or_else(|_| "mongodb://localhost:27017/?replicaSet=rs0".to_string());
        let client = Client::with_uri_str(&uri).await.unwrap();
        let database = client.database(TEST_DATABASE_NAME);
        database.drop().await.unwrap();

        let guard = MongoHighSearchMaintenanceGuard::new(database.clone())
            .await
            .unwrap();
        let recovery_clock = MongoHighSearchMaintenanceRecovery::from_database(database.clone());
        assert!(recovery_clock.server_time_ms().await.unwrap() > 0);
        let first_writer = guard.acquire_mutation().await.unwrap();
        let first_query = guard.acquire_query().await.unwrap();
        let first_memo_access = guard.acquire_access().await.unwrap();
        assert_eq!(
            first_query.route_snapshot(),
            HighSearchQueryRouteSnapshot {
                route: HighSearchQueryRoute::Legacy,
                generation: 0,
            }
        );
        assert_eq!(
            first_memo_access.route_snapshot(),
            HighMemoDataRouteSnapshot {
                route: HighMemoDataRoute::LegacyPlaintext,
                generation: 0,
            }
        );

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
        assert!(matches!(
            guard.acquire_query().await,
            Err(AppError::ServiceUnavailable(_))
        ));
        assert!(matches!(
            guard.acquire_access().await,
            Err(AppError::ServiceUnavailable(_))
        ));
        assert!(!maintenance_task.is_finished());

        first_writer.release().await.unwrap();
        assert!(
            !maintenance_task.is_finished(),
            "maintenance must also drain the protected query lease"
        );
        first_query.release().await.unwrap();
        assert!(
            !maintenance_task.is_finished(),
            "maintenance must also drain the memo data-path lease"
        );
        first_memo_access.release().await.unwrap();
        let maintenance = maintenance_task.await.unwrap().unwrap();
        maintenance.assert_still_enforced().await.unwrap();

        let legacy_route = maintenance.current_query_route().await.unwrap();
        assert_eq!(
            legacy_route,
            HighSearchQueryRouteSnapshot {
                route: HighSearchQueryRoute::Legacy,
                generation: 0,
            }
        );
        let protected_route = maintenance
            .switch_query_route(legacy_route, HighSearchQueryRoute::Protected)
            .await
            .unwrap();
        assert_eq!(
            protected_route,
            HighSearchQueryRouteSnapshot {
                route: HighSearchQueryRoute::Protected,
                generation: 1,
            }
        );
        assert_eq!(
            maintenance
                .switch_query_route(protected_route, HighSearchQueryRoute::Protected)
                .await
                .unwrap(),
            protected_route
        );
        assert!(matches!(
            maintenance
                .switch_query_route(legacy_route, HighSearchQueryRoute::Legacy)
                .await,
            Err(AppError::Conflict(_))
        ));

        let legacy_memo_route = maintenance.current_memo_route().await.unwrap();
        assert_eq!(
            legacy_memo_route,
            HighMemoDataRouteSnapshot {
                route: HighMemoDataRoute::LegacyPlaintext,
                generation: 0,
            }
        );
        let encrypted_memo_route = maintenance
            .switch_memo_route(legacy_memo_route, HighMemoDataRoute::Encrypted)
            .await
            .unwrap();
        assert_eq!(
            encrypted_memo_route,
            HighMemoDataRouteSnapshot {
                route: HighMemoDataRoute::Encrypted,
                generation: 1,
            }
        );
        let route_status = MongoHighSearchMaintenanceRecovery::from_database(database.clone())
            .inspect()
            .await
            .unwrap();
        assert_eq!(route_status.memo_route(), HighMemoDataRoute::Encrypted);
        assert_eq!(route_status.memo_route_generation(), 1);
        assert!(route_status.memo_route_changed_at_ms().is_some());
        assert_eq!(
            maintenance
                .switch_memo_route(encrypted_memo_route, HighMemoDataRoute::Encrypted)
                .await
                .unwrap(),
            encrypted_memo_route
        );
        assert!(matches!(
            maintenance
                .switch_memo_route(legacy_memo_route, HighMemoDataRoute::LegacyPlaintext)
                .await,
            Err(AppError::Conflict(_))
        ));

        assert!(matches!(
            guard.acquire_mutation().await,
            Err(AppError::ServiceUnavailable(_))
        ));
        assert!(matches!(
            guard.acquire_query().await,
            Err(AppError::ServiceUnavailable(_))
        ));
        assert!(matches!(
            guard.acquire_access().await,
            Err(AppError::ServiceUnavailable(_))
        ));

        maintenance.release().await.unwrap();
        let writer_after_release = guard.acquire_mutation().await.unwrap();
        writer_after_release.release().await.unwrap();
        let query_after_release = guard.acquire_query().await.unwrap();
        assert_eq!(
            query_after_release.route_snapshot(),
            HighSearchQueryRouteSnapshot {
                route: HighSearchQueryRoute::Protected,
                generation: 1,
            }
        );
        query_after_release.release().await.unwrap();
        let memo_access_after_release = guard.acquire_access().await.unwrap();
        assert_eq!(
            memo_access_after_release.route_snapshot(),
            HighMemoDataRouteSnapshot {
                route: HighMemoDataRoute::Encrypted,
                generation: 1,
            }
        );
        memo_access_after_release.release().await.unwrap();

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

        // Simulate cancelled mutation/query work: dropping without explicit
        // release intentionally leaves fail-closed activity leases behind.
        let abandoned_writer = guard.acquire_mutation().await.unwrap();
        let abandoned_query = guard.acquire_query().await.unwrap();
        let abandoned_memo_access = guard.acquire_access().await.unwrap();
        drop(abandoned_writer);
        drop(abandoned_query);
        drop(abandoned_memo_access);

        let open_snapshot = recovery.inspect().await.unwrap();
        assert_eq!(open_snapshot.mode(), HighSearchMaintenanceMode::Open);
        assert_eq!(open_snapshot.query_route(), HighSearchQueryRoute::Legacy);
        assert_eq!(open_snapshot.query_route_generation(), 0);
        assert_eq!(
            open_snapshot.memo_route(),
            HighMemoDataRoute::LegacyPlaintext
        );
        assert_eq!(open_snapshot.memo_route_generation(), 0);
        assert_eq!(open_snapshot.active_writer_leases(), 1);
        assert_eq!(open_snapshot.active_query_leases(), 1);
        assert_eq!(open_snapshot.active_memo_access_leases(), 1);

        let mut stale_snapshot = open_snapshot.clone();
        stale_snapshot.writer_epoch -= 1;
        assert!(matches!(
            recovery.recover_stale_state(&stale_snapshot).await,
            Err(AppError::Conflict(_))
        ));
        let after_stale = recovery.inspect().await.unwrap();
        assert_eq!(after_stale.active_writer_leases(), 1);
        assert_eq!(after_stale.active_query_leases(), 1);
        assert_eq!(after_stale.active_memo_access_leases(), 1);

        let mut stale_route_snapshot = open_snapshot.clone();
        stale_route_snapshot.query_route_generation += 1;
        assert!(matches!(
            recovery.recover_stale_state(&stale_route_snapshot).await,
            Err(AppError::Conflict(_))
        ));
        let after_stale_route = recovery.inspect().await.unwrap();
        assert_eq!(after_stale_route.active_writer_leases(), 1);
        assert_eq!(after_stale_route.active_query_leases(), 1);
        assert_eq!(after_stale_route.active_memo_access_leases(), 1);

        let mut stale_memo_route_snapshot = open_snapshot.clone();
        stale_memo_route_snapshot.memo_route_generation += 1;
        assert!(matches!(
            recovery
                .recover_stale_state(&stale_memo_route_snapshot)
                .await,
            Err(AppError::Conflict(_))
        ));
        let after_stale_memo_route = recovery.inspect().await.unwrap();
        assert_eq!(after_stale_memo_route.active_writer_leases(), 1);
        assert_eq!(after_stale_memo_route.active_query_leases(), 1);
        assert_eq!(after_stale_memo_route.active_memo_access_leases(), 1);

        let recovered = recovery.recover_stale_state(&open_snapshot).await.unwrap();
        assert_eq!(recovered.mode(), HighSearchMaintenanceMode::Open);
        assert_eq!(recovered.query_route(), HighSearchQueryRoute::Legacy);
        assert_eq!(recovered.query_route_generation(), 0);
        assert_eq!(recovered.active_writer_leases(), 0);
        assert_eq!(recovered.active_query_leases(), 0);
        assert_eq!(recovered.active_memo_access_leases(), 0);
        assert_eq!(recovered.memo_route(), HighMemoDataRoute::LegacyPlaintext);
        assert_eq!(recovered.memo_route_generation(), 0);
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
        assert_eq!(maintenance_snapshot.active_query_leases(), 0);
        assert_eq!(maintenance_snapshot.active_memo_access_leases(), 0);
        assert!(matches!(
            guard.acquire_mutation().await,
            Err(AppError::ServiceUnavailable(_))
        ));
        assert!(matches!(
            guard.acquire_query().await,
            Err(AppError::ServiceUnavailable(_))
        ));
        assert!(matches!(
            guard.acquire_access().await,
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
        let query_after_recovery = guard.acquire_query().await.unwrap();
        query_after_recovery.release().await.unwrap();
        let memo_access_after_recovery = guard.acquire_access().await.unwrap();
        memo_access_after_recovery.release().await.unwrap();

        database.drop().await.unwrap();
    }
}
