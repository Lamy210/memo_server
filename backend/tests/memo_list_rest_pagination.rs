use std::sync::{Arc, Mutex};

use actix_web::{
    body::to_bytes,
    web::{Data, Query},
};
use async_trait::async_trait;
use memo_app_backend::{
    application::{
        maintenance::{
            UnrestrictedHighMemoAccessGuard, UnrestrictedHighSearchQueryGuard,
            UnrestrictedMemoMutationGuard,
        },
        memo::service::MemoService,
    },
    domain::memo::{
        entity::Memo,
        repository::{MemoListPage, MemoRepository, MemoSearchPage},
    },
    error::{AppError, AppResult},
    infrastructure::auth::AuthenticatedIdentity,
    interfaces::{
        auth::AuthenticatedUser,
        rest::memo::{list_memos, ListParams},
    },
};
use serde_json::Value;
use uuid::Uuid;

struct PagingRepository {
    items: Vec<Memo>,
    has_more: bool,
    calls: Mutex<Vec<(Option<Uuid>, usize)>>,
}

#[async_trait]
impl MemoRepository for PagingRepository {
    async fn find_by_id(&self, _user_id: Uuid, _id: Uuid) -> AppResult<Option<Memo>> {
        Ok(None)
    }

    async fn find_all_by_user_id(&self, _user_id: Uuid) -> AppResult<Vec<Memo>> {
        Err(AppError::ServiceUnavailable(
            "unbounded memo list read invoked".into(),
        ))
    }

    async fn list_page_by_user_id(
        &self,
        _user_id: Uuid,
        after: Option<Uuid>,
        limit: usize,
    ) -> AppResult<MemoListPage> {
        self.calls.lock().unwrap().push((after, limit));
        Ok(MemoListPage {
            items: self.items.clone(),
            has_more: self.has_more,
        })
    }

    async fn find_many_by_ids(&self, _user_id: Uuid, _ids: &[Uuid]) -> AppResult<Vec<Memo>> {
        Ok(Vec::new())
    }

    async fn save(&self, _memo: &Memo) -> AppResult<()> {
        Ok(())
    }

    async fn delete(&self, _user_id: Uuid, _id: Uuid) -> AppResult<()> {
        Ok(())
    }

    async fn search(
        &self,
        _query: &str,
        _tag: Option<String>,
        _user_id: Uuid,
        _page: usize,
        _limit: usize,
    ) -> AppResult<MemoSearchPage> {
        Ok(MemoSearchPage {
            items: Vec::new(),
            total: 0,
        })
    }

    async fn exists(&self, _user_id: Uuid, _id: Uuid) -> AppResult<bool> {
        Ok(false)
    }
}

fn memo(owner: Uuid, id: &str) -> Memo {
    let mut memo = Memo::new(id.into(), "content".into(), Vec::new(), owner);
    memo.id = Uuid::parse_str(id).unwrap();
    memo
}

fn service(repository: Arc<PagingRepository>) -> MemoService {
    MemoService::new(
        repository,
        None,
        Arc::new(UnrestrictedMemoMutationGuard),
        Arc::new(UnrestrictedHighMemoAccessGuard),
        Arc::new(UnrestrictedHighSearchQueryGuard),
        None,
        None,
    )
}

fn user(owner: Uuid) -> AuthenticatedUser {
    AuthenticatedUser(AuthenticatedIdentity { user_id: owner })
}

#[tokio::test]
async fn cursor_v1_rest_request_returns_versioned_shape() {
    let owner = Uuid::new_v4();
    let first = memo(owner, "550e8400-e29b-41d4-a716-446655440004");
    let second = memo(owner, "550e8400-e29b-41d4-a716-446655440003");
    let repository = Arc::new(PagingRepository {
        items: vec![first.clone(), second.clone()],
        has_more: true,
        calls: Mutex::new(Vec::new()),
    });

    let response = list_memos(
        Data::new(service(repository.clone())),
        user(owner),
        Query(ListParams {
            pagination: Some("cursor-v1".into()),
            cursor: None,
            limit: Some(2),
        }),
    )
    .await
    .unwrap();

    let body = to_bytes(response.into_body()).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["pagination"], "cursor-v1");
    assert_eq!(json["limit"], 2);
    assert_eq!(
        json["next_cursor"],
        "v1.550e8400-e29b-41d4-a716-446655440003"
    );
    assert_eq!(json["items"][0]["id"], first.id.to_string());
    assert_eq!(json["items"][1]["id"], second.id.to_string());
    assert_eq!(*repository.calls.lock().unwrap(), vec![(None, 2)]);
}

#[tokio::test]
async fn query_free_rest_request_requires_cursor_v1_without_repository_access() {
    let owner = Uuid::new_v4();
    let repository = Arc::new(PagingRepository {
        items: Vec::new(),
        has_more: false,
        calls: Mutex::new(Vec::new()),
    });

    let error = list_memos(
        Data::new(service(repository.clone())),
        user(owner),
        Query(ListParams {
            pagination: None,
            cursor: None,
            limit: None,
        }),
    )
    .await
    .unwrap_err();

    assert!(matches!(
        error,
        AppError::BadRequest(ref message)
            if message == "Memo list pagination requires pagination=cursor-v1"
    ));
    assert!(repository.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn pagination_fields_require_explicit_cursor_v1_mode() {
    let owner = Uuid::new_v4();
    let repository = Arc::new(PagingRepository {
        items: Vec::new(),
        has_more: false,
        calls: Mutex::new(Vec::new()),
    });

    for params in [
        ListParams {
            pagination: None,
            cursor: None,
            limit: Some(20),
        },
        ListParams {
            pagination: Some("cursor-v2".into()),
            cursor: None,
            limit: None,
        },
    ] {
        assert!(matches!(
            list_memos(
                Data::new(service(repository.clone())),
                user(owner),
                Query(params),
            )
            .await,
            Err(AppError::BadRequest(_))
        ));
    }

    assert!(repository.calls.lock().unwrap().is_empty());
}
