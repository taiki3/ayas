use std::sync::Arc;

use axum::extract::State;
use axum::response::Sse;
use axum::response::sse::Event;
use axum::{Json, Router, routing::post};
use futures::Stream;
use futures::stream;
use serde::Serialize;

use ayas_core::config::RunnableConfig;
use ayas_deep_research::gemini::GeminiInteractionsClient;
use ayas_graph::compiled::StepInfo;
use ayas_graph::stream::StreamEvent;
use ayas_llm::factory::create_chat_model;

use crate::error::AppError;
use crate::extractors::ApiKeys;
use crate::graph_convert::{
    GraphBuildContext, GraphModelFactory, GraphResearchFactory, convert_to_state_graph_with_context,
    validate_graph,
};
use crate::graph_gen;
use crate::sse::{sse_done, sse_event};
use crate::tracing_middleware::{TracingContext, is_tracing_requested};
use crate::types::{
    GraphChannelDto, GraphEdgeDto, GraphExecuteRequest, GraphGenerateRequest,
    GraphGenerateResponse, GraphNodeDto, GraphStreamRequest, GraphValidateRequest,
    GraphValidateResponse,
};

/// Create the default factory that delegates to ayas_llm::factory.
pub fn default_graph_factory() -> GraphModelFactory {
    Arc::new(|provider, api_key, model_id| create_chat_model(provider, api_key, model_id))
}

/// Create the default research factory that creates GeminiInteractionsClient instances.
pub fn default_research_factory() -> GraphResearchFactory {
    Arc::new(|api_key| Arc::new(GeminiInteractionsClient::new(api_key)))
}

pub fn routes() -> Router {
    routes_with_factory(default_graph_factory())
}

pub fn routes_with_factory(factory: GraphModelFactory) -> Router {
    Router::new()
        .route("/graph/validate", post(graph_validate))
        .route("/graph/execute", post(graph_execute))
        .route("/graph/invoke-stream", post(graph_invoke_stream))
        .route("/graph/stream", post(graph_stream))
        .route("/graph/generate", post(graph_generate))
        .with_state(factory)
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
    State(factory): State<GraphModelFactory>,
    api_keys: ApiKeys,
    headers: axum::http::HeaderMap,
    Json(req): Json<GraphExecuteRequest>,
) -> Result<Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>>, AppError> {
    // Set up optional tracing (env var or per-request header)
    let tracing_ctx = TracingContext::from_env().or_else(|| {
        if is_tracing_requested(&headers) {
            Some(TracingContext::from_env_config())
        } else {
            None
        }
    });
    let trace_input = if tracing_ctx.is_some() {
        Some(req.input.clone())
    } else {
        None
    };

    let context = GraphBuildContext {
        factory,
        api_keys,
        research_factory: Some(default_research_factory()),
    };
    let compiled = convert_to_state_graph_with_context(
        &req.nodes, &req.edges, &req.channels, Some(context),
    )?;
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

            // Record trace (non-blocking)
            if let (Some(ctx), Some(input)) = (&tracing_ctx, &trace_input) {
                ctx.record_graph_run("graph-execute", input, &output, None);
            }

            events.push(sse_event(&GraphSseEvent::Complete {
                output,
                total_steps: captured_steps.len(),
            }));
        }
        Err(e) => {
            if let (Some(ctx), Some(input)) = (&tracing_ctx, &trace_input) {
                ctx.record_graph_run(
                    "graph-execute",
                    input,
                    &serde_json::Value::Null,
                    Some(&e.to_string()),
                );
            }
            events.push(sse_event(&GraphSseEvent::Error {
                message: e.to_string(),
            }));
        }
    }

    events.push(sse_done());
    Ok(Sse::new(stream::iter(events)))
}

async fn graph_invoke_stream(
    State(factory): State<GraphModelFactory>,
    api_keys: ApiKeys,
    headers: axum::http::HeaderMap,
    Json(req): Json<GraphExecuteRequest>,
) -> Result<Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>>, AppError> {
    let tracing_ctx = TracingContext::from_env().or_else(|| {
        if is_tracing_requested(&headers) {
            Some(TracingContext::from_env_config())
        } else {
            None
        }
    });
    let trace_input = if tracing_ctx.is_some() {
        Some(req.input.clone())
    } else {
        None
    };

    let context = GraphBuildContext {
        factory,
        api_keys,
        research_factory: Some(default_research_factory()),
    };
    let compiled = convert_to_state_graph_with_context(
        &req.nodes, &req.edges, &req.channels, Some(context),
    )?;
    let config = RunnableConfig::default();

    let (tx, mut rx) = tokio::sync::mpsc::channel::<StreamEvent>(64);

    let input = req.input;

    tokio::spawn(async move {
        let _ = compiled.invoke_with_streaming(input, &config, tx).await;
    });

    let stream = async_stream::stream! {
        let mut final_output = None;
        let mut had_error = false;

        while let Some(event) = rx.recv().await {
            match &event {
                StreamEvent::GraphComplete { output } => {
                    final_output = Some(output.clone());
                }
                StreamEvent::Error { message } => {
                    had_error = true;
                    if let (Some(ctx), Some(input)) = (&tracing_ctx, &trace_input) {
                        ctx.record_graph_run(
                            "graph-invoke-stream",
                            input,
                            &serde_json::Value::Null,
                            Some(message),
                        );
                    }
                }
                _ => {}
            }
            yield sse_event(&event);
        }

        if !had_error {
            if let (Some(ctx), Some(input)) = (&tracing_ctx, &trace_input) {
                let output = final_output.unwrap_or(serde_json::Value::Null);
                ctx.record_graph_run("graph-invoke-stream", input, &output, None);
            }
        }

        yield sse_done();
    };

    Ok(Sse::new(stream))
}

