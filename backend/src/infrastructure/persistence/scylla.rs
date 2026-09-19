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
type ProjectionRetryRow = (Uuid, Uuid, Uuid);

pub const PROJECTION_RETRY_BUCKETS: i32 = 16;

#[derive(Debug, Clone)]
pub struct ProjectionRetry {
    pub bucket: i32,
    pub event_id: Uuid,
    pub user_id: Uuid,
    pub memo_id: Uuid,
}

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
    enqueue_projection_retry: PreparedStatement,
    list_projection_retries: PreparedStatement,
    acknowledge_projection_retry: PreparedStatement,
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
                "CREATE TABLE IF NOT EXISTS memo_app.projection_retries (\
                    bucket int,\
                    event_id uuid,\
                    user_id uuid,\
                    memo_id uuid,\
                    PRIMARY KEY ((bucket), event_id)\
                )",
                &[],
            )
            .await
            .map_err(|error| {
                AppError::DatabaseError(format!(
                    "Failed to create projection_retries table: {error}"
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
        let enqueue_projection_retry = session
            .prepare(
                "INSERT INTO memo_app.projection_retries (bucket, event_id, user_id, memo_id) \
                 VALUES (?, ?, ?, ?)",
            )
            .await
            .map_err(|error| {
                AppError::DatabaseError(format!(
                    "Failed to prepare enqueue_projection_retry: {error}"
                ))
            })?;
        let list_projection_retries = session
            .prepare(
                "SELECT event_id, user_id, memo_id FROM memo_app.projection_retries \
                 WHERE bucket = ? LIMIT 32",
            )
            .await
            .map_err(|error| {
                AppError::DatabaseError(format!(
                    "Failed to prepare list_projection_retries: {error}"
                ))
            })?;
        let acknowledge_projection_retry = session
            .prepare(
                "DELETE FROM memo_app.projection_retries WHERE bucket = ? AND event_id = ?",
            )
            .await
            .map_err(|error| {
                AppError::DatabaseError(format!(
                    "Failed to prepare acknowledge_projection_retry: {error}"
                ))
            })?;

        Ok(PreparedStatements {
            find_by_id,
            find_all_by_user_id,
            save_memo,
            update_memo_if_version,
            delete_memo,
            exists,
            enqueue_projection_retry,
            list_projection_retries,
            acknowledge_projection_retry,
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

    pub async fn enqueue_projection_retry(
        &self,
        user_id: Uuid,
        memo_id: Uuid,
    ) -> AppResult<ProjectionRetry> {
        let event = ProjectionRetry {
            bucket: projection_retry_bucket(memo_id),
            event_id: Uuid::new_v4(),
            user_id,
            memo_id,
        };

        self.session
            .execute_unpaged(
                &self.prepared_statements.enqueue_projection_retry,
                (event.bucket, event.event_id, event.user_id, event.memo_id),
            )
            .await
            .map_err(|error| {
                AppError::DatabaseError(format!(
                    "Failed to enqueue projection reconciliation: {error}"
                ))
            })?;

        Ok(event)
    }

    pub async fn list_projection_retries(
        &self,
        bucket: i32,
    ) -> AppResult<Vec<ProjectionRetry>> {
        let result = self
            .session
            .execute_unpaged(&self.prepared_statements.list_projection_retries, (bucket,))
            .await
            .map_err(|error| {
                AppError::DatabaseError(format!(
                    "Failed to fetch projection reconciliation events: {error}"
                ))
            })?;
        let rows = result.into_rows_result().map_err(|error| {
            AppError::DatabaseError(format!(
                "Failed to read projection reconciliation events: {error}"
            ))
        })?;
        let typed_rows = rows.rows::<ProjectionRetryRow>().map_err(|error| {
            AppError::DatabaseError(format!(
                "Failed to type-check projection reconciliation events: {error}"
            ))
        })?;

        typed_rows
            .map(|row| {
                row.map(|(event_id, user_id, memo_id)| ProjectionRetry {
                    bucket,
                    event_id,
                    user_id,
                    memo_id,
                })
                .map_err(|error| {
                    AppError::DatabaseError(format!(
                        "Failed to deserialize projection reconciliation event: {error}"
                    ))
                })
            })
            .collect()
    }

    pub async fn acknowledge_projection_retry(&self, event: &ProjectionRetry) -> AppResult<()> {
        self.session
            .execute_unpaged(
                &self.prepared_statements.acknowledge_projection_retry,
                (event.bucket, event.event_id),
            )
            .await
            .map_err(|error| {
                AppError::DatabaseError(format!(
                    "Failed to acknowledge projection reconciliation event: {error}"
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

    pub fn projection_retry_bucket(memo_id: Uuid) -> i32 {
        projection_retry_bucket(memo_id)
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

fn projection_retry_bucket(memo_id: Uuid) -> i32 {
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
