// src/domain/user/entity.rs

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: Uuid,
    pub email: String,
    pub name: String,
    pub password_hash: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl User {
    pub fn new(email: String, name: String, password_hash: String) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            email,
            name,
            password_hash,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn update_profile(&mut self, name: Option<String>) {
        if let Some(name) = name {
            self.name = name;
            self.updated_at = Utc::now();
        }
    }

    pub fn update_password(&mut self, password_hash: String) {
        self.password_hash = password_hash;
        self.updated_at = Utc::now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_new_user() {
        let email = "test@example.com".to_string();
        let name = "Test User".to_string();
        let password_hash = "hashed_password".to_string();

        let user = User::new(email.clone(), name.clone(), password_hash.clone());

        assert_eq!(user.email, email);
        assert_eq!(user.name, name);
        assert_eq!(user.password_hash, password_hash);
        assert!(user.id != Uuid::nil());
        assert!(user.created_at <= Utc::now());
        assert_eq!(user.created_at, user.updated_at);
    }

    #[test]
    fn test_update_profile() {
        let mut user = User::new(
            "test@example.com".to_string(),
            "Original Name".to_string(),
            "password_hash".to_string(),
        );
        let original_created_at = user.created_at;
        let original_updated_at = user.updated_at;

        std::thread::sleep(std::time::Duration::from_millis(10));
        let new_name = "Updated Name".to_string();
        user.update_profile(Some(new_name.clone()));

        assert_eq!(user.name, new_name);
        assert_eq!(user.created_at, original_created_at);
        assert!(user.updated_at > original_updated_at);
    }

    #[test]
    fn test_update_password() {
        let mut user = User::new(
            "test@example.com".to_string(),
            "Test User".to_string(),
            "original_hash".to_string(),
        );
        let original_created_at = user.created_at;
        let original_updated_at = user.updated_at;

        std::thread::sleep(std::time::Duration::from_millis(10));
        let new_password_hash = "new_hash".to_string();
        user.update_password(new_password_hash.clone());

        assert_eq!(user.password_hash, new_password_hash);
        assert_eq!(user.created_at, original_created_at);
        assert!(user.updated_at > original_updated_at);
    }
}