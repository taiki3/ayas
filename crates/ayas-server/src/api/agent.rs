use std::sync::Arc;

use axum::{Json, Router, extract::State, routing::post};
use axum::response::Sse;
use axum::response::sse::Event;
use futures::stream;
use futures::Stream;

use ayas_core::message::Message;
use ayas_core::model::{CallOptions, ChatModel};
use ayas_llm::factory::create_chat_model;
use ayas_llm::provider::Provider;

use crate::error::AppError;
use crate::extractors::ApiKeys;
use crate::sse::{sse_done, sse_event};
use crate::tools::build_tools;
use crate::types::{AgentInvokeRequest, AgentSseEvent};

/// Factory function type for creating ChatModel instances (same as chat.rs).
pub type AgentModelFactory =
    Arc<dyn Fn(&Provider, String, String) -> Box<dyn ChatModel> + Send + Sync>;

/// Create the default factory that delegates to ayas_llm::factory.
pub fn default_agent_factory() -> AgentModelFactory {
    Arc::new(|provider, api_key, model_id| create_chat_model(provider, api_key, model_id))
}

pub fn routes() -> Router {
    routes_with_factory(default_agent_factory())
}

pub fn routes_with_factory(factory: AgentModelFactory) -> Router {
    Router::new()
        .route("/agent/invoke", post(agent_invoke))
        .with_state(factory)
}

