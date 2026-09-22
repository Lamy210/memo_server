use std::collections::HashMap;

use ::mongodb::{
    bson::{doc, Document},
    error::Error as MongoError,
    options::WriteConcern,
    Client, Collection, Database, IndexModel,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::{FutureExt, TryStreamExt};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    application::health::HealthProbe,
    domain::memo::entity::Memo,
    error::{AppError, AppResult},
};

use super::ports::{MemoAuthoritativeStore, ProjectionIntent, ProjectionTarget};

const TEST_DATABASE_NAME: &str = "memo_app_test";
const MEMOS_COLLECTION: &str = "memos";
const PROJECTION_INTENTS_COLLECTION: &str = "projection_intents";
const TARGET_VERSION: &str = "version";
const TARGET_DELETED: &str = "deleted";

#[derive(Clone, Debug, Serialize, Deserialize)]
struct MemoDocument {
    #[serde(rename = "_id")]
    id: String,
    user_id: String,
    title: String,
    content: String,
    tags: Vec<String>,
    created_at_ms: i64,
    updated_at_ms: i64,
    version: i32,
}

impl From<&Memo> for MemoDocument {
    fn from(memo: &Memo) -> Self {
        Self {
            id: memo.id.to_string(),
            user_id: memo.user_id.to_string(),
            title: memo.title.clone(),
            content: memo.content.clone(),
            tags: memo.tags.clone(),
            created_at_ms: memo.created_at.timestamp_millis(),
            updated_at_ms: memo.updated_at.timestamp_millis(),
            version: memo.version,
        }
    }
}

