use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use scylla::{
    client::{session::Session, session_builder::SessionBuilder},
    statement::prepared::PreparedStatement,
};
use uuid::Uuid;

use crate::{
    application::health::HealthProbe,
    domain::memo::entity::Memo,
    error::{AppError, AppResult},
};

use super::ports::{MemoAuthoritativeStore, ProjectionIntent, PROJECTION_DELETE_TARGET};

type MemoRow = (
    Uuid,
    String,
    String,
    Vec<String>,
    Uuid,
    DateTime<Utc>,
    DateTime<Utc>,
    i32,
);
type ProjectionIntentRow = (Uuid, Uuid, Uuid, i32);

const PROJECTION_RETRY_BUCKETS: i32 = 16;

pub struct ScyllaDB {
    session: Arc<Session>,
    prepared_statements: PreparedStatements,
}

struct PreparedStatements {
    find_by_id: PreparedStatement,
    find_all_by_user_id: PreparedStatement,
    save_memo: PreparedStatement,
    update_memo_if_version: PreparedStatement,
    delete_memo: PreparedStatement,
    exists: PreparedStatement,
    enqueue_projection_intent: PreparedStatement,
    list_projection_intents: PreparedStatement,
    acknowledge_projection_intent: PreparedStatement,
}

impl ScyllaDB {
    pub async fn new(uri: &str) -> AppResult<Self> {
        let session = SessionBuilder::new()
            .known_node(uri)
            .build()
            .await
            .map_err(|error| {
                AppError::DatabaseError(format!("Failed to connect to ScyllaDB: {error}"))
            })?;
        let session = Arc::new(session);

        Self::initialize_schema(&session).await?;
        let prepared_statements = Self::prepare_statements(&session).await?;

        Ok(Self {
            session,
            prepared_statements,
        })
    }

    async fn initialize_schema(session: &Session) -> AppResult<()> {
        session
            .query_unpaged(
                "CREATE KEYSPACE IF NOT EXISTS memo_app \
                 WITH replication = {'class': 'NetworkTopologyStrategy', 'replication_factor': 1}",
                &[],
            )
            .await
            .map_err(|error| {
                AppError::DatabaseError(format!("Failed to create keyspace: {error}"))
            })?;

        session
            .query_unpaged(
                "CREATE TABLE IF NOT EXISTS memo_app.memos (\
                    id uuid,\
                    title text,\
                    content text,\
                    tags list<text>,\
                    user_id uuid,\
                    created_at timestamp,\
                    updated_at timestamp,\
                    version int,\
                    PRIMARY KEY ((user_id), id)\
                ) WITH CLUSTERING ORDER BY (id DESC)",
                &[],
            )
            .await
            .map_err(|error| {
                AppError::DatabaseError(format!("Failed to create memos table: {error}"))
            })?;

        session
            .query_unpaged(
                "CREATE TABLE IF NOT EXISTS memo_app.projection_intents (\
                    bucket int,\
                    event_id uuid,\
                    user_id uuid,\
                    memo_id uuid,\
                    target_version int,\
                    PRIMARY KEY ((bucket), event_id)\
                )",
                &[],
            )
            .await
            .map_err(|error| {
                AppError::DatabaseError(format!(
                    "Failed to create projection_intents table: {error}"
                ))
            })?;

        Ok(())
    }

