//src/infrastructure/persistence/scylla.rs
use std::sync::Arc;
use scylla::{
    Session, SessionBuilder,
    prepared_statement::PreparedStatement,
    transport::errors::QueryError,
    frame::value::ValueList,
    QueryResult,
};
use tracing::error;
use chrono::{DateTime, Utc};
use crate::error::{AppError, AppResult};

/// ScyllaDBクライアントの実装
/// 
/// この実装は以下の特徴を持ちます：
/// - 非同期処理による高性能なクエリ実行
/// - プリペアドステートメントによるクエリ最適化
/// - コネクションプーリングによるリソース効率化
/// - 包括的なエラーハンドリング
pub struct ScyllaDB {
    session: Arc<Session>,
}

impl ScyllaDB {
    /// 新しいScyllaDBセッションを初期化
    ///
    /// # Arguments
    /// * `uri` - ScyllaDBクラスターのURI（例: "scylla://localhost:9042"）
    pub async fn new(uri: &str) -> AppResult<Self> {
        let session = SessionBuilder::new()
            .known_node(uri)
            .build()
            .await
            .map_err(|e| {
                error!("Failed to connect to ScyllaDB: {}", e);
                AppError::DatabaseError(format!("Failed to connect to database: {}", e))
            })?;

        // スキーマの初期化
        Self::initialize_schema(&session).await?;

        Ok(Self {
            session: Arc::new(session),
        })
    }

    /// データベーススキーマの初期化
    /// キースペース、テーブル、インデックスを作成
    async fn initialize_schema(session: &Session) -> AppResult<()> {
        // キースペースの作成
        let create_keyspace = r#"
            CREATE KEYSPACE IF NOT EXISTS memo_app
            WITH replication = {
                'class': 'SimpleStrategy',
                'replication_factor': 1
            };
        "#;

        session
            .query(create_keyspace, &[])
            .await
            .map_err(|e| AppError::DatabaseError(e.to_string()))?;

        // メモテーブルの作成
        let create_memo_table = r#"
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
            );
        "#;

        session
            .query(create_memo_table, &[])
            .await
            .map_err(|e| AppError::DatabaseError(e.to_string()))?;

        // ユーザーIDによるインデックスの作成
        let create_user_id_index = r#"
            CREATE INDEX IF NOT EXISTS memos_user_id_idx 
            ON memo_app.memos (user_id);
        "#;

        session
            .query(create_user_id_index, &[])
            .await
            .map_err(|e| AppError::DatabaseError(e.to_string()))?;

        Ok(())
    }

    pub fn get_session(&self) -> Arc<Session> {
        self.session.clone()
    }
}

/// プリペアドステートメントの管理構造体
#[derive(Clone)]
pub struct ScyllaDBPreparedStatements {
    pub insert_memo: Arc<PreparedStatement>,
    pub select_memo_by_id: Arc<PreparedStatement>,
    pub select_memos_by_user_id: Arc<PreparedStatement>,
    pub update_memo: Arc<PreparedStatement>,
    pub delete_memo: Arc<PreparedStatement>,
}

impl ScyllaDBPreparedStatements {
    /// プリペアドステートメントの準備
    /// 
    /// 全てのクエリをプリペアドステートメントとして事前に準備し、
    /// 実行時のパフォーマンスを最適化します。
    pub async fn prepare(session: Arc<Session>) -> Result<Self, QueryError> {
        let insert_memo = session.prepare(
            "INSERT INTO memo_app.memos (id, title, content, tags, user_id, created_at, updated_at, version) 
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
        ).await?;

        let select_memo_by_id = session.prepare(
            "SELECT * FROM memo_app.memos WHERE id = ?"
        ).await?;

        let select_memos_by_user_id = session.prepare(
            "SELECT * FROM memo_app.memos WHERE user_id = ?"
        ).await?;

        let update_memo = session.prepare(
            "UPDATE memo_app.memos 
             SET title = ?, content = ?, tags = ?, updated_at = ?, version = ? 
             WHERE id = ? IF version = ?"
        ).await?;

        let delete_memo = session.prepare(
            "DELETE FROM memo_app.memos WHERE id = ?"
        ).await?;

        Ok(Self {
            insert_memo: Arc::new(insert_memo),
            select_memo_by_id: Arc::new(select_memo_by_id),
            select_memos_by_user_id: Arc::new(select_memos_by_user_id),
            update_memo: Arc::new(update_memo),
            delete_memo: Arc::new(delete_memo),
        })
    }
}

/// DateTime<Utc>型をScyllaDBのタイムスタンプ形式に変換するトレイト
pub trait ToScyllaTimestamp {
    fn to_scylla_timestamp(&self) -> i64;
}

impl ToScyllaTimestamp for DateTime<Utc> {
    fn to_scylla_timestamp(&self) -> i64 {
        self.timestamp_millis()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio;

    #[tokio::test]
    async fn test_scylla_connection() {
        let scylla = ScyllaDB::new("scylla://localhost:9042").await;
        assert!(scylla.is_ok(), "Should connect to ScyllaDB successfully");
    }
}