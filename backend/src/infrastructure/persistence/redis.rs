use std::time::Duration;

use async_trait::async_trait;
use redis::{AsyncCommands, Client};
use serde::{de::DeserializeOwned, Serialize};
use tracing::error;

use crate::{
    application::health::HealthProbe,
    error::{AppError, AppResult},
};

pub struct RedisCache {
    client: Client,
}

impl RedisCache {
    pub fn new(uri: &str) -> AppResult<Self> {
        let client = Client::open(uri).map_err(|error| {
            error!("Failed to create Redis client: {error}");
            AppError::DatabaseError(format!("Failed to create Redis client: {error}"))
        })?;
        Ok(Self { client })
    }

    pub async fn get<T: DeserializeOwned>(&self, key: &str) -> AppResult<Option<T>> {
        let mut connection = self.connection().await?;
        let value: Option<String> = connection.get(key).await.map_err(|error| {
            error!("Failed to get Redis value: {error}");
            AppError::DatabaseError(error.to_string())
        })?;

        value
            .map(|serialized| {
                serde_json::from_str(&serialized).map_err(|error| {
                    error!("Failed to deserialize Redis value: {error}");
                    AppError::DatabaseError(error.to_string())
                })
            })
            .transpose()
    }

    pub async fn set<T: Serialize>(
        &self,
        key: &str,
        value: &T,
        expiration: Option<Duration>,
    ) -> AppResult<()> {
        let mut connection = self.connection().await?;
        let serialized = serde_json::to_string(value).map_err(|error| {
            error!("Failed to serialize Redis value: {error}");
            AppError::DatabaseError(error.to_string())
        })?;

        match expiration {
            Some(expiration) => {
                let _: () = connection
                    .set_ex(key, serialized, expiration.as_secs())
                    .await
                    .map_err(|error| {
                        error!("Failed to set Redis value with expiration: {error}");
                        AppError::DatabaseError(error.to_string())
                    })?;
            }
            None => {
                let _: () = connection.set(key, serialized).await.map_err(|error| {
                    error!("Failed to set Redis value: {error}");
                    AppError::DatabaseError(error.to_string())
                })?;
            }
        }

        Ok(())
    }

    pub async fn delete(&self, key: &str) -> AppResult<()> {
        let mut connection = self.connection().await?;
        let _: usize = connection.del(key).await.map_err(|error| {
            error!("Failed to delete Redis key: {error}");
            AppError::DatabaseError(error.to_string())
        })?;
        Ok(())
    }

    pub async fn exists(&self, key: &str) -> AppResult<bool> {
        let mut connection = self.connection().await?;
        connection.exists(key).await.map_err(|error| {
            error!("Failed to check Redis key existence: {error}");
            AppError::DatabaseError(error.to_string())
        })
    }

    pub async fn health_check(&self) -> AppResult<bool> {
        let mut connection = self.connection().await?;
        let pong: String = redis::cmd("PING")
            .query_async(&mut connection)
            .await
            .map_err(|error| {
                error!("Redis health check failed: {error}");
                AppError::DatabaseError(error.to_string())
            })?;
        Ok(pong == "PONG")
    }

    async fn connection(&self) -> AppResult<redis::aio::MultiplexedConnection> {
        self.client
            .get_multiplexed_async_connection()
            .await
            .map_err(|error| {
                error!("Failed to get Redis connection: {error}");
                AppError::DatabaseError(error.to_string())
            })
    }
}

#[async_trait]
impl HealthProbe for RedisCache {
    async fn check(&self) -> bool {
        match self.health_check().await {
            Ok(healthy) => healthy,
            Err(error) => {
                log::warn!("Redis health check failed: {error}");
                false
            }
        }
    }
}