async fn agent_invoke(
    State(factory): State<AgentModelFactory>,
    api_keys: ApiKeys,
    Json(req): Json<AgentInvokeRequest>,
) -> Result<Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>>, AppError> {
    let api_key = api_keys.get_key_for(&req.provider)?;
    let model = factory(&req.provider, api_key, req.model);
    let tools = build_tools(&req.tools, Vec::new());
    let tool_defs: Vec<_> = tools.iter().map(|t| t.definition()).collect();
    let recursion_limit = req.recursion_limit.unwrap_or(10);

    let mut messages = req.messages;
    let options = CallOptions {
        tools: tool_defs,
        ..Default::default()
    };

    let mut events: Vec<Result<Event, std::convert::Infallible>> = Vec::new();
    let mut step = 0;

    loop {
        if step >= recursion_limit {
            events.push(sse_event(&AgentSseEvent::Error {
                message: format!("Recursion limit ({recursion_limit}) exceeded"),
            }));
            break;
        }

        // Emit step event for model call
        events.push(sse_event(&AgentSseEvent::Step {
            step_number: step,
            node_name: "agent".into(),
            summary: format!("Step {}: Calling LLM", step),
        }));

        let result = match model.generate(&messages, &options).await {
            Ok(r) => r,
            Err(e) => {
                events.push(sse_event(&AgentSseEvent::Error {
                    message: e.to_string(),
                }));
                break;
            }
        };

        // Check for tool calls
        let has_tool_calls = match &result.message {
            Message::AI(ai) => !ai.tool_calls.is_empty(),
            _ => false,
        };

        if has_tool_calls {
            let tool_calls = match &result.message {
                Message::AI(ai) => ai.tool_calls.clone(),
                _ => vec![],
            };

            messages.push(result.message);

            for tc in &tool_calls {
                events.push(sse_event(&AgentSseEvent::ToolCall {
                    tool_name: tc.name.clone(),
                    arguments: tc.arguments.clone(),
                }));

                // Find and execute tool
                let tool_result =
                    if let Some(tool) = tools.iter().find(|t| t.definition().name == tc.name) {
                        match tool.call(tc.arguments.clone()).await {
                            Ok(r) => r,
                            Err(e) => format!("Tool error: {e}"),
                        }
                    } else {
                        format!("Tool '{}' not found", tc.name)
                    };

                events.push(sse_event(&AgentSseEvent::ToolResult {
                    tool_name: tc.name.clone(),
                    result: tool_result.clone(),
                }));

                messages.push(Message::tool(tool_result, &tc.id));
            }

            step += 1;
        } else {
            // No tool calls - final response
            let content = result.message.content().to_string();
            messages.push(result.message);

            events.push(sse_event(&AgentSseEvent::Message { content }));
            events.push(sse_event(&AgentSseEvent::Done {
                total_steps: step + 1,
            }));
            break;
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
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use async_trait::async_trait;
    use ayas_core::error::ModelError;
    use ayas_core::message::{AIContent, ToolCall};
    use ayas_core::model::ChatResult;

    /// Mock ChatModel that returns a sequence of responses.
    struct SequenceMockModel {
        responses: std::sync::Mutex<Vec<ChatResult>>,
    }

    impl SequenceMockModel {
        fn new(responses: Vec<ChatResult>) -> Self {
            Self {
                responses: std::sync::Mutex::new(responses),
            }
        }
    }

    #[async_trait]
    impl ChatModel for SequenceMockModel {
        async fn generate(
            &self,
            _messages: &[Message],
            _options: &CallOptions,
        ) -> ayas_core::error::Result<ChatResult> {
            let mut responses = self.responses.lock().unwrap();
            if responses.is_empty() {
                Ok(ChatResult {
                    message: Message::ai("fallback"),
                    usage: None,
                })
            } else {
                Ok(responses.remove(0))
            }
        }

        fn model_name(&self) -> &str {
            "sequence-mock"
        }
    }

    /// Mock ChatModel that always returns an error.
    struct ErrorMockModel;

    #[async_trait]
    impl ChatModel for ErrorMockModel {
        async fn generate(
            &self,
            _messages: &[Message],
            _options: &CallOptions,
        ) -> ayas_core::error::Result<ChatResult> {
            Err(ModelError::ApiRequest("Mock model error".into()).into())
        }

        fn model_name(&self) -> &str {
            "error-mock"
        }
    }

    fn parse_sse_events(body: &[u8]) -> Vec<serde_json::Value> {
        let text = String::from_utf8_lossy(body);
        text.lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .filter(|data| *data != "[DONE]")
            .filter_map(|data| serde_json::from_str(data.trim()).ok())
            .collect()
    }

    fn app_with_sequence(responses: Vec<ChatResult>) -> Router {
        let factory: AgentModelFactory = Arc::new(move |_provider, _key, _model| {
            let responses = responses.clone();
            Box::new(SequenceMockModel::new(responses))
        });
        Router::new().nest("/api", routes_with_factory(factory))
    }

    fn app_with_error() -> Router {
        let factory: AgentModelFactory =
            Arc::new(|_provider, _key, _model| Box::new(ErrorMockModel));
        Router::new().nest("/api", routes_with_factory(factory))
    }

    fn post_agent(body: serde_json::Value) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/api/agent/invoke")
            .header(header::CONTENT_TYPE, "application/json")
            .header("X-Gemini-Key", "test-key")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap()
    }

    fn text_response(content: &str) -> ChatResult {
        ChatResult {
            message: Message::AI(AIContent {
                content: content.to_string(),
                tool_calls: Vec::new(),
                usage: None,
            }),
            usage: None,
        }
    }

    fn tool_call_response(tool_name: &str, args: serde_json::Value) -> ChatResult {
        ChatResult {
            message: Message::AI(AIContent {
                content: String::new(),
                tool_calls: vec![ToolCall {
                    id: format!("call_{}", tool_name),
                    name: tool_name.to_string(),
                    arguments: args,
                }],
                usage: None,
            }),
            usage: None,
        }
    }

    fn app() -> Router {
        Router::new().nest("/api", routes())
    }

    #[tokio::test]
    async fn agent_invoke_missing_key() {
        // Ensure env var fallback doesn't interfere
        unsafe {
            std::env::remove_var("GEMINI_API_KEY");
        }
        let app = app();
        let body = serde_json::json!({
            "provider": "gemini",
            "model": "gemini-2.0-flash",
            "tools": [],
            "messages": [{"type": "user", "content": "Hello"}]
        });

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/agent/invoke")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn agent_invoke_invalid_json() {
        let app = app();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/agent/invoke")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("not json"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn agent_invoke_no_tools_direct_response() {
        let app = app_with_sequence(vec![text_response("Hello, world!")]);
        let body = serde_json::json!({
            "provider": "gemini",
            "model": "test-model",
            "tools": [],
            "messages": [{"type": "user", "content": "Say hello"}]
        });

        let resp = app.oneshot(post_agent(body)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let events = parse_sse_events(&bytes);

        // Should have: step, message, done
        assert!(events.len() >= 3, "Expected at least 3 events, got {}: {:?}", events.len(), events);

        // First event is a step
        assert_eq!(events[0]["type"], "step");
        assert_eq!(events[0]["node_name"], "agent");

        // Second event is the message
        assert_eq!(events[1]["type"], "message");
        assert_eq!(events[1]["content"], "Hello, world!");

        // Third event is done
        assert_eq!(events[2]["type"], "done");
        assert_eq!(events[2]["total_steps"], 1);
    }

    #[tokio::test]
    async fn agent_invoke_with_tool_call() {
        let app = app_with_sequence(vec![
            tool_call_response("calculator", serde_json::json!({"expression": "2+2"})),
            text_response("The answer is 4"),
        ]);
        let body = serde_json::json!({
            "provider": "gemini",
            "model": "test-model",
            "tools": ["calculator"],
            "messages": [{"type": "user", "content": "What is 2+2?"}]
        });

        let resp = app.oneshot(post_agent(body)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let events = parse_sse_events(&bytes);

        // step → tool_call → tool_result → step → message → done
        assert!(events.len() >= 6, "Expected at least 6 events, got {}: {:?}", events.len(), events);

        assert_eq!(events[0]["type"], "step");
        assert_eq!(events[1]["type"], "tool_call");
        assert_eq!(events[1]["tool_name"], "calculator");
        assert_eq!(events[2]["type"], "tool_result");
        assert_eq!(events[2]["tool_name"], "calculator");
        assert_eq!(events[3]["type"], "step");
        assert_eq!(events[4]["type"], "message");
        assert_eq!(events[4]["content"], "The answer is 4");
        assert_eq!(events[5]["type"], "done");
    }

    #[tokio::test]
    async fn agent_invoke_recursion_limit() {
        // Mock always returns tool calls → will hit recursion limit
        let responses: Vec<ChatResult> = (0..5)
            .map(|i| tool_call_response("calculator", serde_json::json!({"expression": format!("{i}+1")})))
            .collect();
        let app = app_with_sequence(responses);
        let body = serde_json::json!({
            "provider": "gemini",
            "model": "test-model",
            "tools": ["calculator"],
            "messages": [{"type": "user", "content": "Loop forever"}],
            "recursion_limit": 2
        });

        let resp = app.oneshot(post_agent(body)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let events = parse_sse_events(&bytes);

        // Should end with an error event about recursion limit
        let error_event = events.iter().find(|e| e["type"] == "error");
        assert!(error_event.is_some(), "Expected error event, got: {:?}", events);
        let msg = error_event.unwrap()["message"].as_str().unwrap();
        assert!(msg.contains("Recursion limit"), "Error message: {}", msg);
    }

    #[tokio::test]
    async fn agent_invoke_model_error() {
        let app = app_with_error();
        let body = serde_json::json!({
            "provider": "gemini",
            "model": "test-model",
            "tools": [],
            "messages": [{"type": "user", "content": "Fail"}]
        });

        let resp = app.oneshot(post_agent(body)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let events = parse_sse_events(&bytes);

        // Should have: step, error
        let error_event = events.iter().find(|e| e["type"] == "error");
        assert!(error_event.is_some(), "Expected error event, got: {:?}", events);
        let msg = error_event.unwrap()["message"].as_str().unwrap();
        assert!(msg.contains("Mock model error"), "Error message: {}", msg);
    }

    #[tokio::test]
    async fn agent_invoke_tool_not_found() {
        let app = app_with_sequence(vec![
            tool_call_response("nonexistent_tool", serde_json::json!({})),
            text_response("Done"),
        ]);
        let body = serde_json::json!({
            "provider": "gemini",
            "model": "test-model",
            "tools": [],
            "messages": [{"type": "user", "content": "Call a missing tool"}]
        });

        let resp = app.oneshot(post_agent(body)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let events = parse_sse_events(&bytes);

        // Should have tool_result with "not found" message
        let tool_result_event = events.iter().find(|e| e["type"] == "tool_result");
        assert!(tool_result_event.is_some(), "Expected tool_result event, got: {:?}", events);
        let result_str = tool_result_event.unwrap()["result"].as_str().unwrap();
        assert!(result_str.contains("not found"), "Tool result: {}", result_str);
    }
}
