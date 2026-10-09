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
    error::{AppError, AppResult},
    interfaces::auth::AuthenticatedUser,
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListParams {
    pub pagination: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<usize>,
}

impl ListParams {
    fn require_cursor_v1(&self) -> AppResult<()> {
        if self.pagination.as_deref() != Some("cursor-v1") {
            return Err(AppError::BadRequest(
                "Memo list pagination requires pagination=cursor-v1".into(),
            ));
        }

        Ok(())
    }
}

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
    authenticated_user: AuthenticatedUser,
    payload: Json<CreateMemoDto>,
) -> AppResult<HttpResponse> {
    let memo = service
        .create_memo(payload.into_inner(), authenticated_user.0.user_id)
        .await?;
    Ok(HttpResponse::Created().json(memo))
}

pub async fn update_memo(
    service: Data<MemoService>,
    authenticated_user: AuthenticatedUser,
    id: Path<Uuid>,
    payload: Json<UpdateMemoDto>,
) -> AppResult<HttpResponse> {
    let memo = service
        .update_memo(
            id.into_inner(),
            payload.into_inner(),
            authenticated_user.0.user_id,
        )
        .await?;
    Ok(HttpResponse::Ok().json(memo))
}

pub async fn get_memo(
    service: Data<MemoService>,
    authenticated_user: AuthenticatedUser,
    id: Path<Uuid>,
) -> AppResult<HttpResponse> {
    let memo = service
        .get_memo(id.into_inner(), authenticated_user.0.user_id)
        .await?;
    Ok(HttpResponse::Ok().json(memo))
}

pub async fn delete_memo(
    service: Data<MemoService>,
    authenticated_user: AuthenticatedUser,
    id: Path<Uuid>,
) -> AppResult<HttpResponse> {
    service
        .delete_memo(id.into_inner(), authenticated_user.0.user_id)
        .await?;
    Ok(HttpResponse::NoContent().finish())
}

pub async fn list_memos(
    service: Data<MemoService>,
    authenticated_user: AuthenticatedUser,
    query_params: Query<ListParams>,
) -> AppResult<HttpResponse> {
    query_params.require_cursor_v1()?;
    let page = service
        .get_user_memos_page(
            authenticated_user.0.user_id,
            query_params.cursor.as_deref(),
            query_params.limit,
        )
        .await?;
    Ok(HttpResponse::Ok().json(page))
}

pub async fn search_memos(
    service: Data<MemoService>,
    authenticated_user: AuthenticatedUser,
    query_params: Query<SearchParams>,
) -> AppResult<HttpResponse> {
    let result = service
        .search_memos(
            &query_params.query.clone().unwrap_or_default(),
            query_params.tag.clone(),
            authenticated_user.0.user_id,
            query_params.page,
            query_params.limit,
        )
        .await?;
    Ok(HttpResponse::Ok().json(result))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unversioned_memo_list_is_rejected() {
        let params = ListParams {
            pagination: None,
            cursor: None,
            limit: None,
        };

        let error = params.require_cursor_v1().unwrap_err();

        assert!(matches!(
            error,
            AppError::BadRequest(ref message)
                if message == "Memo list pagination requires pagination=cursor-v1"
        ));
    }
}
