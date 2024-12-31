// src/infrastructure/persistence/scylla.rs

use scylla::{
    Session, SessionBuilder,
    statement::batch::{Batch, BatchType},
    statement::prepared_statement::PreparedStatement,
    transport::errors::QueryError,
    transport::session::TypedRowIter,
    statement::Consistency,
    query::Query,
};
use std::sync::Arc;
use uuid::Uuid;
use chrono::{DateTime, Utc};
use crate::{
    domain::memo::entity::Memo,
    error::{AppError, AppResult},
};

/// ScyllaDBクライアントの実装
/// 
/// 非同期処理とバッチ処理を活用し、データの整合性を保証します。
pub struct ScyllaDB {
    session: Arc<Session>,
    prepared_statements: PreparedStatements,
}

/// プリペアドステートメントのコレクション
/// 
/// パフォーマンスとセキュリティを向上させるため、
/// 頻繁に使用されるクエリを事前にコンパイルします。
struct PreparedStatements {
    find_by_id: PreparedStatement,
    find_all_by_user_id: PreparedStatement,
    save_memo: PreparedStatement,
    delete_memo: PreparedStatement,
    exists: PreparedStatement,
}

impl ScyllaDB {
    pub async fn new(uri: &str) -> AppResult<Self> {
        let session = SessionBuilder::new()
            .known_node(uri)
            .build()
            .await
            .map_err(|e| AppError::DatabaseError(format!("Failed to connect to ScyllaDB: {}", e)))?;

        let session = Arc::new(session);
        
        // キースペースとテーブルの初期化
        Self::initialize_schema(&session).await?;

        // プリペアドステートメントの準備
        let prepared_statements = Self::prepare_statements(&session).await?;

        Ok(Self { 
            session,
            prepared_statements,
        })
    }

    /// プリペアドステートメントの初期化
    async fn prepare_statements(session: &Session) -> AppResult<PreparedStatements> {
        Ok(PreparedStatements {
            find_by_id: session.prepare("SELECT * FROM memo_app.memos WHERE id = ?").await
                .map_err(|e| AppError::DatabaseError(format!("Failed to prepare find_by_id: {}", e)))?,
            
            find_all_by_user_id: session.prepare("SELECT * FROM memo_app.memos WHERE user_id = ?").await
                .map_err(|e| AppError::DatabaseError(format!("Failed to prepare find_all_by_user_id: {}", e)))?,
            
            save_memo: session.prepare(
                "INSERT INTO memo_app.memos (id, title, content, tags, user_id, created_at, updated_at, version) 
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
            ).await
                .map_err(|e| AppError::DatabaseError(format!("Failed to prepare save_memo: {}", e)))?,
            
            delete_memo: session.prepare("DELETE FROM memo_app.memos WHERE id = ?").await
                .map_err(|e| AppError::DatabaseError(format!("Failed to prepare delete_memo: {}", e)))?,
            
            exists: session.prepare("SELECT id FROM memo_app.memos WHERE id = ?").await
                .map_err(|e| AppError::DatabaseError(format!("Failed to prepare exists: {}", e)))?,
        })
    }

    /// データベーススキーマの初期化
    async fn initialize_schema(session: &Session) -> AppResult<()> {
        // キースペース作成のバッチ
        let keyspace_batch = Batch::new(BatchType::Logged).add_statement(
            Query::new(
                "CREATE KEYSPACE IF NOT EXISTS memo_app 
                WITH replication = {'class': 'SimpleStrategy', 'replication_factor': 1}"
            )
        );

        session.batch(&keyspace_batch)
            .consistency(Consistency::All)
            .await
            .map_err(|e| AppError::DatabaseError(format!("Failed to create keyspace: {}", e)))?;

        // テーブル作成のバッチ
        let table_batch = Batch::new(BatchType::Logged).add_statement(
            Query::new(
                "CREATE TABLE IF NOT EXISTS memo_app.memos (
                    id uuid,
                    title text,
                    content text,
                    tags list<text>,
                    user_id uuid,
                    created_at timestamp,
                    updated_at timestamp,
                    version int,
                    PRIMARY KEY ((user_id), id)
                ) WITH CLUSTERING ORDER BY (id DESC)"
            )
        );

        session.batch(&table_batch)
            .consistency(Consistency::All)
            .await
            .map_err(|e| AppError::DatabaseError(format!("Failed to create table: {}", e)))?;