impl MemoDocument {
    fn try_into_memo(self) -> AppResult<Memo> {
        let id = Uuid::parse_str(&self.id).map_err(|error| {
            AppError::DatabaseError(format!("Invalid MongoDB memo id: {error}"))
        })?;
        let user_id = Uuid::parse_str(&self.user_id).map_err(|error| {
            AppError::DatabaseError(format!("Invalid MongoDB memo user id: {error}"))
        })?;
        let created_at =
            DateTime::<Utc>::from_timestamp_millis(self.created_at_ms).ok_or_else(|| {
                AppError::DatabaseError(format!(
                    "Invalid MongoDB memo created_at milliseconds: {}",
                    self.created_at_ms
                ))
            })?;
        let updated_at =
            DateTime::<Utc>::from_timestamp_millis(self.updated_at_ms).ok_or_else(|| {
                AppError::DatabaseError(format!(
                    "Invalid MongoDB memo updated_at milliseconds: {}",
                    self.updated_at_ms
                ))
            })?;

        Ok(Memo {
            id,
            title: self.title,
            content: self.content,
            tags: self.tags,
            user_id,
            created_at,
            updated_at,
            version: self.version,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ProjectionIntentDocument {
    #[serde(rename = "_id")]
    event_id: String,
    user_id: String,
    memo_id: String,
    target_kind: String,
    target_version: Option<i32>,
}

impl ProjectionIntentDocument {
    fn from_intent(intent: &ProjectionIntent) -> AppResult<Self> {
        let (target_kind, target_version) = match intent.target {
            ProjectionTarget::Version(version) if version > 0 => {
                (TARGET_VERSION.to_string(), Some(version))
            }
            ProjectionTarget::Version(version) => {
                return Err(AppError::DatabaseError(format!(
                    "Projection version must be positive, got {version}"
                )));
            }
            ProjectionTarget::Deleted => (TARGET_DELETED.to_string(), None),
        };

        Ok(Self {
            event_id: intent.event_id.to_string(),
            user_id: intent.user_id.to_string(),
            memo_id: intent.memo_id.to_string(),
            target_kind,
            target_version,
        })
    }

    fn try_into_intent(self) -> AppResult<ProjectionIntent> {
        let event_id = parse_uuid("projection event id", &self.event_id)?;
        let user_id = parse_uuid("projection user id", &self.user_id)?;
        let memo_id = parse_uuid("projection memo id", &self.memo_id)?;
        let target = match (self.target_kind.as_str(), self.target_version) {
            (TARGET_VERSION, Some(version)) if version > 0 => ProjectionTarget::Version(version),
            (TARGET_DELETED, None) => ProjectionTarget::Deleted,
            _ => {
                return Err(AppError::DatabaseError(format!(
                    "Invalid MongoDB projection target: kind={} version={:?}",
                    self.target_kind, self.target_version
                )));
            }
        };

        Ok(ProjectionIntent {
            event_id,
            user_id,
            memo_id,
            target,
        })
    }
}

fn parse_uuid(field: &str, value: &str) -> AppResult<Uuid> {
    Uuid::parse_str(value)
        .map_err(|error| AppError::DatabaseError(format!("Invalid MongoDB {field}: {error}")))
}

#[derive(Debug)]
struct OptimisticConflict;

#[derive(Debug)]
struct MissingMemo;

struct SaveTransactionContext {
    memos: Collection<MemoDocument>,
    intents: Collection<ProjectionIntentDocument>,
    memo: MemoDocument,
    intent: ProjectionIntentDocument,
}

struct DeleteTransactionContext {
    memos: Collection<MemoDocument>,
    intents: Collection<ProjectionIntentDocument>,
    user_id: String,
    memo_id: String,
    intent: ProjectionIntentDocument,
}

pub struct MongoDbAuthoritativeStore {
    client: Client,
    database: Database,
    memos: Collection<MemoDocument>,
    projection_intents: Collection<ProjectionIntentDocument>,
}

impl MongoDbAuthoritativeStore {
    pub async fn new(uri: &str, database_name: &str) -> AppResult<Self> {
        let client = Client::with_uri_str(uri).await.map_err(|error| {
            AppError::DatabaseError(format!("Failed to create MongoDB client: {error}"))
        })?;
        let database = client.database(database_name);
        let memos = database.collection::<MemoDocument>(MEMOS_COLLECTION);
        let projection_intents =
            database.collection::<ProjectionIntentDocument>(PROJECTION_INTENTS_COLLECTION);

        let store = Self {
            client,
            database,
            memos,
            projection_intents,
        };
        store.validate_transaction_topology().await?;
        store.initialize_schema().await?;
        Ok(store)
    }

    async fn validate_transaction_topology(&self) -> AppResult<()> {
        let hello = self
            .client
            .database("admin")
            .run_command(doc! { "hello": 1 })
            .await
            .map_err(|error| mongo_error("inspect MongoDB topology", error))?;

        if transaction_topology_supported(&hello) {
            return Ok(());
        }

        Err(AppError::DatabaseError(
            "MongoDB authoritative storage requires a replica set or sharded cluster because memo mutations and projection intents must commit atomically"
                .to_string(),
        ))
    }

    async fn initialize_schema(&self) -> AppResult<()> {
        self.memos
            .create_index(
                IndexModel::builder()
                    .keys(doc! { "user_id": 1, "updated_at_ms": -1 })
                    .build(),
            )
            .await
            .map_err(|error| mongo_error("create MongoDB memo indexes", error))?;

        self.projection_intents
            .create_index(IndexModel::builder().keys(doc! { "memo_id": 1 }).build())
            .await
            .map_err(|error| mongo_error("create MongoDB projection intent indexes", error))?;

        Ok(())
    }

    async fn find_by_id_inner(&self, user_id: Uuid, id: Uuid) -> AppResult<Option<Memo>> {
        let document = self
            .memos
            .find_one(doc! {
                "_id": id.to_string(),
                "user_id": user_id.to_string(),
            })
            .await
            .map_err(|error| mongo_error("find MongoDB memo", error))?;

        document.map(MemoDocument::try_into_memo).transpose()
    }

    async fn save_transaction(&self, memo: &Memo, event: &ProjectionIntent) -> AppResult<()> {
        let mut session = self
            .client
            .start_session()
            .await
            .map_err(|error| mongo_error("start MongoDB session", error))?;
        let context = SaveTransactionContext {
            memos: self.memos.clone(),
            intents: self.projection_intents.clone(),
            memo: MemoDocument::from(memo),
            intent: ProjectionIntentDocument::from_intent(event)?,
        };

        let result = session
            .start_transaction()
            .write_concern(WriteConcern::majority())
            .and_run(context, |session, context| {
                async move {
                    if context.memo.version == 1 {
                        context
                            .memos
                            .insert_one(context.memo.clone())
                            .session(&mut *session)
                            .await?;
                    } else {
                        let expected_version = context.memo.version - 1;
                        let update = doc! {
                            "$set": {
                                "title": context.memo.title.clone(),
                                "content": context.memo.content.clone(),
                                "tags": context.memo.tags.clone(),
                                "updated_at_ms": context.memo.updated_at_ms,
                                "version": context.memo.version,
                            }
                        };
                        let result = context
                            .memos
                            .update_one(
                                doc! {
                                    "_id": context.memo.id.clone(),
                                    "user_id": context.memo.user_id.clone(),
                                    "version": expected_version,
                                },
                                update,
                            )
                            .session(&mut *session)
                            .await?;

                        if result.matched_count != 1 {
                            return Err(MongoError::custom(OptimisticConflict));
                        }
                    }

                    context
                        .intents
                        .insert_one(context.intent.clone())
                        .session(&mut *session)
                        .await?;

                    Ok(())
                }
                .boxed()
            })
            .await;

        match result {
            Ok(()) => Ok(()),
            Err(error) if error.get_custom::<OptimisticConflict>().is_some() => Err(
                AppError::Conflict("Memo has been updated by another client".into()),
            ),
            Err(error) => Err(mongo_error("commit MongoDB memo transaction", error)),
        }
    }

    async fn delete_transaction(
        &self,
        user_id: Uuid,
        id: Uuid,
        event: &ProjectionIntent,
    ) -> AppResult<()> {
        let mut session = self
            .client
            .start_session()
            .await
            .map_err(|error| mongo_error("start MongoDB session", error))?;
        let context = DeleteTransactionContext {
            memos: self.memos.clone(),
            intents: self.projection_intents.clone(),
            user_id: user_id.to_string(),
            memo_id: id.to_string(),
            intent: ProjectionIntentDocument::from_intent(event)?,
        };

        let result = session
            .start_transaction()
            .write_concern(WriteConcern::majority())
            .and_run(context, |session, context| {
                async move {
                    let result = context
                        .memos
                        .delete_one(doc! {
                            "_id": context.memo_id.clone(),
                            "user_id": context.user_id.clone(),
                        })
                        .session(&mut *session)
                        .await?;

                    if result.deleted_count != 1 {
                        return Err(MongoError::custom(MissingMemo));
                    }

                    context
                        .intents
                        .insert_one(context.intent.clone())
                        .session(&mut *session)
                        .await?;

                    Ok(())
                }
                .boxed()
            })
            .await;

        match result {
            Ok(()) => Ok(()),
            Err(error) if error.get_custom::<MissingMemo>().is_some() => {
                Err(AppError::NotFound("Memo not found".into()))
            }
            Err(error) => Err(mongo_error("commit MongoDB delete transaction", error)),
        }
    }

    pub async fn health_check(&self) -> AppResult<bool> {
        self.database
            .run_command(doc! { "ping": 1 })
            .await
            .map_err(|error| mongo_error("MongoDB health check", error))?;
        Ok(true)
    }
}

fn mongo_error(operation: &str, error: MongoError) -> AppError {
    AppError::DatabaseError(format!("{operation} failed: {error}"))
}

fn transaction_topology_supported(hello: &Document) -> bool {
    hello.get_str("setName").is_ok() || hello.get_str("msg").is_ok_and(|msg| msg == "isdbgrid")
}

fn order_memos_by_ids(ids: &[Uuid], by_id: &HashMap<Uuid, Memo>) -> Vec<Memo> {
    ids.iter().filter_map(|id| by_id.get(id).cloned()).collect()
}

#[async_trait]
impl MemoAuthoritativeStore for MongoDbAuthoritativeStore {
    async fn find_by_id(&self, user_id: Uuid, id: Uuid) -> AppResult<Option<Memo>> {
        self.find_by_id_inner(user_id, id).await
    }

    async fn find_all_by_user_id(&self, user_id: Uuid) -> AppResult<Vec<Memo>> {
        let documents: Vec<MemoDocument> = self
            .memos
            .find(doc! { "user_id": user_id.to_string() })
            .sort(doc! { "updated_at_ms": -1 })
            .await
            .map_err(|error| mongo_error("find MongoDB memos", error))?
            .try_collect()
            .await
            .map_err(|error| mongo_error("read MongoDB memo cursor", error))?;

        documents
            .into_iter()
            .map(MemoDocument::try_into_memo)
            .collect()
    }

    async fn find_many_by_ids(&self, user_id: Uuid, ids: &[Uuid]) -> AppResult<Vec<Memo>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }

        let requested_ids: Vec<String> = ids.iter().map(Uuid::to_string).collect();
        let documents: Vec<MemoDocument> = self
            .memos
            .find(doc! {
                "user_id": user_id.to_string(),
                "_id": { "$in": requested_ids.clone() },
            })
            .await
            .map_err(|error| mongo_error("find MongoDB memos by ids", error))?
            .try_collect()
            .await
            .map_err(|error| mongo_error("read MongoDB memo hydration cursor", error))?;

        let mut by_id = HashMap::with_capacity(documents.len());
        for document in documents {
            let memo = document.try_into_memo()?;
            by_id.insert(memo.id, memo);
        }

        Ok(order_memos_by_ids(ids, &by_id))
    }

    async fn save_with_projection_intent(&self, memo: &Memo) -> AppResult<ProjectionIntent> {
        let event = ProjectionIntent::new(
            memo.user_id,
            memo.id,
            ProjectionTarget::Version(memo.version),
        );
        self.save_transaction(memo, &event).await?;
        Ok(event)
    }

    async fn delete_with_projection_intent(
        &self,
        user_id: Uuid,
        id: Uuid,
    ) -> AppResult<ProjectionIntent> {
        let event = ProjectionIntent::new(user_id, id, ProjectionTarget::Deleted);
        self.delete_transaction(user_id, id, &event).await?;
        Ok(event)
    }

    async fn exists(&self, user_id: Uuid, id: Uuid) -> AppResult<bool> {
        Ok(self.find_by_id_inner(user_id, id).await?.is_some())
    }

    async fn enqueue_projection_intent(
        &self,
        user_id: Uuid,
        memo_id: Uuid,
        target: ProjectionTarget,
    ) -> AppResult<ProjectionIntent> {
        let event = ProjectionIntent::new(user_id, memo_id, target);
        let document = ProjectionIntentDocument::from_intent(&event)?;
        self.projection_intents
            .insert_one(document)
            .await
            .map_err(|error| mongo_error("enqueue MongoDB projection intent", error))?;
        Ok(event)
    }

    async fn list_projection_intents(&self) -> AppResult<Vec<ProjectionIntent>> {
        let documents: Vec<ProjectionIntentDocument> = self
            .projection_intents
            .find(doc! {})
            .await
            .map_err(|error| mongo_error("list MongoDB projection intents", error))?
            .try_collect()
            .await
            .map_err(|error| mongo_error("read MongoDB projection intent cursor", error))?;

        documents
            .into_iter()
            .map(ProjectionIntentDocument::try_into_intent)
            .collect()
    }

    async fn acknowledge_projection_intent(&self, event: &ProjectionIntent) -> AppResult<()> {
        self.projection_intents
            .delete_one(doc! {
                "_id": event.event_id.to_string(),
                "user_id": event.user_id.to_string(),
                "memo_id": event.memo_id.to_string(),
            })
            .await
            .map_err(|error| mongo_error("acknowledge MongoDB projection intent", error))?;
        Ok(())
    }
}

#[async_trait]
impl HealthProbe for MongoDbAuthoritativeStore {
    async fn check(&self) -> bool {
        match self.health_check().await {
            Ok(healthy) => healthy,
            Err(error) => {
                log::warn!("MongoDB health check failed: {error}");
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memo_document_round_trip_preserves_domain_fields() {
        let memo = Memo::new(
            "Title".into(),
            "Content".into(),
            vec!["one".into(), "two".into()],
            Uuid::new_v4(),
        );

        let restored = MemoDocument::from(&memo).try_into_memo().unwrap();

        assert_eq!(restored.id, memo.id);
        assert_eq!(restored.user_id, memo.user_id);
        assert_eq!(restored.title, memo.title);
        assert_eq!(restored.content, memo.content);
        assert_eq!(restored.tags, memo.tags);
        assert_eq!(
            restored.created_at.timestamp_millis(),
            memo.created_at.timestamp_millis()
        );
        assert_eq!(
            restored.updated_at.timestamp_millis(),
            memo.updated_at.timestamp_millis()
        );
        assert_eq!(restored.version, memo.version);
    }

    #[test]
    fn hydration_preserves_requested_order_and_omits_missing_ids() {
        let user_id = Uuid::new_v4();
        let mut first = Memo::new("First".into(), "Content".into(), vec![], user_id);
        let mut second = Memo::new("Second".into(), "Content".into(), vec![], user_id);
        first.id = Uuid::new_v4();
        second.id = Uuid::new_v4();
        let missing = Uuid::new_v4();

        let by_id = HashMap::from([(first.id, first.clone()), (second.id, second.clone())]);
        let ordered = order_memos_by_ids(&[second.id, missing, first.id, second.id], &by_id);

        assert_eq!(
            ordered.iter().map(|memo| memo.id).collect::<Vec<_>>(),
            vec![second.id, first.id, second.id]
        );
    }

    #[test]
    fn transaction_topology_requires_replica_set_or_mongos() {
        assert!(transaction_topology_supported(&doc! { "setName": "rs0" }));
        assert!(transaction_topology_supported(&doc! { "msg": "isdbgrid" }));
        assert!(!transaction_topology_supported(&doc! { "isWritablePrimary": true }));
    }

    #[test]
    fn projection_intent_round_trip_preserves_typed_target() {
        let versioned =
            ProjectionIntent::new(Uuid::new_v4(), Uuid::new_v4(), ProjectionTarget::Version(3));
        let deleted =
            ProjectionIntent::new(Uuid::new_v4(), Uuid::new_v4(), ProjectionTarget::Deleted);

        assert_eq!(
            ProjectionIntentDocument::from_intent(&versioned)
                .unwrap()
                .try_into_intent()
                .unwrap()
                .target,
            ProjectionTarget::Version(3)
        );
        assert_eq!(
            ProjectionIntentDocument::from_intent(&deleted)
                .unwrap()
                .try_into_intent()
                .unwrap()
                .target,
            ProjectionTarget::Deleted
        );
    }

    #[tokio::test]
    #[ignore = "requires a local MongoDB replica set"]
    async fn mongodb_replica_set_preserves_atomic_outbox_and_tenant_scope() {
        let uri = std::env::var("MONGODB_TEST_URI")
            .unwrap_or_else(|_| "mongodb://localhost:27017/?replicaSet=rs0".to_string());

        let cleanup_client = Client::with_uri_str(&uri).await.unwrap();
        cleanup_client
            .database(TEST_DATABASE_NAME)
            .drop()
            .await
            .unwrap();

        let store = MongoDbAuthoritativeStore::new(&uri, TEST_DATABASE_NAME)
            .await
            .unwrap();
        let owner = Uuid::new_v4();
        let other_owner = Uuid::new_v4();

        let memo = Memo::new(
            "Version one".into(),
            "MongoDB integration content".into(),
            vec!["integration".into()],
            owner,
        );

        let create_intent = store.save_with_projection_intent(&memo).await.unwrap();
        assert_eq!(
            store.find_by_id(owner, memo.id).await.unwrap().unwrap().id,
            memo.id
        );
        assert!(store
            .find_by_id(other_owner, memo.id)
            .await
            .unwrap()
            .is_none());
        assert_eq!(store.list_projection_intents().await.unwrap().len(), 1);
        store
            .acknowledge_projection_intent(&create_intent)
            .await
            .unwrap();

        let mut winner = store.find_by_id(owner, memo.id).await.unwrap().unwrap();
        let mut stale = winner.clone();

        winner.update(Some("Winner".into()), None, None);
        let update_intent = store.save_with_projection_intent(&winner).await.unwrap();
        store
            .acknowledge_projection_intent(&update_intent)
            .await
            .unwrap();

        stale.update(Some("Stale writer".into()), None, None);
        assert!(matches!(
            store.save_with_projection_intent(&stale).await,
            Err(AppError::Conflict(_))
        ));
        assert!(store.list_projection_intents().await.unwrap().is_empty());

        let mut rollback_candidate = store.find_by_id(owner, winner.id).await.unwrap().unwrap();
        rollback_candidate.update(Some("Must roll back".into()), None, None);
        let duplicate_event = ProjectionIntent::new(
            owner,
            rollback_candidate.id,
            ProjectionTarget::Version(rollback_candidate.version),
        );
        store
            .projection_intents
            .insert_one(ProjectionIntentDocument::from_intent(&duplicate_event).unwrap())
            .await
            .unwrap();

        assert!(matches!(
            store
                .save_transaction(&rollback_candidate, &duplicate_event)
                .await,
            Err(AppError::DatabaseError(_))
        ));

        let after_aborted_transaction =
            store.find_by_id(owner, winner.id).await.unwrap().unwrap();
        assert_eq!(after_aborted_transaction.title, "Winner");
        assert_eq!(after_aborted_transaction.version, winner.version);
        store
            .acknowledge_projection_intent(&duplicate_event)
            .await
            .unwrap();
        assert!(store.list_projection_intents().await.unwrap().is_empty());

        let second = Memo::new(
            "Second memo".into(),
            "Hydration ordering".into(),
            vec![],
            owner,
        );
        let second_intent = store.save_with_projection_intent(&second).await.unwrap();
        store
            .acknowledge_projection_intent(&second_intent)
            .await
            .unwrap();

        let missing = Uuid::new_v4();
        let hydrated = store
            .find_many_by_ids(owner, &[second.id, winner.id, missing, second.id])
            .await
            .unwrap();
        assert_eq!(
            hydrated.iter().map(|memo| memo.id).collect::<Vec<_>>(),
            vec![second.id, winner.id, second.id]
        );

        let duplicate_delete_event =
            ProjectionIntent::new(owner, winner.id, ProjectionTarget::Deleted);
        store
            .projection_intents
            .insert_one(
                ProjectionIntentDocument::from_intent(&duplicate_delete_event).unwrap(),
            )
            .await
            .unwrap();

        assert!(matches!(
            store
                .delete_transaction(owner, winner.id, &duplicate_delete_event)
                .await,
            Err(AppError::DatabaseError(_))
        ));
        assert!(store.find_by_id(owner, winner.id).await.unwrap().is_some());
        store
            .acknowledge_projection_intent(&duplicate_delete_event)
            .await
            .unwrap();
        assert!(store.list_projection_intents().await.unwrap().is_empty());

        let delete_intent = store
            .delete_with_projection_intent(owner, winner.id)
            .await
            .unwrap();
        assert!(store.find_by_id(owner, winner.id).await.unwrap().is_none());
        assert_eq!(store.list_projection_intents().await.unwrap().len(), 1);
        store
            .acknowledge_projection_intent(&delete_intent)
            .await
            .unwrap();

        assert!(matches!(
            store.delete_with_projection_intent(owner, winner.id).await,
            Err(AppError::NotFound(_))
        ));
        assert!(store.list_projection_intents().await.unwrap().is_empty());

        cleanup_client
            .database(TEST_DATABASE_NAME)
            .drop()
            .await
            .unwrap();
    }

    #[test]
    fn projection_version_must_be_positive() {
        let event =
            ProjectionIntent::new(Uuid::new_v4(), Uuid::new_v4(), ProjectionTarget::Version(0));

        assert!(ProjectionIntentDocument::from_intent(&event).is_err());
    }
}
