use actix_web::{web::Data, HttpResponse};

use crate::application::health::HealthService;

pub async fn live() -> HttpResponse {
    HttpResponse::Ok().json(serde_json::json!({
        "status": "ok",
        "timestamp": chrono::Utc::now().to_rfc3339()
    }))
}

pub async fn ready(service: Data<HealthService>) -> HttpResponse {
    let readiness = service.readiness().await;

    if readiness.ready {
        HttpResponse::Ok().json(readiness)
    } else {
        HttpResponse::ServiceUnavailable().json(readiness)
    }
}
