use std::sync::Arc;

use axum::{Json, Router, routing::post};
use axum::response::Sse;
use axum::response::sse::Event;
use futures::stream;
use futures::Stream;

use ayas_core::config::RunnableConfig;
use ayas_core::runnable::Runnable;
use ayas_deep_research::gemini::GeminiInteractionsClient;
use ayas_deep_research::runnable::{DeepResearchInput, DeepResearchRunnable};

use crate::error::AppError;
use crate::extractors::ApiKeys;
use crate::sse::{sse_done, sse_event};
use crate::types::ResearchInvokeRequest;

pub fn routes() -> Router {
    Router::new().route("/research/invoke", post(research_invoke))
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ResearchSseEvent {
    Progress { message: String },
    Complete { text: String, interaction_id: String },
    Error { message: String },
}

async fn research_invoke(
    api_keys: ApiKeys,
    Json(req): Json<ResearchInvokeRequest>,
) -> Result<Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>>, AppError> {
    // Deep research only works with Gemini
    let api_key = api_keys.get_key_for(&ayas_llm::provider::Provider::Gemini)?;

    let client = Arc::new(GeminiInteractionsClient::new(api_key));
    let runnable = DeepResearchRunnable::new(client);

    let mut input = DeepResearchInput::new(&req.query);
    if let Some(agent) = req.agent {
        input = input.with_agent(agent);
    }
    if let Some(prev_id) = req.previous_interaction_id {
        input = input.with_previous_interaction_id(prev_id);
    }

    let mut events: Vec<Result<Event, std::convert::Infallible>> = Vec::new();

    events.push(sse_event(&ResearchSseEvent::Progress {
        message: "Starting deep research...".into(),
    }));

    let config = RunnableConfig::default();
    match runnable.invoke(input, &config).await {
        Ok(output) => {
            events.push(sse_event(&ResearchSseEvent::Complete {
                text: output.text,
                interaction_id: output.interaction_id,
            }));
        }
        Err(e) => {
            events.push(sse_event(&ResearchSseEvent::Error {
                message: e.to_string(),
            }));
        }
    }

    events.push(sse_done());

    Ok(Sse::new(stream::iter(events)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use tower::ServiceExt;

    fn app() -> Router {
        Router::new().nest("/api", routes())
    }

    #[tokio::test]
    async fn research_invoke_missing_key() {
        // Ensure env var fallback doesn't interfere
        unsafe { std::env::remove_var("GEMINI_API_KEY"); }
        let app = app();
        let body = serde_json::json!({
            "query": "What is quantum computing?"
        });

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/research/invoke")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn research_invoke_invalid_json() {
        let app = app();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/research/invoke")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("not json"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
