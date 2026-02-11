use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::message::{Message, UsageMetadata};

fn default_true() -> bool {
    true
}

/// Desired response format for structured output.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseFormat {
    /// Free-form text (default, equivalent to omitting the field).
    Text,
    /// Force JSON output (no schema).
    JsonObject,
    /// Force JSON output conforming to a schema.
    JsonSchema {
        name: String,
        schema: serde_json::Value,
        /// OpenAI strict mode (default true).
        #[serde(default = "default_true")]
        strict: bool,
    },
}

/// Options controlling a ChatModel invocation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CallOptions {
    /// Maximum tokens to generate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,

    /// Sampling temperature (0.0 - 2.0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,

    /// Tool definitions available for the model to call.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<crate::tool::ToolDefinition>,

    /// Stop sequences.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stop: Vec<String>,

    /// Structured output format.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_format: Option<ResponseFormat>,
}

/// Result of a chat model generation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResult {
    /// The generated message.
    pub message: Message,

    /// Token usage metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageMetadata>,
}

/// Events emitted during streaming model generation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
#[serde(rename_all = "snake_case")]
pub enum ChatStreamEvent {
    /// A text token from the model.
    Token(String),
    /// Start of a tool call.
    ToolCallStart { id: String, name: String },
    /// Partial arguments for an in-progress tool call.
    ToolCallDelta { id: String, arguments: String },
    /// Token usage metadata.
    Usage(UsageMetadata),
    /// Stream completed.
    Done,
}

/// Trait for chat language models.
///
/// Implementations should handle API communication, request formatting,
/// and response parsing for a specific model provider.
#[async_trait]
pub trait ChatModel: Send + Sync {
    /// Generate a response for the given messages.
    async fn generate(
        &self,
        messages: &[Message],
        options: &CallOptions,
    ) -> Result<ChatResult>;

    /// Return the model name/identifier.
    fn model_name(&self) -> &str;

