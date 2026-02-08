use axum::response::Sse;
use axum::response::sse::Event;
use axum::{Json, Router, routing::post};
use futures::Stream;
use futures::stream;
use serde::Serialize;

use ayas_core::config::RunnableConfig;
use ayas_graph::compiled::StepInfo;

use crate::error::AppError;
use crate::extractors::ApiKeys;
use crate::graph_convert::{convert_to_state_graph, validate_graph};
use crate::sse::{sse_done, sse_event};
use crate::types::{
    GraphChannelDto, GraphEdgeDto, GraphExecuteRequest, GraphGenerateRequest,
    GraphGenerateResponse, GraphNodeDto, GraphValidateRequest, GraphValidateResponse,
};

pub fn routes() -> Router {
    Router::new()
        .route("/graph/validate", post(graph_validate))
        .route("/graph/execute", post(graph_execute))
        .route("/graph/generate", post(graph_generate))
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum GraphSseEvent {
    NodeStart {
        node_id: String,
        step_number: usize,
    },
    NodeEnd {
        node_id: String,
        state: serde_json::Value,
        step_number: usize,
    },
    Complete {
        output: serde_json::Value,
        total_steps: usize,
    },
    Error {
        message: String,
    },
}

async fn graph_validate(Json(req): Json<GraphValidateRequest>) -> Json<GraphValidateResponse> {
    let errors = validate_graph(&req.nodes, &req.edges, &req.channels);
    Json(GraphValidateResponse {
        valid: errors.is_empty(),
        errors,
    })
}

async fn graph_execute(
    Json(req): Json<GraphExecuteRequest>,
) -> Result<Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>>, AppError> {
    let compiled = convert_to_state_graph(&req.nodes, &req.edges, &req.channels)?;
    let config = RunnableConfig::default();

    let mut events: Vec<Result<Event, std::convert::Infallible>> = Vec::new();
    let steps = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let steps_clone = steps.clone();

    let observer = move |info: StepInfo| {
        steps_clone.lock().unwrap().push(info);
    };

    match compiled
        .invoke_with_observer(req.input, &config, observer)
        .await
    {
        Ok(output) => {
            let captured_steps = steps.lock().unwrap();
            for step in captured_steps.iter() {
                events.push(sse_event(&GraphSseEvent::NodeStart {
                    node_id: step.node_name.clone(),
                    step_number: step.step_number,
                }));
                events.push(sse_event(&GraphSseEvent::NodeEnd {
                    node_id: step.node_name.clone(),
                    state: step.state_after.clone(),
                    step_number: step.step_number,
                }));
            }
            events.push(sse_event(&GraphSseEvent::Complete {
                output,
                total_steps: captured_steps.len(),
            }));
        }
        Err(e) => {
            events.push(sse_event(&GraphSseEvent::Error {
                message: e.to_string(),
            }));
        }
    }

    events.push(sse_done());
    Ok(Sse::new(stream::iter(events)))
}

async fn graph_generate(
    api_keys: ApiKeys,
    Json(req): Json<GraphGenerateRequest>,
) -> Result<Json<GraphGenerateResponse>, AppError> {
    // For now, return a simple template graph
    // In the future, this would use an LLM to generate the graph
    let _api_key = api_keys.get_key_for(&req.provider)?;

    let nodes = vec![GraphNodeDto {
        id: "transform_1".into(),
        node_type: "transform".into(),
        label: Some("Process Input".into()),
        config: Some(serde_json::json!({"expression": "process"})),
    }];
    let edges = vec![
        GraphEdgeDto {
            from: "start".into(),
            to: "transform_1".into(),
            condition: None,
        },
        GraphEdgeDto {
            from: "transform_1".into(),
            to: "end".into(),
            condition: None,
        },
    ];
    let channels = vec![GraphChannelDto {
        key: "value".into(),
        channel_type: "LastValue".into(),
        default: None,
    }];

    Ok(Json(GraphGenerateResponse {
        nodes,
        edges,
        channels,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn app() -> Router {
        Router::new().nest("/api", routes())
    }

    #[tokio::test]
    async fn graph_validate_valid() {
        let app = app();
        let body = serde_json::json!({
            "nodes": [{"id": "n1", "type": "passthrough"}],
            "edges": [
                {"from": "start", "to": "n1"},
                {"from": "n1", "to": "end"}
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
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let result: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(result["valid"], true);
    }

    #[tokio::test]
    async fn graph_validate_no_start() {
        let app = app();
        let body = serde_json::json!({
            "nodes": [{"id": "n1", "type": "passthrough"}],
            "edges": [{"from": "n1", "to": "end"}],
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
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let result: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(result["valid"], false);
        assert!(!result["errors"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn graph_validate_unreachable() {
        let app = app();
        let body = serde_json::json!({
            "nodes": [
                {"id": "n1", "type": "passthrough"},
                {"id": "n2", "type": "passthrough"}
            ],
            "edges": [
                {"from": "start", "to": "n1"},
                {"from": "n1", "to": "end"}
            ],
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

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let result: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(result["valid"], false);
        let errors = result["errors"].as_array().unwrap();
        assert!(errors
            .iter()
            .any(|e| e.as_str().unwrap().contains("unreachable")));
    }

    #[tokio::test]
    async fn graph_execute_linear() {
        let app = app();
        let body = serde_json::json!({
            "nodes": [{"id": "n1", "type": "passthrough"}],
            "edges": [
                {"from": "start", "to": "n1"},
                {"from": "n1", "to": "end"}
            ],
            "channels": [{"key": "value", "type": "LastValue"}],
            "input": {"value": "hello"}
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
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let body_str = String::from_utf8_lossy(&body);
        assert!(body_str.contains("node_start"));
        assert!(body_str.contains("complete"));
    }

    #[tokio::test]
    async fn graph_generate_returns_nodes() {
        let app = app();
        let body = serde_json::json!({
            "prompt": "Create a simple pipeline",
            "provider": "gemini",
            "model": "gemini-2.0-flash"
        });

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/graph/generate")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header("X-Gemini-Key", "test-key")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let result: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(!result["nodes"].as_array().unwrap().is_empty());
        assert!(!result["edges"].as_array().unwrap().is_empty());
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
    async fn graph_execute_start_to_end() {
        let app = app();
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
            "channels": [{"key": "value", "type": "LastValue"}],
            "input": {"value": "test_data"}
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

        // Should have node_start/node_end pairs for both nodes, plus complete
        let complete_event = events.iter().find(|e| e["type"] == "complete");
        assert!(complete_event.is_some(), "Expected complete event, got: {:?}", events);
        let output = &complete_event.unwrap()["output"];
        assert_eq!(output["value"], "test_data");
    }

    #[tokio::test]
    async fn graph_validate_edge_to_unknown_node() {
        let app = app();
        let body = serde_json::json!({
            "nodes": [{"id": "n1", "type": "passthrough"}],
            "edges": [
                {"from": "start", "to": "n1"},
                {"from": "n1", "to": "unknown_node"},
                {"from": "unknown_node", "to": "end"}
            ],
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
        let errors = result["errors"].as_array().unwrap();
        assert!(errors
            .iter()
            .any(|e| e.as_str().unwrap().contains("unknown_node")));
    }
}
