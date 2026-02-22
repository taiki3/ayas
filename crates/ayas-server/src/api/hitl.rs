use std::sync::Arc;

use axum::extract::{Path, State};
use axum::response::sse::Event;
use axum::response::Sse;
use axum::{Json, Router, routing::{delete, get, post}};
use chrono::Utc;
use futures::{Stream, stream};
use serde::Serialize;
use serde_json::{json, Value};
use uuid::Uuid;

use ayas_checkpoint::prelude::{CheckpointConfigExt, GraphOutput};
use ayas_core::config::RunnableConfig;
use ayas_graph::compiled::StepInfo;

use crate::error::AppError;
use crate::extractors::ApiKeys;
use crate::graph_convert::{GraphBuildContext, convert_to_state_graph_with_context};
use crate::api::graph::{default_graph_factory, default_research_factory, default_tools_factory};
use crate::session::InterruptSession;
use crate::sse::{sse_done, sse_event};
use crate::state::AppState;
use crate::types::{ExecuteResumableRequest, GraphChannelDto, GraphEdgeDto, GraphNodeDto, ResumeRequest};

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum HitlSseEvent {
    NodeStart {
        node_id: String,
        step_number: usize,
    },
    NodeEnd {
        node_id: String,
        state: Value,
        step_number: usize,
    },
    Interrupted {
        session_id: String,
        checkpoint_id: String,
        interrupt_value: Value,
        state: Value,
    },
    Complete {
        output: Value,
        total_steps: usize,
    },
    Error {
        message: String,
    },
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/graph/execute-resumable", post(execute_resumable))
        .route("/graph/resume", post(resume))
        .route("/graph/sessions", get(list_sessions))
        .route("/graph/sessions/{id}", delete(cancel_session))
}

fn build_context(api_keys: ApiKeys) -> GraphBuildContext {
    GraphBuildContext {
        factory: default_graph_factory(),
        api_keys,
        research_factory: Some(default_research_factory()),
        tools_factory: Some(default_tools_factory()),
    }
}

async fn execute_resumable(
    State(state): State<AppState>,
    api_keys: ApiKeys,
    Json(req): Json<ExecuteResumableRequest>,
) -> Result<Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>>, AppError> {
    let context = build_context(api_keys);
    let compiled = convert_to_state_graph_with_context(&req.nodes, &req.edges, &req.channels, Some(context))?;

    let config = RunnableConfig::default().with_thread_id(&req.thread_id);

    let steps = Arc::new(std::sync::Mutex::new(Vec::new()));
    let steps_clone = steps.clone();
    let observer = move |info: StepInfo| {
        steps_clone.lock().unwrap().push(info);
    };

    let graph_def = json!({
        "nodes": req.nodes,
        "edges": req.edges,
        "channels": req.channels,
    });

    let mut events: Vec<Result<Event, std::convert::Infallible>> = Vec::new();

    match compiled
        .invoke_resumable_with_observer(req.input, &config, state.checkpoint_store.as_ref(), observer)
        .await
    {
        Ok(output) => {
            // Clone steps out of mutex to avoid holding MutexGuard across .await
            let captured_steps: Vec<StepInfo> = std::mem::take(&mut *steps.lock().unwrap());
            for step in &captured_steps {
                events.push(sse_event(&HitlSseEvent::NodeStart {
                    node_id: step.node_name.clone(),
                    step_number: step.step_number,
                }));
                events.push(sse_event(&HitlSseEvent::NodeEnd {
                    node_id: step.node_name.clone(),
                    state: step.state_after.clone(),
                    step_number: step.step_number,
                }));
            }

            match output {
                GraphOutput::Complete(final_state) => {
                    events.push(sse_event(&HitlSseEvent::Complete {
                        output: final_state,
                        total_steps: captured_steps.len(),
                    }));
                }
                GraphOutput::Interrupted {
                    checkpoint_id,
                    interrupt_value,
                    state: interrupt_state,
                } => {
                    let session_id = Uuid::new_v4().to_string();
                    let session = InterruptSession {
                        session_id: session_id.clone(),
                        thread_id: req.thread_id,
                        checkpoint_id: checkpoint_id.clone(),
                        interrupt_value: interrupt_value.clone(),
                        graph_definition: graph_def,
                        created_at: Utc::now(),
                    };
                    state.session_store.create(session).await;

                    events.push(sse_event(&HitlSseEvent::Interrupted {
                        session_id,
                        checkpoint_id,
                        interrupt_value,
                        state: interrupt_state,
                    }));
                }
            }
        }
        Err(e) => {
            events.push(sse_event(&HitlSseEvent::Error {
                message: e.to_string(),
            }));
        }
    }

    events.push(sse_done());
    Ok(Sse::new(stream::iter(events)))
}

