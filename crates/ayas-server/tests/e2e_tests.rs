//! End-to-end tests that hit real LLM APIs.
//! Run with: cargo test -p ayas-server --test e2e_tests -- --ignored

use axum::body::Body;
use axum::http::{Request, header};
use http_body_util::BodyExt;
use tower::ServiceExt;

/// Create the full app router for E2E tests.
fn app() -> axum::Router {
    ayas_server::app_router()
}

fn parse_sse_events(body: &[u8]) -> Vec<serde_json::Value> {
    let text = String::from_utf8_lossy(body);
    text.lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .filter(|data| *data != "[DONE]")
        .filter_map(|data| serde_json::from_str(data.trim()).ok())
        .collect()
}

#[tokio::test]
#[ignore]
async fn e2e_chat_gemini() {
    let key = std::env::var("GEMINI_API_KEY").expect("GEMINI_API_KEY must be set");
    let app = app();
    let body = serde_json::json!({
        "provider": "gemini",
        "model": "gemini-2.0-flash",
        "messages": [{"type": "user", "content": "Say hello in one word"}]
    });

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/chat/invoke")
                .header(header::CONTENT_TYPE, "application/json")
                .header("X-Gemini-Key", &key)
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let result: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(
        !result["content"].as_str().unwrap().is_empty(),
        "Expected non-empty content"
    );
}

#[tokio::test]
#[ignore]
async fn e2e_agent_gemini_calculator() {
    let key = std::env::var("GEMINI_API_KEY").expect("GEMINI_API_KEY must be set");
    let app = app();
    let body = serde_json::json!({
        "provider": "gemini",
        "model": "gemini-2.0-flash",
        "tools": ["calculator"],
        "messages": [{"type": "user", "content": "What is 123 * 456? Use the calculator tool."}],
        "recursion_limit": 5
    });

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/agent/invoke")
                .header(header::CONTENT_TYPE, "application/json")
                .header("X-Gemini-Key", &key)
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let events = parse_sse_events(&bytes);
    assert!(!events.is_empty(), "Expected SSE events");

    // Should have at least a step and a message or done event
    let has_step = events.iter().any(|e| e["type"] == "step");
    let has_message_or_done = events
        .iter()
        .any(|e| e["type"] == "message" || e["type"] == "done");
    assert!(has_step, "Expected step event in: {:?}", events);
    assert!(
        has_message_or_done,
        "Expected message or done event in: {:?}",
        events
    );
}

#[tokio::test]
#[ignore]
async fn e2e_agent_claude_calculator() {
    let key = std::env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY must be set");
    let app = app();
    let body = serde_json::json!({
        "provider": "claude",
        "model": "claude-sonnet-4-5-20250929",
        "tools": ["calculator"],
        "messages": [{"type": "user", "content": "What is 99 + 1? Use the calculator tool."}],
        "recursion_limit": 5
    });

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/agent/invoke")
                .header(header::CONTENT_TYPE, "application/json")
                .header("X-Anthropic-Key", &key)
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let events = parse_sse_events(&bytes);
    assert!(!events.is_empty(), "Expected SSE events");

    let has_step = events.iter().any(|e| e["type"] == "step");
    assert!(has_step, "Expected step event in: {:?}", events);
}

#[tokio::test]
#[ignore]
async fn e2e_agent_openai_calculator() {
    let key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY must be set");
    let app = app();
    let body = serde_json::json!({
        "provider": "openai",
        "model": "gpt-4o-mini",
        "tools": ["calculator"],
        "messages": [{"type": "user", "content": "What is 7 * 8? Use the calculator tool."}],
        "recursion_limit": 5
    });

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/agent/invoke")
                .header(header::CONTENT_TYPE, "application/json")
                .header("X-OpenAI-Key", &key)
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let events = parse_sse_events(&bytes);
    assert!(!events.is_empty(), "Expected SSE events");

    let has_step = events.iter().any(|e| e["type"] == "step");
    assert!(has_step, "Expected step event in: {:?}", events);
}
