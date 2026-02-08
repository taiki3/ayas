pub mod agent;
pub mod chat;
pub mod graph;
pub mod research;

use axum::{Router, routing::get};

pub fn api_routes() -> Router {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .nest(
            "/api",
            chat::routes()
                .merge(agent::routes())
                .merge(graph::routes())
                .merge(research::routes()),
        )
}
