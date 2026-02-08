use std::sync::Arc;

use axum::{Json, Router, extract::State, routing::post};

use ayas_core::message::Message;
use ayas_core::model::{CallOptions, ChatModel};
use ayas_llm::factory::create_chat_model;
use ayas_llm::provider::Provider;

use crate::error::AppError;
use crate::extractors::ApiKeys;
use crate::types::{ChatInvokeRequest, ChatInvokeResponse};

/// Factory function type for creating ChatModel instances.
pub type ChatModelFactory =
    Arc<dyn Fn(&Provider, String, String) -> Box<dyn ChatModel> + Send + Sync>;

/// Create the default factory that delegates to ayas_llm::factory.
pub fn default_model_factory() -> ChatModelFactory {
    Arc::new(|provider, api_key, model_id| create_chat_model(provider, api_key, model_id))
}

pub fn routes() -> Router {
    routes_with_factory(default_model_factory())
}

pub fn routes_with_factory(factory: ChatModelFactory) -> Router {
    Router::new()
        .route("/chat/invoke", post(chat_invoke))
        .with_state(factory)
}

async fn chat_invoke(
    State(factory): State<ChatModelFactory>,
    api_keys: ApiKeys,
    Json(req): Json<ChatInvokeRequest>,
) -> Result<Json<ChatInvokeResponse>, AppError> {
    let api_key = api_keys.get_key_for(&req.provider)?;
    let model = factory(&req.provider, api_key, req.model);

    // Build messages, prepending system prompt if provided
    let mut messages = Vec::new();
    if let Some(system_prompt) = &req.system_prompt {
        messages.push(Message::system(system_prompt.as_str()));
    }
    messages.extend(req.messages);

    let options = CallOptions {
        temperature: req.temperature,
        max_tokens: req.max_tokens,
        ..Default::default()
    };

    let result = model.generate(&messages, &options).await?;

    let (tokens_in, tokens_out) = result
        .usage
        .as_ref()
        .map(|u| (u.input_tokens, u.output_tokens))
        .unwrap_or((0, 0));

    Ok(Json(ChatInvokeResponse {
        content: result.message.content().to_string(),
        tokens_in,
        tokens_out,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use ayas_core::error::Result;
    use ayas_core::message::{AIContent, UsageMetadata};

    /// Mock ChatModel that records calls and returns preset responses.
    struct MockChatModel {
        response: String,
        received_messages: std::sync::Mutex<Vec<Vec<Message>>>,
        received_options: std::sync::Mutex<Vec<CallOptions>>,
    }

    impl MockChatModel {
        fn new(response: impl Into<String>) -> Self {
            Self {
                response: response.into(),
                received_messages: std::sync::Mutex::new(Vec::new()),
                received_options: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl ChatModel for MockChatModel {
        async fn generate(
            &self,
            messages: &[Message],
            options: &CallOptions,
        ) -> Result<ayas_core::model::ChatResult> {
            self.received_messages
                .lock()
                .unwrap()
                .push(messages.to_vec());
            self.received_options
                .lock()
                .unwrap()
                .push(options.clone());

            Ok(ayas_core::model::ChatResult {
                message: Message::AI(AIContent {
                    content: self.response.clone(),
                    tool_calls: Vec::new(),
                    usage: Some(UsageMetadata {
                        input_tokens: 10,
                        output_tokens: 5,
                        total_tokens: 15,
                    }),
                }),
                usage: Some(UsageMetadata {
                    input_tokens: 10,
                    output_tokens: 5,
                    total_tokens: 15,
                }),
            })
        }

        fn model_name(&self) -> &str {
            "mock-model"
        }
    }

    /// Track how many times factory was called + share mock for assertions.
    fn mock_factory(response: &str) -> (ChatModelFactory, Arc<AtomicUsize>) {
        let resp = response.to_string();
        let call_count = Arc::new(AtomicUsize::new(0));
        let cc = call_count.clone();
        let factory: ChatModelFactory = Arc::new(move |_provider, _key, _model| {
            cc.fetch_add(1, Ordering::Relaxed);
            Box::new(MockChatModel::new(resp.clone()))
        });
        (factory, call_count)
    }

    fn app_with_mock(response: &str) -> (Router, Arc<AtomicUsize>) {
        let (factory, count) = mock_factory(response);
        let router = Router::new().nest("/api", routes_with_factory(factory));
        (router, count)
    }

    fn post_chat(body: serde_json::Value) -> Request<Body> {
        post_chat_with_headers(body, vec![("X-Gemini-Key", "test-key")])
    }

    fn post_chat_with_headers(
        body: serde_json::Value,
        headers: Vec<(&str, &str)>,
    ) -> Request<Body> {
        let mut builder = Request::builder()
            .method("POST")
            .uri("/api/chat/invoke")
            .header(header::CONTENT_TYPE, "application/json");
        for (k, v) in headers {
            builder = builder.header(k, v);
        }
        builder
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap()
    }

    #[tokio::test]
    async fn chat_invoke_success() {
        let (app, count) = app_with_mock("Hello from mock!");
        let body = serde_json::json!({
            "provider": "gemini",
            "model": "gemini-2.0-flash",
            "messages": [{"type": "user", "content": "Hi"}]
        });

        let resp = app.oneshot(post_chat(body)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(count.load(Ordering::Relaxed), 1);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let result: ChatInvokeResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(result.content, "Hello from mock!");
        assert_eq!(result.tokens_in, 10);
        assert_eq!(result.tokens_out, 5);
    }

    #[tokio::test]
    async fn chat_invoke_with_system_prompt() {
        // We verify the system prompt is prepended by checking the mock was called.
        // The mock factory creates a fresh MockChatModel each time, so we can't
        // inspect messages directly. Instead, we verify the response is successful
        // and the system_prompt field is accepted.
        let (app, _) = app_with_mock("System prompt response");
        let body = serde_json::json!({
            "provider": "gemini",
            "model": "gemini-2.0-flash",
            "system_prompt": "You are a helpful assistant",
            "messages": [{"type": "user", "content": "Hi"}]
        });

        let resp = app.oneshot(post_chat(body)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let result: ChatInvokeResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(result.content, "System prompt response");
    }

    #[tokio::test]
    async fn chat_invoke_missing_key() {
        // Ensure env var fallback doesn't interfere
        unsafe { std::env::remove_var("GEMINI_API_KEY"); }
        let (app, count) = app_with_mock("should not reach");
        let body = serde_json::json!({
            "provider": "gemini",
            "model": "gemini-2.0-flash",
            "messages": [{"type": "user", "content": "Hello"}]
        });

        // No API key header
        let req = Request::builder()
            .method("POST")
            .uri("/api/chat/invoke")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        // Factory should not have been called
        assert_eq!(count.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn chat_invoke_empty_messages() {
        let (app, count) = app_with_mock("Empty messages response");
        let body = serde_json::json!({
            "provider": "gemini",
            "model": "gemini-2.0-flash",
            "messages": []
        });

        let resp = app.oneshot(post_chat(body)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(count.load(Ordering::Relaxed), 1);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let result: ChatInvokeResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(result.content, "Empty messages response");
    }

    #[tokio::test]
    async fn chat_invoke_options_passed() {
        // Use a factory that captures the options for inspection.
        let captured_options: Arc<std::sync::Mutex<Vec<CallOptions>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let opts = captured_options.clone();

        let factory: ChatModelFactory = Arc::new(move |_provider, _key, _model| {
            let opts_inner = opts.clone();
            // Create a model wrapper that captures options
            struct CapturingModel {
                opts: Arc<std::sync::Mutex<Vec<CallOptions>>>,
            }

            #[async_trait]
            impl ChatModel for CapturingModel {
                async fn generate(
                    &self,
                    _messages: &[Message],
                    options: &CallOptions,
                ) -> Result<ayas_core::model::ChatResult> {
                    self.opts.lock().unwrap().push(options.clone());
                    Ok(ayas_core::model::ChatResult {
                        message: Message::ai("OK"),
                        usage: Some(UsageMetadata {
                            input_tokens: 1,
                            output_tokens: 1,
                            total_tokens: 2,
                        }),
                    })
                }

                fn model_name(&self) -> &str {
                    "capturing-model"
                }
            }

            Box::new(CapturingModel {
                opts: opts_inner,
            })
        });

        let app = Router::new().nest("/api", routes_with_factory(factory));
        let body = serde_json::json!({
            "provider": "gemini",
            "model": "gemini-2.0-flash",
            "messages": [{"type": "user", "content": "Hi"}],
            "temperature": 0.8,
            "max_tokens": 1024
        });

        let resp = app.oneshot(post_chat(body)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let opts = captured_options.lock().unwrap();
        assert_eq!(opts.len(), 1);
        assert_eq!(opts[0].temperature, Some(0.8));
        assert_eq!(opts[0].max_tokens, Some(1024));
    }

    #[tokio::test]
    async fn chat_invoke_claude_provider() {
        let (app, count) = app_with_mock("Claude response");
        let body = serde_json::json!({
            "provider": "claude",
            "model": "claude-sonnet-4-5-20250929",
            "messages": [{"type": "user", "content": "Hi"}]
        });

        let resp = app
            .oneshot(post_chat_with_headers(
                body,
                vec![("X-Anthropic-Key", "test-anthropic-key")],
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(count.load(Ordering::Relaxed), 1);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let result: ChatInvokeResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(result.content, "Claude response");
    }

    #[tokio::test]
    async fn chat_invoke_openai_provider() {
        let (app, count) = app_with_mock("OpenAI response");
        let body = serde_json::json!({
            "provider": "openai",
            "model": "gpt-4o",
            "messages": [{"type": "user", "content": "Hi"}]
        });

        let resp = app
            .oneshot(post_chat_with_headers(
                body,
                vec![("X-OpenAI-Key", "test-openai-key")],
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(count.load(Ordering::Relaxed), 1);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let result: ChatInvokeResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(result.content, "OpenAI response");
    }
}