    async fn prepare_statements(session: &Session) -> AppResult<PreparedStatements> {
        let find_by_id = session
            .prepare(
                "SELECT id, title, content, tags, user_id, created_at, updated_at, version \
                 FROM memo_app.memos WHERE user_id = ? AND id = ?",
            )
            .await
            .map_err(|error| {
                AppError::DatabaseError(format!("Failed to prepare find_by_id: {error}"))
            })?;
        let find_all_by_user_id = session
            .prepare(
                "SELECT id, title, content, tags, user_id, created_at, updated_at, version \
                 FROM memo_app.memos WHERE user_id = ?",
            )
            .await
            .map_err(|error| {
                AppError::DatabaseError(format!("Failed to prepare find_all_by_user_id: {error}"))
            })?;
        let save_memo = session
            .prepare(
                "INSERT INTO memo_app.memos \
                 (id, title, content, tags, user_id, created_at, updated_at, version) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .await
            .map_err(|error| {
                AppError::DatabaseError(format!("Failed to prepare save_memo: {error}"))
            })?;
        let update_memo_if_version = session
            .prepare(
                "UPDATE memo_app.memos SET title = ?, content = ?, tags = ?, updated_at = ?, version = ? \
                 WHERE user_id = ? AND id = ? IF version = ?",
            )
            .await
            .map_err(|error| {
                AppError::DatabaseError(format!(
                    "Failed to prepare update_memo_if_version: {error}"
                ))
            })?;
        let delete_memo = session
            .prepare("DELETE FROM memo_app.memos WHERE user_id = ? AND id = ?")
            .await
            .map_err(|error| {
                AppError::DatabaseError(format!("Failed to prepare delete_memo: {error}"))
            })?;
        let exists = session
            .prepare("SELECT id FROM memo_app.memos WHERE user_id = ? AND id = ?")
            .await
            .map_err(|error| {
                AppError::DatabaseError(format!("Failed to prepare exists: {error}"))
            })?;
        let enqueue_projection_intent = session
            .prepare(
                "INSERT INTO memo_app.projection_intents \
                 (bucket, event_id, user_id, memo_id, target_version) VALUES (?, ?, ?, ?, ?)",
            )
            .await
            .map_err(|error| {
                AppError::DatabaseError(format!(
                    "Failed to prepare enqueue_projection_intent: {error}"
                ))
            })?;
        let list_projection_intents = session
            .prepare(
                "SELECT event_id, user_id, memo_id, target_version FROM memo_app.projection_intents \
                 WHERE bucket = ?",
            )
            .await
            .map_err(|error| {
                AppError::DatabaseError(format!(
                    "Failed to prepare list_projection_intents: {error}"
                ))
            })?;
        let acknowledge_projection_intent = session
            .prepare("DELETE FROM memo_app.projection_intents WHERE bucket = ? AND event_id = ?")
            .await
            .map_err(|error| {
                AppError::DatabaseError(format!(
                    "Failed to prepare acknowledge_projection_intent: {error}"
                ))
            })?;

        Ok(PreparedStatements {
            find_by_id,
            find_all_by_user_id,
            save_memo,
            update_memo_if_version,
            delete_memo,
            exists,
            enqueue_projection_intent,
            list_projection_intents,
            acknowledge_projection_intent,
        })
    }

    pub async fn find_by_id(&self, user_id: Uuid, id: Uuid) -> AppResult<Option<Memo>> {
        let result = self
            .session
            .execute_unpaged(&self.prepared_statements.find_by_id, (user_id, id))
            .await
            .map_err(|error| AppError::DatabaseError(format!("Failed to fetch memo: {error}")))?;
        let rows = result.into_rows_result().map_err(|error| {
            AppError::DatabaseError(format!("Failed to read memo result: {error}"))
        })?;
        let row = rows.maybe_first_row::<MemoRow>().map_err(|error| {
            AppError::DatabaseError(format!("Failed to deserialize memo: {error}"))
        })?;

        Ok(row.map(Self::memo_from_row))
    }

    pub async fn find_all_by_user_id(&self, user_id: Uuid) -> AppResult<Vec<Memo>> {
        let result = self
            .session
            .execute_unpaged(&self.prepared_statements.find_all_by_user_id, (user_id,))
            .await
            .map_err(|error| AppError::DatabaseError(format!("Failed to fetch memos: {error}")))?;
        let rows = result.into_rows_result().map_err(|error| {
            AppError::DatabaseError(format!("Failed to read memos result: {error}"))
        })?;
        let typed_rows = rows.rows::<MemoRow>().map_err(|error| {
            AppError::DatabaseError(format!("Failed to type-check memo rows: {error}"))
        })?;

        typed_rows
            .map(|row| {
                row.map(Self::memo_from_row).map_err(|error| {
                    AppError::DatabaseError(format!("Failed to deserialize memo: {error}"))
                })
            })
            .collect()
    }

    pub async fn save(&self, memo: &Memo) -> AppResult<()> {
        if memo.version == 1 {
            self.session
                .execute_unpaged(
                    &self.prepared_statements.save_memo,
                    (
                        memo.id,
                        memo.title.as_str(),
                        memo.content.as_str(),
                        &memo.tags,
                        memo.user_id,
                        memo.created_at,
                        memo.updated_at,
                        memo.version,
                    ),
                )
                .await
                .map_err(|error| {
                    AppError::DatabaseError(format!("Failed to create memo: {error}"))
                })?;
            return Ok(());
        }

        let expected_version = memo.version - 1;
        let result = self
            .session
            .execute_unpaged(
                &self.prepared_statements.update_memo_if_version,
                (
                    memo.title.as_str(),
                    memo.content.as_str(),
                    &memo.tags,
                    memo.updated_at,
                    memo.version,
                    memo.user_id,
                    memo.id,
                    expected_version,
                ),
            )
            .await
            .map_err(|error| AppError::DatabaseError(format!("Failed to update memo: {error}")))?;
        let rows = result.into_rows_result().map_err(|error| {
            AppError::DatabaseError(format!("Failed to read conditional update result: {error}"))
        })?;
        let (applied, _current_version) =
            rows.first_row::<(bool, Option<i32>)>().map_err(|error| {
                AppError::DatabaseError(format!(
                    "Failed to deserialize conditional update result: {error}"
                ))
            })?;

        if !applied {
            return Err(AppError::Conflict(
                "Memo has been updated by another client".into(),
            ));
        }

        Ok(())
    }

    pub async fn delete(&self, user_id: Uuid, id: Uuid) -> AppResult<()> {
        self.session
            .execute_unpaged(&self.prepared_statements.delete_memo, (user_id, id))
            .await
            .map_err(|error| AppError::DatabaseError(format!("Failed to delete memo: {error}")))?;
        Ok(())
    }

    pub async fn exists(&self, user_id: Uuid, id: Uuid) -> AppResult<bool> {
        let result = self
            .session
            .execute_unpaged(&self.prepared_statements.exists, (user_id, id))
            .await
            .map_err(|error| {
                AppError::DatabaseError(format!("Failed to check memo existence: {error}"))
            })?;
        let rows = result.into_rows_result().map_err(|error| {
            AppError::DatabaseError(format!("Failed to read existence result: {error}"))
        })?;
        let row = rows.maybe_first_row::<(Uuid,)>().map_err(|error| {
            AppError::DatabaseError(format!("Failed to deserialize existence result: {error}"))
        })?;
        Ok(row.is_some())
    }

    pub async fn enqueue_projection_intent(
        &self,
        user_id: Uuid,
        memo_id: Uuid,
        target_version: i32,
    ) -> AppResult<ProjectionIntent> {
        let event = ProjectionIntent::new(user_id, memo_id, target_version);
        let bucket = projection_bucket(memo_id);

        self.session
            .execute_unpaged(
                &self.prepared_statements.enqueue_projection_intent,
                (
                    bucket,
                    event.event_id,
                    event.user_id,
                    event.memo_id,
                    event.target_version,
                ),
            )
            .await
            .map_err(|error| {
                AppError::DatabaseError(format!(
                    "Failed to persist projection reconciliation intent: {error}"
                ))
            })?;

        Ok(event)
    }

    async fn list_projection_intents_in_bucket(
        &self,
        bucket: i32,
    ) -> AppResult<Vec<ProjectionIntent>> {
        let result = self
            .session
            .execute_unpaged(&self.prepared_statements.list_projection_intents, (bucket,))
            .await
            .map_err(|error| {
                AppError::DatabaseError(format!(
                    "Failed to fetch projection reconciliation intents: {error}"
                ))
            })?;
        let rows = result.into_rows_result().map_err(|error| {
            AppError::DatabaseError(format!(
                "Failed to read projection reconciliation intents: {error}"
            ))
        })?;
        let typed_rows = rows.rows::<ProjectionIntentRow>().map_err(|error| {
            AppError::DatabaseError(format!(
                "Failed to type-check projection reconciliation intents: {error}"
            ))
        })?;

        typed_rows
            .map(|row| {
                row.map(
                    |(event_id, user_id, memo_id, target_version)| ProjectionIntent {
                        event_id,
                        user_id,
                        memo_id,
                        target_version,
                    },
                )
                .map_err(|error| {
                    AppError::DatabaseError(format!(
                        "Failed to deserialize projection reconciliation intent: {error}"
                    ))
                })
            })
            .collect()
    }

    pub async fn acknowledge_projection_intent(&self, event: &ProjectionIntent) -> AppResult<()> {
        let bucket = projection_bucket(event.memo_id);
        self.session
            .execute_unpaged(
                &self.prepared_statements.acknowledge_projection_intent,
                (bucket, event.event_id),
            )
            .await
            .map_err(|error| {
                AppError::DatabaseError(format!(
                    "Failed to acknowledge projection reconciliation intent: {error}"
                ))
            })?;
        Ok(())
    }

    pub async fn health_check(&self) -> AppResult<bool> {
        let result = self
            .session
            .query_unpaged("SELECT release_version FROM system.local", &[])
            .await
            .map_err(|error| AppError::DatabaseError(format!("Health check failed: {error}")))?;
        let rows = result.into_rows_result().map_err(|error| {
            AppError::DatabaseError(format!("Failed to read health check result: {error}"))
        })?;
        Ok(rows.rows_num() > 0)
    }

    fn memo_from_row(row: MemoRow) -> Memo {
        let (id, title, content, tags, user_id, created_at, updated_at, version) = row;
        Memo {
            id,
            title,
            content,
            tags,
            user_id,
            created_at,
            updated_at,
            version,
        }
    }
}

#[async_trait]
impl MemoAuthoritativeStore for ScyllaDB {
    async fn find_by_id(&self, user_id: Uuid, id: Uuid) -> AppResult<Option<Memo>> {
        ScyllaDB::find_by_id(self, user_id, id).await
    }

