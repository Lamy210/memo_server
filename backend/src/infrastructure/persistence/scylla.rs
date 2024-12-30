// src/infrastructure/persistence/scylla.rs

use std::sync::Arc;
use scylla::{
    Session, SessionBuilder,
    prepared_statement::PreparedStatement,
    statement::{Consistency, SerialConsistency},
    batch::{Batch, BatchType},
    frame::value::SerializeRow,
    deserialize::row::{RowIterator, IntoTypedRows},
    query::Query,
};
use tokio::sync::RwLock;
use tracing::{debug, error, info};
use crate::error::{AppError, AppResult};

#[derive(Clone)]
pub struct ScyllaDB {
    session: Arc<Session>,
    prepared_statements: Arc<RwLock<Option<PreparedStatements>>>,
}

#[derive(Clone)]
pub struct PreparedStatements {
    pub insert_memo: PreparedStatement,
    pub select_memo_by_id: PreparedStatement,
    pub select_memos_by_user_id: PreparedStatement,
    pub update_memo: PreparedStatement,
    pub delete_memo: PreparedStatement,
    pub select_memo_exists: PreparedStatement,
}

impl ScyllaDB {
    pub async fn new(uri: &str) -> AppResult<Self> {
        let session = SessionBuilder::new()
            .known_node(uri)
            .build()
            .await
            .map_err(|e| {
                error!("Failed to connect to ScyllaDB: {}", e);
                AppError::DatabaseError(format!("Database connection failed: {}", e))
            })?;

        info!("Successfully connected to ScyllaDB");

        let db = Self {
            session: Arc::new(session),
            prepared_statements: Arc::new(RwLock::new(None)),
        };

        db.initialize_schema().await?;
        db.prepare_statements().await?;

        Ok(db)
    }

    async fn initialize_schema(&self) -> AppResult<()> {
        debug!("Initializing database schema");

        let create_keyspace = Query::new(
            "CREATE KEYSPACE IF NOT EXISTS memo_app WITH replication = {'class': 'SimpleStrategy', 'replication_factor': 1} AND durable_writes = true"
        );

        let create_memo_table = Query::new(
            r#"
            CREATE TABLE IF NOT EXISTS memo_app.memos (
                id uuid,
                title text,
                content text,
                tags set<text>,
                user_id uuid,
                created_at timestamp,
                updated_at timestamp,
                version int,
                PRIMARY KEY (id)
            )
            "#
        );

        self.session
            .query(create_keyspace, &[])
            .await
            .map_err(|e| AppError::DatabaseError(format!("Failed to create keyspace: {}", e)))?;

        self.session
            .query(create_memo_table, &[])
            .await
            .map_err(|e| AppError::DatabaseError(format!("Failed to create memos table: {}", e)))?;

        let indices = [
            "CREATE INDEX IF NOT EXISTS memos_user_id_idx ON memo_app.memos (user_id)",
            "CREATE INDEX IF NOT EXISTS memos_tags_idx ON memo_app.memos (tags)",
        ];

        for create_index in indices.iter() {
            let query = Query::new(create_index);
            self.session
                .query(query, &[])
                .await
                .map_err(|e| AppError::DatabaseError(format!("Failed to create index: {}", e)))?;
        }

        debug!("Schema initialization completed");
        Ok(())
    }

    async fn prepare_statements(&self) -> AppResult<()> {
        let statements = PreparedStatements {
            insert_memo: self.session
                .prepare("INSERT INTO memo_app.memos (id, title, content, tags, user_id, created_at, updated_at, version) VALUES (?, ?, ?, ?, ?, ?, ?, ?)")
                .await
                .map_err(|e| AppError::DatabaseError(e.to_string()))?,

            select_memo_by_id: self.session
                .prepare("SELECT * FROM memo_app.memos WHERE id = ?")
                .await
                .map_err(|e| AppError::DatabaseError(e.to_string()))?,

            select_memos_by_user_id: self.session
                .prepare("SELECT * FROM memo_app.memos WHERE user_id = ?")
                .await
                .map_err(|e| AppError::DatabaseError(e.to_string()))?,

            update_memo: self.session
                .prepare("UPDATE memo_app.memos SET title = ?, content = ?, tags = ?, updated_at = ?, version = ? WHERE id = ? IF version = ?")
                .await
                .map_err(|e| AppError::DatabaseError(e.to_string()))?,

            delete_memo: self.session
                .prepare("DELETE FROM memo_app.memos WHERE id = ?")
                .await
                .map_err(|e| AppError::DatabaseError(e.to_string()))?,

            select_memo_exists: self.session
                .prepare("SELECT 1 FROM memo_app.memos WHERE id = ? LIMIT 1")
                .await
                .map_err(|e| AppError::DatabaseError(e.to_string()))?,
        };

        let mut prepared_statements = self.prepared_statements.write().await;
        *prepared_statements = Some(statements);

        debug!("Statement preparation completed");
        Ok(())
    }

    pub async fn get_prepared_statements(&self) -> AppResult<PreparedStatements> {
        let statements = self.prepared_statements.read().await;
        match &*statements {
            Some(statements) => Ok(statements.clone()),
            None => {
                drop(statements);
                self.prepare_statements().await?;
                Ok(self.prepared_statements.read().await.as_ref().unwrap().clone())
            }
        }
    }

    pub async fn execute<V>(&self, query: &str, values: V) -> AppResult<scylla::QueryResult>
    where
        V: SerializeRow + Send,
    {
        let query = Query::new(query);
        self.session
            .query(query, values)
            .await
            .map_err(|e| AppError::DatabaseError(e.to_string()))
    }

    pub async fn execute_prepared<V>(&self, statement: &PreparedStatement, values: V) -> AppResult<scylla::QueryResult>
    where
        V: SerializeRow + Send,
    {
        self.session
            .execute(statement, values)
            .await
            .map_err(|e| AppError::DatabaseError(e.to_string()))
    }

    pub async fn batch(&self) -> Batch {
        let mut batch = Batch::default();
        batch.set_consistency(Consistency::LocalQuorum);
        batch
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[tokio::test]
    async fn test_database_connection() {
        let db = ScyllaDB::new("scylla://localhost:9042").await;
        assert!(db.is_ok(), "Should connect to database successfully");
    }

    #[tokio::test]
    async fn test_prepared_statements() {
        let db = ScyllaDB::new("scylla://localhost:9042").await.unwrap();
        let statements = db.get_prepared_statements().await;
        assert!(statements.is_ok(), "Should prepare statements successfully");
    }
}