async fn resume(
    State(state): State<AppState>,
    api_keys: ApiKeys,
    Json(req): Json<ResumeRequest>,
) -> Result<Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>>, AppError> {
    let session = state
        .session_store
        .get(&req.session_id)
        .await
        .ok_or_else(|| AppError::Internal(format!("Session '{}' not found", req.session_id)))?;

    // Reconstruct graph from stored definition
    let graph_def = &session.graph_definition;
    let nodes: Vec<GraphNodeDto> = serde_json::from_value(
        graph_def.get("nodes").cloned().unwrap_or(json!([])),
    )
    .map_err(|e| AppError::Internal(format!("Failed to deserialize graph nodes: {e}")))?;
    let edges: Vec<GraphEdgeDto> = serde_json::from_value(
        graph_def.get("edges").cloned().unwrap_or(json!([])),
    )
    .map_err(|e| AppError::Internal(format!("Failed to deserialize graph edges: {e}")))?;
    let channels: Vec<GraphChannelDto> = serde_json::from_value(
        graph_def.get("channels").cloned().unwrap_or(json!([])),
    )
    .map_err(|e| AppError::Internal(format!("Failed to deserialize graph channels: {e}")))?;

    let context = build_context(api_keys);
    let compiled = convert_to_state_graph_with_context(&nodes, &edges, &channels, Some(context))?;

    let config = RunnableConfig::default()
        .with_thread_id(&session.thread_id)
        .with_checkpoint_id(&session.checkpoint_id)
        .with_resume_value(req.resume_value);

    let steps = Arc::new(std::sync::Mutex::new(Vec::new()));
    let steps_clone = steps.clone();
    let observer = move |info: StepInfo| {
        steps_clone.lock().unwrap().push(info);
    };

    let mut events: Vec<Result<Event, std::convert::Infallible>> = Vec::new();

    match compiled
        .invoke_resumable_with_observer(json!({}), &config, state.checkpoint_store.as_ref(), observer)
        .await
    {
        Ok(output) => {
            // Clone steps out of mutex to avoid holding MutexGuard across .await
            let captured_steps: Vec<StepInfo> = std::mem::take(&mut *steps.lock().unwrap());
            for step in &captured_steps {
                events.push(sse_event(&HitlSseEvent::NodeStart {
                    node_id: step.node_name.clone(),
                    step_number: step.step_number,
                }));
                events.push(sse_event(&HitlSseEvent::NodeEnd {
                    node_id: step.node_name.clone(),
                    state: step.state_after.clone(),
                    step_number: step.step_number,
                }));
            }

            match output {
                GraphOutput::Complete(final_state) => {
                    // Execution completed; delete the session
                    state.session_store.delete(&req.session_id).await;
                    events.push(sse_event(&HitlSseEvent::Complete {
                        output: final_state,
                        total_steps: captured_steps.len(),
                    }));
                }
                GraphOutput::Interrupted {
                    checkpoint_id,
                    interrupt_value,
                    state: interrupt_state,
                } => {
                    // Interrupted again; update session with new checkpoint
                    let graph_def_clone = session.graph_definition.clone();
                    let updated_session = InterruptSession {
                        session_id: req.session_id.clone(),
                        thread_id: session.thread_id.clone(),
                        checkpoint_id: checkpoint_id.clone(),
                        interrupt_value: interrupt_value.clone(),
                        graph_definition: graph_def_clone,
                        created_at: Utc::now(),
                    };
                    // Delete old, create updated
                    state.session_store.delete(&req.session_id).await;
                    state.session_store.create(updated_session).await;

                    events.push(sse_event(&HitlSseEvent::Interrupted {
                        session_id: req.session_id,
                        checkpoint_id,
                        interrupt_value,
                        state: interrupt_state,
                    }));
                }
            }
        }
        Err(e) => {
            events.push(sse_event(&HitlSseEvent::Error {
                message: e.to_string(),
            }));
        }
    }

    events.push(sse_done());
    Ok(Sse::new(stream::iter(events)))
}

async fn list_sessions(
    State(state): State<AppState>,
) -> Json<Vec<InterruptSession>> {
    Json(state.session_store.list_pending().await)
}