/// Multi-mode streaming endpoint.
async fn graph_stream(
    State(factory): State<GraphModelFactory>,
    api_keys: ApiKeys,
    Json(req): Json<GraphStreamRequest>,
) -> Result<Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>>, AppError> {
    use ayas_core::stream::{StreamEvent as CoreEvent, parse_stream_modes};

    let modes = parse_stream_modes(req.stream_mode.as_deref().unwrap_or(""))
        .map_err(|e| AppError::BadRequest(e))?;

    let context = GraphBuildContext {
        factory,
        api_keys,
        research_factory: Some(default_research_factory()),
    };
    let compiled = convert_to_state_graph_with_context(
        &req.nodes, &req.edges, &req.channels, Some(context),
    )?;
    let config = RunnableConfig::default();

    let (tx, mut rx) = tokio::sync::mpsc::channel::<CoreEvent>(64);

    let modes_clone = modes.clone();
    tokio::spawn(async move {
        let _ = compiled
            .stream_with_modes(req.input, &config, &modes_clone, tx)
            .await;
    });

    let stream = async_stream::stream! {
        while let Some(event) = rx.recv().await {
            let mode_name = match &event {
                CoreEvent::Values { .. } => "values",
                CoreEvent::Updates { .. } => "updates",
                CoreEvent::Message { .. } => "messages",
                CoreEvent::Debug { .. } => "debug",
                CoreEvent::GraphComplete { .. } => "complete",
                CoreEvent::Error { .. } => "error",
            };

            let json = serde_json::to_string(&event).unwrap_or_else(|_| "{}".into());
            yield Ok::<_, std::convert::Infallible>(
                Event::default().event(mode_name).data(json)
            );
        }
        yield sse_done();
    };

    Ok(Sse::new(stream))
}

