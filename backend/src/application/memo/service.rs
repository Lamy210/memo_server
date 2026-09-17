use std::sync::Arc;

use uuid::Uuid;

use super::dto::{CreateMemoDto, MemoResponse, SearchResponse, UpdateMemoDto};
use crate::{
    domain::memo::{entity::Memo, repository::MemoRepository},
    error::{AppError, AppResult},
};

pub struct MemoService {
    memo_repository: Arc<dyn MemoRepository>,
}

impl MemoService {
    pub fn new(memo_repository: Arc<dyn MemoRepository>) -> Self {
        Self { memo_repository }
    }

    pub async fn create_memo(&self, dto: CreateMemoDto, user_id: Uuid) -> AppResult<MemoResponse> {
        let memo = Memo::new(dto.title, dto.content, dto.tags, user_id);
        Self::validate_memo(&memo)?;
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
            .find_by_id(user_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("Memo not found".into()))?;

        if memo.user_id != user_id {
            return Err(AppError::Unauthorized(
                "Not authorized to update this memo".into(),
            ));
        }
        if memo.version != dto.version {
            return Err(AppError::Conflict(
                "Memo has been updated by another client".into(),
            ));
        }

        memo.update(dto.title, dto.content, dto.tags);
        Self::validate_memo(&memo)?;
        self.memo_repository.save(&memo).await?;
        Ok(MemoResponse::from(memo))
    }

    pub async fn get_memo(&self, id: Uuid, user_id: Uuid) -> AppResult<MemoResponse> {
        let memo = self
            .memo_repository
            .find_by_id(user_id, id)
            .await?
            .ok_or_else(|| AppError::NotFound("Memo not found".into()))?;
        Ok(MemoResponse::from(memo))
    }

    pub async fn delete_memo(&self, id: Uuid, user_id: Uuid) -> AppResult<()> {
        if !self.memo_repository.exists(user_id, id).await? {
            return Err(AppError::NotFound("Memo not found".into()));
        }
        self.memo_repository.delete(user_id, id).await
    }

    pub async fn get_user_memos(&self, user_id: Uuid) -> AppResult<Vec<MemoResponse>> {
        let mut memos = self.memo_repository.find_all_by_user_id(user_id).await?;
        memos.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
        Ok(memos.into_iter().map(MemoResponse::from).collect())
    }

    pub async fn search_memos(
        &self,
        query: &str,
        tag: Option<String>,
        user_id: Uuid,
        page: usize,
        limit: usize,
    ) -> AppResult<SearchResponse> {
        let memos = self.memo_repository.search(query, tag, user_id).await?;
        let total = memos.len();
        let page = page.max(1);
        let limit = limit.clamp(1, 100);
        let total_pages = total.div_ceil(limit);
        let start = (page - 1).saturating_mul(limit);
        let items = memos
            .into_iter()
            .skip(start)
            .take(limit)
            .map(MemoResponse::from)
            .collect();

        Ok(SearchResponse {
            items,
            total,
            page,
            total_pages,
        })
    }

    fn validate_memo(memo: &Memo) -> AppResult<()> {
        if memo.validate() {
            Ok(())
        } else {
            Err(AppError::ValidationError(
                "Title and content are required; tags must be non-empty and limited to 10".into(),
            ))
        }
    }
}
