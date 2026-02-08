pub mod error;
pub mod extractors;
pub mod types;
pub mod tools;
pub mod graph_convert;
pub mod api;
pub mod sse;

use axum::Router;
use tower_http::cors::{CorsLayer, Any};

pub fn app_router() -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    api::api_routes().layer(cors)
}
