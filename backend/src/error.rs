use actix_web::{HttpResponse, ResponseError};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum AppError {
    #[error("Internal Server Error: {0}")]
    InternalServerError(String),
    
    #[error("Not Found: {0}")]
    NotFound(String),
    
    #[error("Bad Request: {0}")]
    BadRequest(String),
    
    #[error("Validation Error: {0}")]
    ValidationError(String),
    
    #[error("Database Error: {0}")]
    DatabaseError(String),
    
    #[error("Conflict: {0}")]
    Conflict(String),

    #[error("Unauthorized: {0}")]
    Unauthorized(String),
}

impl ResponseError for AppError {
    fn error_response(&self) -> HttpResponse {
        match self {
            AppError::NotFound(msg) => HttpResponse::NotFound().json(ErrorResponse {
                error: "Not Found".into(),
                message: msg.clone(),
            }),
            AppError::BadRequest(msg) => HttpResponse::BadRequest().json(ErrorResponse {
                error: "Bad Request".into(),
                message: msg.clone(),
            }),
            AppError::ValidationError(msg) => HttpResponse::UnprocessableEntity().json(ErrorResponse {
                error: "Validation Error".into(),
                message: msg.clone(),
            }),
            AppError::Unauthorized(msg) => HttpResponse::Unauthorized().json(ErrorResponse {
                error: "Unauthorized".into(),
                message: msg.clone(),
            }),
            AppError::Conflict(msg) => HttpResponse::Conflict().json(ErrorResponse {
                error: "Conflict".into(),
                message: msg.clone(),
            }),
            _ => HttpResponse::InternalServerError().json(ErrorResponse {
                error: "Internal Server Error".into(),
                message: "An unexpected error occurred".into(),
            }),
        }
    }
}

#[derive(serde::Serialize)]
struct ErrorResponse {
    error: String,
    message: String,
}

pub type AppResult<T> = Result<T, AppError>;