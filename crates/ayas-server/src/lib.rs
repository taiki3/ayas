pub mod error;
pub mod extractors;
pub mod session;
pub mod state;
pub mod types;
pub mod run_types;
pub mod tools;
pub mod graph_convert;
pub mod graph_gen;
pub mod tracing_middleware;
pub mod tracing_mw;
pub mod api;
pub mod sse;

use axum::Router;
use tower_http::cors::{CorsLayer, Any};

use crate::state::AppState;
use crate::tracing_mw::TracingLayer;

pub async fn app_router() -> Router {
    let state = AppState::new().await;
    app_router_with_state(state)
}

pub fn app_router_with_state(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let tracing = TracingLayer::new(state.smith_client.clone());

    api::api_routes(state).layer(cors).layer(tracing)
}
