pub mod agent;
pub mod chat;
pub mod datasets;
pub mod feedback;
pub mod graph;
pub mod hitl;
pub mod pipeline;
pub mod projects;
pub mod research;
pub mod runs;

use axum::{Router, routing::get, Json};
use serde::Serialize;

use crate::state::AppState;

#[derive(Serialize)]
struct EnvKeysResponse {
    gemini: bool,
    claude: bool,
    openai: bool,
}

async fn env_keys() -> Json<EnvKeysResponse> {
    Json(EnvKeysResponse {
        gemini: std::env::var("GEMINI_API_KEY").ok().filter(|s| !s.is_empty()).is_some(),
        claude: std::env::var("ANTHROPIC_API_KEY").ok().filter(|s| !s.is_empty()).is_some(),
        openai: std::env::var("OPENAI_API_KEY").ok().filter(|s| !s.is_empty()).is_some(),
    })
}

pub fn api_routes(state: AppState) -> Router {
    // Stateful routes: convert Router<AppState> to Router<()> via .with_state()
    let stateful: Router = runs::routes()
        .merge(feedback::routes())
        .merge(projects::routes())
        .merge(datasets::routes())
        .with_state(state);

    // Stateless routes (already Router<()>)
    let stateless: Router = chat::routes()
        .merge(agent::routes())
        .merge(graph::routes())
        .merge(research::routes())
        .merge(hitl::routes())
        .merge(pipeline::routes());

    Router::new()
        .route("/health", get(|| async { "ok" }))
        .nest("/api", stateful.merge(stateless).route("/env-keys", get(env_keys)))
}
