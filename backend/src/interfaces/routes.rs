use actix_web::web;
use crate::interfaces::rest::memo;

pub fn configure_routes(cfg: &mut web::ServiceConfig) {
    cfg
        .service(
            web::scope("/api/v1")
                .service(
                    web::scope("/memos")
                        .route("", web::post().to(memo::create_memo))
                        .route("", web::get().to(memo::list_memos))
                        .route("/search", web::get().to(memo::search_memos))
                        .route("/{id}", web::get().to(memo::get_memo))
                        .route("/{id}", web::patch().to(memo::update_memo))
                        .route("/{id}", web::delete().to(memo::delete_memo)),
                )
                .route("/health", web::get().to(memo::health_check)),
        );
}