async fn graph_generate(
    State(factory): State<GraphModelFactory>,
    api_keys: ApiKeys,
    Json(req): Json<GraphGenerateRequest>,
) -> Result<Json<GraphGenerateResponse>, AppError> {
    let api_key = api_keys.get_key_for(&req.provider)?;
    let model = factory(&req.provider, api_key, req.model);

    // Try LLM generation, fall back to template on failure
    match graph_gen::generate_graph(model.as_ref(), &req.prompt).await {
        Ok((nodes, edges, channels)) => Ok(Json(GraphGenerateResponse {
            nodes,
            edges,
            channels,
        })),
        Err(_) => {
            // Fallback: return a simple template
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use ayas_core::error::Result;
    use ayas_core::message::{AIContent, UsageMetadata};
    use ayas_core::model::ChatModel;

    fn app() -> Router {
        Router::new().nest("/api", routes())
    }

    /// Mock ChatModel that returns a preset JSON graph response.
    struct MockGraphModel {
        response: String,
    }

    #[async_trait]
    impl ChatModel for MockGraphModel {
        async fn generate(
            &self,
            _messages: &[ayas_core::message::Message],
            _options: &ayas_core::model::CallOptions,
        ) -> Result<ayas_core::model::ChatResult> {
            Ok(ayas_core::model::ChatResult {
                message: ayas_core::message::Message::AI(AIContent {
                    content: self.response.clone(),
                    tool_calls: Vec::new(),
                    usage: Some(UsageMetadata {
                        input_tokens: 100,
                        output_tokens: 50,
                        total_tokens: 150,
                    }),
                }),
                usage: None,
            })
        }

        fn model_name(&self) -> &str {
            "mock-graph-model"
        }
    }

    fn mock_graph_factory(response: &str) -> (GraphModelFactory, Arc<AtomicUsize>) {
        let resp = response.to_string();
        let call_count = Arc::new(AtomicUsize::new(0));
        let cc = call_count.clone();
        let factory: GraphModelFactory = Arc::new(move |_provider, _key, _model| {
            cc.fetch_add(1, Ordering::Relaxed);
            Box::new(MockGraphModel {
                response: resp.clone(),
            })
        });
        (factory, call_count)
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
        let graph_json = r#"{"nodes":[{"id":"qa_llm","type":"llm","label":"Q&A Model"}],"edges":[{"from":"start","to":"qa_llm"},{"from":"qa_llm","to":"end"}],"channels":[{"key":"value","type":"LastValue"}]}"#;
        let (factory, count) = mock_graph_factory(graph_json);
        let app = Router::new().nest("/api", routes_with_factory(factory));
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
        assert_eq!(count.load(Ordering::Relaxed), 1);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let result: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(!result["nodes"].as_array().unwrap().is_empty());
        assert!(!result["edges"].as_array().unwrap().is_empty());
        assert_eq!(result["nodes"][0]["id"], "qa_llm");
    }

    #[tokio::test]
    async fn graph_generate_fallback_on_bad_response() {
        // Return invalid JSON that can't be parsed as a graph
        let (factory, _) = mock_graph_factory("I cannot generate a graph sorry");
        let app = Router::new().nest("/api", routes_with_factory(factory));
        let body = serde_json::json!({
            "prompt": "Something weird",
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
        // Falls back to template
        assert_eq!(result["nodes"][0]["id"], "transform_1");
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

    #[tokio::test]
    async fn test_graph_invoke_stream_linear() {
        let app = app();
        let body = serde_json::json!({
            "nodes": [
                {"id": "a", "type": "passthrough"},
                {"id": "b", "type": "passthrough"}
            ],
            "edges": [
                {"from": "start", "to": "a"},
                {"from": "a", "to": "b"},
                {"from": "b", "to": "end"}
            ],
            "channels": [{"key": "value", "type": "LastValue"}],
            "input": {"value": "hello"}
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

        assert!(
            events.iter().any(|e| e["type"] == "node_start"),
            "Expected node_start event, got: {:?}",
            events
        );
        assert!(
            events.iter().any(|e| e["type"] == "node_end"),
            "Expected node_end event, got: {:?}",
            events
        );
        assert!(
            events.iter().any(|e| e["type"] == "graph_complete"),
            "Expected graph_complete event, got: {:?}",
            events
        );
    }

    #[tokio::test]
    async fn test_graph_invoke_stream_empty_graph() {
        let app = app();
        let body = serde_json::json!({
            "nodes": [],
            "edges": [
                {"from": "start", "to": "end"}
            ],
            "channels": [{"key": "value", "type": "LastValue"}],
            "input": {"value": "test"}
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

        assert!(
            events.iter().any(|e| e["type"] == "graph_complete"),
            "Expected graph_complete event, got: {:?}",
            events
        );
    }

    #[tokio::test]
    async fn test_graph_execute_with_mock_llm() {
        let (factory, call_count) = mock_graph_factory("LLM response text");
        let app = Router::new().nest("/api", routes_with_factory(factory));

        let body = serde_json::json!({
            "nodes": [{"id": "llm_1", "type": "llm", "config": {
                "prompt": "Summarize",
                "provider": "gemini",
                "model": "gemini-2.0-flash"
            }}],
            "edges": [
                {"from": "start", "to": "llm_1"},
                {"from": "llm_1", "to": "end"}
            ],
            "channels": [{"key": "value", "type": "LastValue"}],
            "input": {"value": "some text"}
        });

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/graph/execute")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header("X-Gemini-Key", "test-key")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(call_count.load(Ordering::Relaxed), 1);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let events = parse_sse_events(&bytes);
        let complete = events.iter().find(|e| e["type"] == "complete");
        assert!(complete.is_some(), "Expected complete event, got: {:?}", events);
        assert_eq!(complete.unwrap()["output"]["value"], "LLM response text");
    }

    #[tokio::test]
    async fn test_graph_invoke_stream_with_mock_llm() {
        let (factory, _call_count) = mock_graph_factory("Streamed LLM output");
        let app = Router::new().nest("/api", routes_with_factory(factory));

        let body = serde_json::json!({
            "nodes": [{"id": "llm_1", "type": "llm", "config": {
                "prompt": "Translate",
                "provider": "gemini",
                "model": "gemini-2.0-flash"
            }}],
            "edges": [
                {"from": "start", "to": "llm_1"},
                {"from": "llm_1", "to": "end"}
            ],
            "channels": [{"key": "value", "type": "LastValue"}],
            "input": {"value": "hello"}
        });

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/graph/invoke-stream")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header("X-Gemini-Key", "test-key")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let events = parse_sse_events(&bytes);
        let complete = events.iter().find(|e| e["type"] == "graph_complete");
        assert!(complete.is_some(), "Expected graph_complete, got: {:?}", events);
        // LLM node should have written mock response to "value" channel
        assert_eq!(complete.unwrap()["output"]["value"], "Streamed LLM output");
    }
}
