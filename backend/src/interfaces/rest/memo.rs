use actix_web::{
    web::{Data, Json, Path, Query},
    HttpResponse,
};
use serde::Deserialize;
use uuid::Uuid;
use crate::{
    application::memo::{
        dto::{CreateMemoDto, UpdateMemoDto},
        service::MemoService,
    },
    error::AppResult,
};

#[derive(Debug, Deserialize)]
pub struct SearchParams {
    pub query: Option<String>,
    pub tag: Option<String>,
    #[serde(default = "default_page")]
    pub page: usize,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_page() -> usize {
    1
}

fn default_limit() -> usize {
    20
}

// メモ作成エンドポイント
pub async fn create_memo(
    service: Data<MemoService>,
    payload: Json<CreateMemoDto>,
) -> AppResult<HttpResponse> {
    let user_id = Uuid::parse_str("12345678-1234-1234-1234-123456789012").unwrap();
    let memo = service.create_memo(payload.into_inner(), user_id).await?;
    Ok(HttpResponse::Created().json(memo))
}

// メモ更新エンドポイント
pub async fn update_memo(
    service: Data<MemoService>,
    id: Path<Uuid>,
    payload: Json<UpdateMemoDto>,
) -> AppResult<HttpResponse> {
    let user_id = Uuid::parse_str("12345678-1234-1234-1234-123456789012").unwrap();
    let memo = service.update_memo(id.into_inner(), payload.into_inner(), user_id).await?;
    Ok(HttpResponse::Ok().json(memo))
}

// メモ取得エンドポイント
pub async fn get_memo(
    service: Data<MemoService>,
    id: Path<Uuid>,
) -> AppResult<HttpResponse> {
    let user_id = Uuid::parse_str("12345678-1234-1234-1234-123456789012").unwrap();
    let memo = service.get_memo(id.into_inner(), user_id).await?;
    Ok(HttpResponse::Ok().json(memo))
}

// メモ削除エンドポイント
pub async fn delete_memo(
    service: Data<MemoService>,
    id: Path<Uuid>,
) -> AppResult<HttpResponse> {
    let user_id = Uuid::parse_str("12345678-1234-1234-1234-123456789012").unwrap();
    service.delete_memo(id.into_inner(), user_id).await?;
    Ok(HttpResponse::NoContent().finish())
}

// ユーザーのメモ一覧取得エンドポイント
pub async fn list_memos(
    service: Data<MemoService>,
) -> AppResult<HttpResponse> {
    let user_id = Uuid::parse_str("12345678-1234-1234-1234-123456789012").unwrap();
    let memos = service.get_user_memos(user_id).await?;
    Ok(HttpResponse::Ok().json(memos))
}

// メモ検索エンドポイント
pub async fn search_memos(
    service: Data<MemoService>,
    query_params: Query<SearchParams>,
) -> AppResult<HttpResponse> {
    let user_id = Uuid::parse_str("12345678-1234-1234-1234-123456789012").unwrap();
    let result = service
        .search_memos(
            &query_params.query.clone().unwrap_or_default(),
            query_params.tag.clone(),
            user_id,
        )
        .await?;
    Ok(HttpResponse::Ok().json(result))
}

// ヘルスチェックエンドポイント
pub async fn health_check() -> HttpResponse {
    HttpResponse::Ok().json(serde_json::json!({
        "status": "ok",
        "timestamp": chrono::Utc::now().to_rfc3339()
    }))
}