use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use scylla::{
    frame::response::result::{CqlValue, Row},
    statement::prepared_statement::PreparedStatement,
    Session, SessionBuilder,
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

        Ok(PreparedStatements {
            find_by_id,
            find_all_by_user_id,
            save_memo,
            update_memo_if_version,
            delete_memo,
            exists,
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
        let row = rows.first_row::<Row>().map_err(|error| {
            AppError::DatabaseError(format!(
                "Failed to deserialize conditional update result: {error}"
            ))
        })?;
        let applied = matches!(row.columns.first(), Some(Some(CqlValue::Boolean(true))));

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