    /// Stream a response token by token.
    ///
    /// Default implementation calls `generate` and wraps the result as events.
    async fn stream(
        &self,
        messages: &[Message],
        options: &CallOptions,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<ChatStreamEvent>> + Send>>> {
        let result = self.generate(messages, options).await?;
        let mut events: Vec<Result<ChatStreamEvent>> = Vec::new();
        let content = result.message.content().to_string();
        if !content.is_empty() {
            events.push(Ok(ChatStreamEvent::Token(content)));
        }
        if let Message::AI(ref ai) = result.message {
            for tc in &ai.tool_calls {
                events.push(Ok(ChatStreamEvent::ToolCallStart {
                    id: tc.id.clone(),
                    name: tc.name.clone(),
                }));
                events.push(Ok(ChatStreamEvent::ToolCallDelta {
                    id: tc.id.clone(),
                    arguments: serde_json::to_string(&tc.arguments).unwrap_or_default(),
                }));
            }
        }
        if let Some(usage) = result.usage {
            events.push(Ok(ChatStreamEvent::Usage(usage)));
        }
        events.push(Ok(ChatStreamEvent::Done));
        Ok(Box::pin(futures::stream::iter(events)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{AIContent, ToolCall};
    use futures::StreamExt;

    struct MockChatModel {
        response: String,
    }

    #[async_trait]
    impl ChatModel for MockChatModel {
        async fn generate(
            &self,
            _messages: &[Message],
            _options: &CallOptions,
        ) -> Result<ChatResult> {
            Ok(ChatResult {
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

    #[tokio::test]
    async fn mock_chat_model_generate() {
        let model = MockChatModel {
            response: "Hello!".into(),
        };
        let messages = vec![Message::user("Hi")];
        let options = CallOptions::default();

        let result = model.generate(&messages, &options).await.unwrap();
        assert_eq!(result.message.content(), "Hello!");
        assert!(result.usage.is_some());
    }

    #[tokio::test]
    async fn mock_chat_model_name() {
        let model = MockChatModel {
            response: String::new(),
        };
        assert_eq!(model.model_name(), "mock-model");
    }

    #[test]
    fn call_options_default() {
        let opts = CallOptions::default();
        assert!(opts.max_tokens.is_none());
        assert!(opts.temperature.is_none());
        assert!(opts.tools.is_empty());
        assert!(opts.stop.is_empty());
    }

    // -----------------------------------------------------------------------
    // ChatStreamEvent tests
    // -----------------------------------------------------------------------

    #[test]
    fn stream_event_token_serde_roundtrip() {
        let event = ChatStreamEvent::Token("Hello".into());
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"token""#));
        let parsed: ChatStreamEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, parsed);
    }

    #[test]
    fn stream_event_tool_call_start_serde_roundtrip() {
        let event = ChatStreamEvent::ToolCallStart {
            id: "call_1".into(),
            name: "calc".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"tool_call_start""#));
        let parsed: ChatStreamEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, parsed);
    }

    #[test]
    fn stream_event_tool_call_delta_serde_roundtrip() {
        let event = ChatStreamEvent::ToolCallDelta {
            id: "call_1".into(),
            arguments: r#"{"expr"#.into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"tool_call_delta""#));
        let parsed: ChatStreamEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, parsed);
    }

    #[test]
    fn stream_event_usage_serde_roundtrip() {
        let event = ChatStreamEvent::Usage(UsageMetadata {
            input_tokens: 10,
            output_tokens: 5,
            total_tokens: 15,
        });
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"usage""#));
        let parsed: ChatStreamEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, parsed);
    }

    #[test]
    fn stream_event_done_serde_roundtrip() {
        let event = ChatStreamEvent::Done;
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"done""#));
        let parsed: ChatStreamEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, parsed);
    }

    #[tokio::test]
    async fn default_stream_text_response() {
        let model = MockChatModel {
            response: "Hello!".into(),
        };
        let messages = vec![Message::user("Hi")];
        let options = CallOptions::default();
        let mut stream = model.stream(&messages, &options).await.unwrap();

        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            events.push(event.unwrap());
        }

        assert_eq!(events.len(), 3); // Token + Usage + Done
        assert_eq!(events[0], ChatStreamEvent::Token("Hello!".into()));
        assert!(matches!(events[1], ChatStreamEvent::Usage(_)));
        assert_eq!(events[2], ChatStreamEvent::Done);
    }

    #[tokio::test]
    async fn default_stream_with_tool_calls() {
        let model = MockToolCallModel;
        let messages = vec![Message::user("calc 2+2")];
        let options = CallOptions::default();
        let mut stream = model.stream(&messages, &options).await.unwrap();

        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            events.push(event.unwrap());
        }

        // Token("thinking") + ToolCallStart + ToolCallDelta + Usage + Done
        assert_eq!(events.len(), 5);
        assert_eq!(events[0], ChatStreamEvent::Token("thinking".into()));
        assert!(matches!(events[1], ChatStreamEvent::ToolCallStart { .. }));
        assert!(matches!(events[2], ChatStreamEvent::ToolCallDelta { .. }));
        assert!(matches!(events[3], ChatStreamEvent::Usage(_)));
        assert_eq!(events[4], ChatStreamEvent::Done);
    }

    struct MockToolCallModel;

    #[async_trait]
    impl ChatModel for MockToolCallModel {
        async fn generate(
            &self,
            _messages: &[Message],
            _options: &CallOptions,
        ) -> Result<ChatResult> {
            Ok(ChatResult {
                message: Message::ai_with_tool_calls(
                    "thinking",
                    vec![ToolCall {
                        id: "call_1".into(),
                        name: "calculator".into(),
                        arguments: serde_json::json!({"expr": "2+2"}),
                    }],
                ),
                usage: Some(UsageMetadata {
                    input_tokens: 10,
                    output_tokens: 20,
                    total_tokens: 30,
                }),
            })
        }

        fn model_name(&self) -> &str {
            "mock-tool-call"
        }
    }

    #[test]
    fn chat_result_with_tool_calls() {
        let result = ChatResult {
            message: Message::ai_with_tool_calls(
                "",
                vec![ToolCall {
                    id: "call_1".into(),
                    name: "search".into(),
                    arguments: serde_json::json!({"q": "test"}),
                }],
            ),
            usage: None,
        };
        match &result.message {
            Message::AI(ai) => {
                assert_eq!(ai.tool_calls.len(), 1);
                assert_eq!(ai.tool_calls[0].name, "search");
            }
            _ => panic!("expected AI message"),
        }
    }
}
