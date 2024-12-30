use std::sync::Arc;
use uuid::Uuid;
use crate::{
    domain::memo::{entity::Memo, repository::MemoRepository},
    error::{AppError, AppResult},
};
use super::dto::{CreateMemoDto, UpdateMemoDto, MemoResponse, SearchResponse};

pub struct MemoService {
    memo_repository: Arc<dyn MemoRepository>,
}

impl MemoService {
    pub fn new(memo_repository: Arc<dyn MemoRepository>) -> Self {
        Self { memo_repository }
    }

    pub async fn create_memo(&self, dto: CreateMemoDto, user_id: Uuid) -> AppResult<MemoResponse> {
        let memo = Memo::new(dto.title, dto.content, dto.tags, user_id);
        self.memo_repository.save(&memo).await?;
        Ok(MemoResponse::from(memo))
    }

    pub async fn update_memo(
        &self,
        id: Uuid,
        dto: UpdateMemoDto,
        user_id: Uuid,
    ) -> AppResult<MemoResponse> {
        let mut memo = self
            .memo_repository
            .find_by_id(id)
            .await?
            .ok_or_else(|| AppError::NotFound("Memo not found".into()))?;

        if memo.user_id != user_id {
            return Err(AppError::Unauthorized("Not authorized to update this memo".into()));
        }

        if memo.version != dto.version {
            return Err(AppError::Conflict("Memo has been updated by another user".into()));
        }

        memo.update(dto.title, dto.content, dto.tags);
        self.memo_repository.save(&memo).await?;
        Ok(MemoResponse::from(memo))
    }

    pub async fn get_memo(&self, id: Uuid, user_id: Uuid) -> AppResult<MemoResponse> {
        let memo = self
            .memo_repository
            .find_by_id(id)
            .await?
            .ok_or_else(|| AppError::NotFound("Memo not found".into()))?;

        if memo.user_id != user_id {
            return Err(AppError::Unauthorized("Not authorized to view this memo".into()));
        }

        Ok(MemoResponse::from(memo))
    }

    pub async fn delete_memo(&self, id: Uuid, user_id: Uuid) -> AppResult<()> {
        let memo = self
            .memo_repository
            .find_by_id(id)
            .await?
            .ok_or_else(|| AppError::NotFound("Memo not found".into()))?;

        if memo.user_id != user_id {
            return Err(AppError::Unauthorized("Not authorized to delete this memo".into()));
        }

        self.memo_repository.delete(id).await
    }

    pub async fn get_user_memos(&self, user_id: Uuid) -> AppResult<Vec<MemoResponse>> {
        let memos = self.memo_repository.find_all_by_user_id(user_id).await?;
        Ok(memos.into_iter().map(MemoResponse::from).collect())
    }

    pub async fn search_memos(
        &self,
        query: &str,
        tag: Option<String>,
        user_id: Uuid,
    ) -> AppResult<SearchResponse> {
        let memos = self.memo_repository.search(query, tag, user_id).await?;
        let total = memos.len();
        
        Ok(SearchResponse {
            items: memos.into_iter().map(MemoResponse::from).collect(),
            total,
            page: 1,
            total_pages: 1,
        })
    }
}