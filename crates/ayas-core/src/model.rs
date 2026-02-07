use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::message::{Message, UsageMetadata};

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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{AIContent, ToolCall};

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