    async fn find_all_by_user_id(&self, user_id: Uuid) -> AppResult<Vec<Memo>> {
        ScyllaDB::find_all_by_user_id(self, user_id).await
    }

    async fn save_with_projection_intent(&self, memo: &Memo) -> AppResult<ProjectionIntent> {
        let event =
            ScyllaDB::enqueue_projection_intent(self, memo.user_id, memo.id, memo.version).await?;

        ScyllaDB::save(self, memo).await?;

        Ok(event)
    }

    async fn delete_with_projection_intent(
        &self,
        user_id: Uuid,
        id: Uuid,
    ) -> AppResult<ProjectionIntent> {
        let event =
            ScyllaDB::enqueue_projection_intent(self, user_id, id, PROJECTION_DELETE_TARGET).await?;

        ScyllaDB::delete(self, user_id, id).await?;

        Ok(event)
    }

    async fn exists(&self, user_id: Uuid, id: Uuid) -> AppResult<bool> {
        ScyllaDB::exists(self, user_id, id).await
    }

    async fn enqueue_projection_intent(
        &self,
        user_id: Uuid,
        memo_id: Uuid,
        target_version: i32,
    ) -> AppResult<ProjectionIntent> {
        ScyllaDB::enqueue_projection_intent(self, user_id, memo_id, target_version).await
    }

