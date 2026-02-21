//! Server-level E2E tests for graph execution and validation endpoints.
//!
//! These tests verify the HTTP API layer using axum test utilities.
//! They do not require real LLM API keys.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

async fn app() -> axum::Router {
    ayas_server::app_router().await
}

fn parse_sse_events(body: &[u8]) -> Vec<serde_json::Value> {
    let text = String::from_utf8_lossy(body);
    text.lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .filter(|data| *data != "[DONE]")
        .filter_map(|data| serde_json::from_str(data.trim()).ok())
        .collect()
}

// ---------------------------------------------------------------------------
// Test 8: POST /api/graph/execute with passthrough graph, verify SSE events
// ---------------------------------------------------------------------------

#[tokio::test]
async fn graph_execute_passthrough_sse() {
    let app = app().await;
    let body = serde_json::json!({
        "nodes": [{"id": "pass1", "type": "passthrough"}],
        "edges": [
            {"from": "start", "to": "pass1"},
            {"from": "pass1", "to": "end"}
        ],
        "channels": [{"key": "value", "type": "LastValue"}],
        "input": {"value": "hello_e2e"}
    });

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/graph/execute")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let events = parse_sse_events(&bytes);

    // Should have node_start, node_end, and complete events
    assert!(
        events
            .iter()
            .any(|e| e["type"] == "node_start" && e["node_id"] == "pass1"),
        "Expected node_start for pass1, got: {:?}",
        events
    );
    assert!(
        events
            .iter()
            .any(|e| e["type"] == "node_end" && e["node_id"] == "pass1"),
        "Expected node_end for pass1, got: {:?}",
        events
    );
    let complete = events.iter().find(|e| e["type"] == "complete");
    assert!(
        complete.is_some(),
        "Expected complete event, got: {:?}",
        events
    );
    assert_eq!(complete.unwrap()["output"]["value"], "hello_e2e");
}

// ---------------------------------------------------------------------------
// Test 9: POST /api/graph/invoke-stream with multi-node graph
// ---------------------------------------------------------------------------

#[tokio::test]
async fn graph_invoke_stream_multi_node() {
    let app = app().await;
    let body = serde_json::json!({
        "nodes": [
            {"id": "a", "type": "passthrough"},
            {"id": "b", "type": "passthrough"},
            {"id": "c", "type": "passthrough"}
        ],
        "edges": [
            {"from": "start", "to": "a"},
            {"from": "a", "to": "b"},
            {"from": "b", "to": "c"},
            {"from": "c", "to": "end"}
        ],
        "channels": [{"key": "data", "type": "LastValue"}],
        "input": {"data": "streaming_test"}
    });

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/graph/invoke-stream")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let events = parse_sse_events(&bytes);

    // Should have node_start and node_end for each of the 3 nodes
    let node_starts: Vec<_> = events
        .iter()
        .filter(|e| e["type"] == "node_start")
        .collect();
    let node_ends: Vec<_> = events
        .iter()
        .filter(|e| e["type"] == "node_end")
        .collect();

    assert!(
        node_starts.len() >= 3,
        "Expected 3+ node_start events, got {}: {:?}",
        node_starts.len(),
        events
    );
    assert!(
        node_ends.len() >= 3,
        "Expected 3+ node_end events, got {}: {:?}",
        node_ends.len(),
        events
    );

    // Should have graph_complete with the data preserved
    let complete = events.iter().find(|e| e["type"] == "graph_complete");
    assert!(
        complete.is_some(),
        "Expected graph_complete event, got: {:?}",
        events
    );
    assert_eq!(complete.unwrap()["output"]["data"], "streaming_test");
}

// ---------------------------------------------------------------------------
// Test 10: POST /api/graph/validate with valid and invalid graphs
// ---------------------------------------------------------------------------

#[tokio::test]
async fn graph_validate_valid_graph() {
    let app = app().await;
    let body = serde_json::json!({
        "nodes": [
            {"id": "n1", "type": "passthrough"},
            {"id": "n2", "type": "passthrough"}
        ],
        "edges": [
            {"from": "start", "to": "n1"},
            {"from": "n1", "to": "n2"},
            {"from": "n2", "to": "end"}
        ],
        "channels": [{"key": "value", "type": "LastValue"}]
    });

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/graph/validate")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let result: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(result["valid"], true);
    assert!(result["errors"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn graph_validate_invalid_graph() {
    let app = app().await;
    let body = serde_json::json!({
        "nodes": [{"id": "orphan", "type": "passthrough"}],
        "edges": [{"from": "orphan", "to": "end"}],
        "channels": []
    });

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/graph/validate")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let result: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(result["valid"], false);
    assert!(!result["errors"].as_array().unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// Test 11: Graph execute with tracing header, verify trace recorded
// ---------------------------------------------------------------------------

#[tokio::test]
async fn graph_execute_with_tracing_header() {
    let dir = tempfile::tempdir().unwrap();

    // Point smith writes to a temp directory so we can verify trace creation.
    // SAFETY: env var mutation is not thread-safe, but acceptable for test.
    unsafe {
        std::env::set_var("AYAS_SMITH_BASE_DIR", dir.path());
        std::env::set_var("AYAS_SMITH_PROJECT", "e2e-trace-test");
    }

    let app = app().await;
    let body = serde_json::json!({
        "nodes": [{"id": "traced", "type": "passthrough"}],
        "edges": [
            {"from": "start", "to": "traced"},
            {"from": "traced", "to": "end"}
        ],
        "channels": [{"key": "value", "type": "LastValue"}],
        "input": {"value": "traced_data"}
    });

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/graph/execute")
                .header(header::CONTENT_TYPE, "application/json")
                .header("X-Trace-Enabled", "true")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let events = parse_sse_events(&bytes);

    // Verify normal execution still works with tracing enabled
    let complete = events.iter().find(|e| e["type"] == "complete");
    assert!(
        complete.is_some(),
        "Expected complete event with tracing, got: {:?}",
        events
    );
    assert_eq!(complete.unwrap()["output"]["value"], "traced_data");

    // Allow background writer time to flush
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // Verify trace was recorded
    assert!(
        dir.path().join("e2e-trace-test").exists(),
        "Expected trace project directory at {:?}",
        dir.path().join("e2e-trace-test")
    );

    // Cleanup env vars
    unsafe {
        std::env::remove_var("AYAS_SMITH_BASE_DIR");
        std::env::remove_var("AYAS_SMITH_PROJECT");
    }
}