        Ok(())
    }

    /// IDによるメモの検索
    pub async fn find_by_id(&self, id: Uuid) -> AppResult<Option<Memo>> {
        let batch = Batch::new(BatchType::Logged)
            .add_statement(self.prepared_statements.find_by_id.bind((id,)));

        let result = self.session
            .batch(&batch)
            .consistency(Consistency::One)
            .await
            .map_err(|e| AppError::DatabaseError(format!("Failed to fetch memo: {}", e)))?;

        if let Some(rows) = result.rows {
            if let Some(row) = rows.into_typed::<(
                Uuid, String, String, Vec<String>, Uuid, DateTime<Utc>, DateTime<Utc>, i32
            )>().next() {
                let (id, title, content, tags, user_id, created_at, updated_at, version) = row
                    .map_err(|e| AppError::DatabaseError(format!("Failed to parse row: {}", e)))?;

                Ok(Some(Memo {
                    id,
                    title,
                    content,
                    tags,
                    user_id,
                    created_at,
                    updated_at,
                    version,
                }))
            } else {
                Ok(None)
            }
        } else {
            Ok(None)
        }
    }

    /// ユーザーIDによるメモの一覧取得
    pub async fn find_all_by_user_id(&self, user_id: Uuid) -> AppResult<Vec<Memo>> {
        let batch = Batch::new(BatchType::Logged)
            .add_statement(self.prepared_statements.find_all_by_user_id.bind((user_id,)));

        let result = self.session
            .batch(&batch)
            .consistency(Consistency::One)
            .await
            .map_err(|e| AppError::DatabaseError(format!("Failed to fetch memos: {}", e)))?;

        let mut memos = Vec::new();

        if let Some(rows) = result.rows {
            for row in rows.into_typed::<(
                Uuid, String, String, Vec<String>, Uuid, DateTime<Utc>, DateTime<Utc>, i32
            )>() {
                let (id, title, content, tags, user_id, created_at, updated_at, version) = row
                    .map_err(|e| AppError::DatabaseError(format!("Failed to parse row: {}", e)))?;

                memos.push(Memo {
                    id,
                    title,
                    content,
                    tags,
                    user_id,
                    created_at,
                    updated_at,
                    version,
                });
            }
        }

        Ok(memos)
    }

    /// メモの保存
    pub async fn save(&self, memo: &Memo) -> AppResult<()> {
        let batch = Batch::new(BatchType::Logged)
            .add_statement(self.prepared_statements.save_memo.bind((
                memo.id,
                &memo.title,
                &memo.content,
                &memo.tags,
                memo.user_id,
                memo.created_at,
                memo.updated_at,
                memo.version,
            )));

        match self.session
            .batch(&batch)
            .consistency(Consistency::Quorum)
            .await
        {
            Ok(_) => Ok(()),
            Err(e) => match e {
                QueryError::DbError(ref dbe) if dbe.message.contains("already exists") => {
                    Err(AppError::Conflict("Memo already exists".into()))
                }
                e => Err(AppError::DatabaseError(format!("Failed to save memo: {}", e))),
            }
        }
    }

    /// メモの削除
    pub async fn delete(&self, id: Uuid) -> AppResult<()> {
        let batch = Batch::new(BatchType::Logged)
            .add_statement(self.prepared_statements.delete_memo.bind((id,)));

        self.session
            .batch(&batch)
            .consistency(Consistency::Quorum)
            .await
            .map_err(|e| AppError::DatabaseError(format!("Failed to delete memo: {}", e)))?;

        Ok(())
    }

    /// メモの存在確認
    pub async fn exists(&self, id: Uuid) -> AppResult<bool> {
        let batch = Batch::new(BatchType::Logged)
            .add_statement(self.prepared_statements.exists.bind((id,)));

        let result = self.session
            .batch(&batch)
            .consistency(Consistency::One)
            .await
            .map_err(|e| AppError::DatabaseError(format!("Failed to check memo existence: {}", e)))?;

        Ok(result.rows.map_or(false, |rows| !rows.is_empty()))
    }

    /// ヘルスチェック
    pub async fn health_check(&self) -> AppResult<bool> {
        let batch = Batch::new(BatchType::Logged)
            .add_statement(Query::new("SELECT release_version FROM system.local"));

        let result = self.session
            .batch(&batch)
            .consistency(Consistency::One)
            .await
            .map_err(|e| AppError::DatabaseError(format!("Health check failed: {}", e)))?;

        Ok(result.rows.map_or(false, |rows| !rows.is_empty()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[tokio::test]
    async fn test_save_and_find_memo() {
        let scylla = ScyllaDB::new("scylla://localhost:9042").await.unwrap();
        
        let memo = Memo {
            id: Uuid::new_v4(),
            title: "Test Memo".to_string(),
            content: "Test Content".to_string(),
            tags: vec!["test".to_string()],
            user_id: Uuid::new_v4(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            version: 1,
        };

        scylla.save(&memo).await.unwrap();

        let found = scylla.find_by_id(memo.id).await.unwrap().unwrap();
        assert_eq!(found.id, memo.id);
        assert_eq!(found.title, memo.title);
        assert_eq!(found.content, memo.content);
        assert_eq!(found.tags, memo.tags);
        assert_eq!(found.user_id, memo.user_id);
        assert_eq!(found.version, memo.version);

        scylla.delete(memo.id).await.unwrap();
    }
}