async fn cancel_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    state
        .session_store
        .delete(&id)
        .await
        .ok_or_else(|| AppError::Internal(format!("Session '{id}' not found")))?;
    Ok(Json(json!({"status": "cancelled", "session_id": id})))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn app() -> Router {
        let state = AppState::with_smith_dir(
            ayas_smith::client::SmithConfig::default().base_dir,
        );
        Router::new().nest("/api", routes().with_state(state))
    }

    fn parse_sse_events(body: &[u8]) -> Vec<Value> {
        let text = String::from_utf8_lossy(body);
        text.lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .filter(|data| *data != "[DONE]")
            .filter_map(|data| serde_json::from_str(data.trim()).ok())
            .collect()
    }

    #[tokio::test]
    async fn execute_resumable_completes_without_interrupt() {
        let app = app();
        let body = json!({
            "thread_id": "test-thread-1",
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
                    .uri("/api/graph/execute-resumable")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let events = parse_sse_events(&bytes);
        let complete = events.iter().find(|e| e["type"] == "complete");
        assert!(complete.is_some(), "Expected complete event, got: {events:?}");
        assert_eq!(complete.unwrap()["output"]["value"], "hello");
    }

    #[tokio::test]
    async fn execute_resumable_returns_interrupted() {
        let app = app();
        let body = json!({
            "thread_id": "test-interrupt-1",
            "nodes": [
                {"id": "n1", "type": "passthrough"},
                {"id": "blocker", "type": "interrupt", "config": {"value": "approve?"}},
                {"id": "n2", "type": "passthrough"}
            ],
            "edges": [
                {"from": "start", "to": "n1"},
                {"from": "n1", "to": "blocker"},
                {"from": "blocker", "to": "n2"},
                {"from": "n2", "to": "end"}
            ],
            "channels": [{"key": "value", "type": "LastValue"}],
            "input": {"value": "test"}
        });

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/graph/execute-resumable")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let events = parse_sse_events(&bytes);
        let interrupted = events.iter().find(|e| e["type"] == "interrupted");
        assert!(interrupted.is_some(), "Expected interrupted event, got: {events:?}");
        let interrupted = interrupted.unwrap();
        assert!(!interrupted["session_id"].as_str().unwrap().is_empty());
        assert_eq!(interrupted["interrupt_value"], "approve?");
    }

    #[tokio::test]
    async fn full_interrupt_resume_cycle() {
        // Use shared state so both requests share the same session/checkpoint stores
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::with_smith_dir(dir.path().to_path_buf());
        let app = Router::new()
            .route("/api/graph/execute-resumable", post(execute_resumable))
            .route("/api/graph/resume", post(resume))
            .route("/api/graph/sessions", get(list_sessions))
            .with_state(state);

        // Step 1: Execute and get interrupted
        let body = json!({
            "thread_id": "resume-thread",
            "nodes": [
                {"id": "n1", "type": "passthrough"},
                {"id": "blocker", "type": "interrupt", "config": {"value": "approve?"}},
                {"id": "n2", "type": "passthrough"}
            ],
            "edges": [
                {"from": "start", "to": "n1"},
                {"from": "n1", "to": "blocker"},
                {"from": "blocker", "to": "n2"},
                {"from": "n2", "to": "end"}
            ],
            "channels": [{"key": "value", "type": "LastValue"}],
            "input": {"value": "data"}
        });

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/graph/execute-resumable")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let events = parse_sse_events(&bytes);
        let interrupted = events.iter().find(|e| e["type"] == "interrupted").unwrap();
        let session_id = interrupted["session_id"].as_str().unwrap().to_string();

        // Step 2: Verify session is listed
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/graph/sessions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let sessions: Vec<Value> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0]["session_id"], session_id);

        // Step 3: Resume
        let resume_body = json!({
            "session_id": session_id,
            "resume_value": "approved"
        });

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/graph/resume")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_string(&resume_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let events = parse_sse_events(&bytes);
        let complete = events.iter().find(|e| e["type"] == "complete");
        assert!(complete.is_some(), "Expected complete event after resume, got: {events:?}");

        // Step 4: Verify session is cleaned up
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/graph/sessions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let sessions: Vec<Value> = serde_json::from_slice(&bytes).unwrap();
        assert!(sessions.is_empty());
    }

    #[tokio::test]
    async fn list_sessions_empty() {
        let app = app();
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/graph/sessions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let sessions: Vec<Value> = serde_json::from_slice(&bytes).unwrap();
        assert!(sessions.is_empty());
    }

    #[tokio::test]
    async fn cancel_session_success() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::with_smith_dir(dir.path().to_path_buf());
        let session = InterruptSession {
            session_id: "cancel-me".into(),
            thread_id: "t1".into(),
            checkpoint_id: "cp1".into(),
            interrupt_value: json!("question"),
            graph_definition: json!({}),
            created_at: Utc::now(),
        };
        state.session_store.create(session).await;

        let app = Router::new()
            .route("/api/graph/sessions/{id}", delete(cancel_session))
            .with_state(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/graph/sessions/cancel-me")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let result: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(result["status"], "cancelled");
    }

    #[tokio::test]
    async fn cancel_nonexistent_session() {
        let app = app();
        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/graph/sessions/does-not-exist")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn resume_nonexistent_session() {
        let app = app();
        let body = json!({
            "session_id": "no-such-session",
            "resume_value": "approved"
        });

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/graph/resume")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
