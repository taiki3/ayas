pub mod agent;
pub mod chat;
pub mod feedback;
pub mod graph;
pub mod hitl;
pub mod research;
pub mod runs;

use axum::{Router, routing::get};

use crate::state::AppState;

pub fn api_routes(state: AppState) -> Router {
    // Stateful routes: convert Router<AppState> to Router<()> via .with_state()
    let stateful: Router = runs::routes()
        .merge(feedback::routes())
        .with_state(state);

    // Stateless routes (already Router<()>)
    let stateless: Router = chat::routes()
        .merge(agent::routes())
        .merge(graph::routes())
        .merge(research::routes())
        .merge(hitl::routes());

    Router::new()
        .route("/health", get(|| async { "ok" }))
        .nest("/api", stateful.merge(stateless))
}
