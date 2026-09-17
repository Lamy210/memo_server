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

pub async fn create_memo(
    service: Data<MemoService>,
    development_user_id: Data<Uuid>,
    payload: Json<CreateMemoDto>,
) -> AppResult<HttpResponse> {
    let memo = service
        .create_memo(payload.into_inner(), *development_user_id.get_ref())
        .await?;
    Ok(HttpResponse::Created().json(memo))
}

pub async fn update_memo(
    service: Data<MemoService>,
    development_user_id: Data<Uuid>,
    id: Path<Uuid>,
    payload: Json<UpdateMemoDto>,
) -> AppResult<HttpResponse> {
    let memo = service
        .update_memo(
            id.into_inner(),
            payload.into_inner(),
            *development_user_id.get_ref(),
        )
        .await?;
    Ok(HttpResponse::Ok().json(memo))
}

pub async fn get_memo(
    service: Data<MemoService>,
    development_user_id: Data<Uuid>,
    id: Path<Uuid>,
) -> AppResult<HttpResponse> {
    let memo = service
        .get_memo(id.into_inner(), *development_user_id.get_ref())
        .await?;
    Ok(HttpResponse::Ok().json(memo))
}

pub async fn delete_memo(
    service: Data<MemoService>,
    development_user_id: Data<Uuid>,
    id: Path<Uuid>,
) -> AppResult<HttpResponse> {
    service
        .delete_memo(id.into_inner(), *development_user_id.get_ref())
        .await?;
    Ok(HttpResponse::NoContent().finish())
}

pub async fn list_memos(
    service: Data<MemoService>,
    development_user_id: Data<Uuid>,
) -> AppResult<HttpResponse> {
    let memos = service
        .get_user_memos(*development_user_id.get_ref())
        .await?;
    Ok(HttpResponse::Ok().json(memos))
}

pub async fn search_memos(
    service: Data<MemoService>,
    development_user_id: Data<Uuid>,
    query_params: Query<SearchParams>,
) -> AppResult<HttpResponse> {
    let result = service
        .search_memos(
            &query_params.query.clone().unwrap_or_default(),
            query_params.tag.clone(),
            *development_user_id.get_ref(),
            query_params.page,
            query_params.limit,
        )
        .await?;
    Ok(HttpResponse::Ok().json(result))
}

pub async fn health_check() -> HttpResponse {
    HttpResponse::Ok().json(serde_json::json!({
        "status": "ok",
        "timestamp": chrono::Utc::now().to_rfc3339()
    }))
}
