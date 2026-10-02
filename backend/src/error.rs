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

    #[error("Service Unavailable: {0}")]
    ServiceUnavailable(String),
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
            AppError::ValidationError(msg) => {
                HttpResponse::UnprocessableEntity().json(ErrorResponse {
                    error: "Validation Error".into(),
                    message: msg.clone(),
                })
            }
            AppError::Unauthorized(msg) => HttpResponse::Unauthorized().json(ErrorResponse {
                error: "Unauthorized".into(),
                message: msg.clone(),
            }),
            AppError::ServiceUnavailable(msg) => {
                log::warn!("Request failed because a required service is unavailable: {msg}");
                HttpResponse::ServiceUnavailable().json(ErrorResponse {
                    error: "Service Unavailable".into(),
                    message: "A required service is temporarily unavailable".into(),
                })
            }
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


#[cfg(test)]
mod tests {
    use actix_web::{body::to_bytes, http::StatusCode};
    use serde_json::Value;

    use super::*;

    #[tokio::test]
    async fn service_unavailable_response_hides_internal_detail() {
        let response = AppError::ServiceUnavailable(
            "memo mutation failed; primary=Database Error: secret backend detail".into(),
        )
        .error_response();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

        let body = to_bytes(response.into_body())
            .await
            .expect("service unavailable response body");
        let json: Value = serde_json::from_slice(&body).expect("valid JSON error response");

        assert_eq!(json["error"], "Service Unavailable");
        assert_eq!(
            json["message"],
            "A required service is temporarily unavailable"
        );
        assert!(!String::from_utf8_lossy(&body).contains("secret backend detail"));
    }

    #[tokio::test]
    async fn client_actionable_errors_keep_their_existing_detail() {
        let response = AppError::ValidationError("Title is required".into()).error_response();
        let body = to_bytes(response.into_body())
            .await
            .expect("validation response body");
        let json: Value = serde_json::from_slice(&body).expect("valid JSON error response");

        assert_eq!(json["message"], "Title is required");
    }
}
