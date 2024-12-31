// src/domain/user/repository.rs

use async_trait::async_trait;
use uuid::Uuid;
use crate::error::AppResult;
use super::entity::User;

#[async_trait]
pub trait UserRepository: Send + Sync {
    async fn find_by_id(&self, id: Uuid) -> AppResult<Option<User>>;
    async fn find_by_email(&self, email: &str) -> AppResult<Option<User>>;
    async fn save(&self, user: &User) -> AppResult<()>;
    async fn delete(&self, id: Uuid) -> AppResult<()>;
    async fn exists(&self, id: Uuid) -> AppResult<bool>;
    async fn exists_by_email(&self, email: &str) -> AppResult<bool>;
}