    async fn list_projection_intents(&self) -> AppResult<Vec<ProjectionIntent>> {
        let mut intents = Vec::new();
        for bucket in 0..PROJECTION_RETRY_BUCKETS {
            intents.extend(self.list_projection_intents_in_bucket(bucket).await?);
        }
        Ok(intents)
    }

    async fn acknowledge_projection_intent(&self, event: &ProjectionIntent) -> AppResult<()> {
        ScyllaDB::acknowledge_projection_intent(self, event).await
    }
}

fn projection_bucket(memo_id: Uuid) -> i32 {
    (memo_id.as_u128() % PROJECTION_RETRY_BUCKETS as u128) as i32
}

#[async_trait]
impl HealthProbe for ScyllaDB {
    async fn check(&self) -> bool {
        match self.health_check().await {
            Ok(healthy) => healthy,
            Err(error) => {
                log::warn!("Scylla health check failed: {error}");
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_bucket_is_stable_and_bounded() {
        let memo_id = Uuid::new_v4();

        let first = projection_bucket(memo_id);
        let second = projection_bucket(memo_id);

        assert_eq!(first, second);
        assert!((0..PROJECTION_RETRY_BUCKETS).contains(&first));
    }

    #[tokio::test]
    #[ignore = "requires a local ScyllaDB instance"]
    async fn save_find_and_delete_round_trip() {
        let scylla = ScyllaDB::new("127.0.0.1:9042").await.unwrap();
        let user_id = Uuid::new_v4();
        let memo = Memo::new(
            "Test Memo".to_string(),
            "Test Content".to_string(),
            vec!["test".to_string()],
            user_id,
        );

        scylla.save(&memo).await.unwrap();
        let found = scylla.find_by_id(user_id, memo.id).await.unwrap().unwrap();
        assert_eq!(found.id, memo.id);
        assert_eq!(found.user_id, user_id);

        scylla.delete(user_id, memo.id).await.unwrap();
        assert!(!scylla.exists(user_id, memo.id).await.unwrap());
    }
}
