use std::collections::HashMap;
use std::pin::Pin;

use async_trait::async_trait;
use futures::{Stream, StreamExt};

use ayas_core::config::RunnableConfig;
use ayas_core::error::Result;
use ayas_core::message::{AIContent, Message, ToolCall, UsageMetadata};
use ayas_core::model::{CallOptions, ChatModel, ChatResult, ChatStreamEvent};
use ayas_core::runnable::Runnable;

/// Adapts a ChatModel to the Runnable trait.
///
/// Input: Vec<Message>, Output: Vec<Message> (input + AI response appended)
pub struct ChatModelRunnable<M: ChatModel> {
    model: M,
    options: CallOptions,
}

impl<M: ChatModel> ChatModelRunnable<M> {
    pub fn new(model: M, options: CallOptions) -> Self {
        Self { model, options }
    }

    pub fn model(&self) -> &M {
        &self.model
    }

    pub fn options(&self) -> &CallOptions {
        &self.options
    }
}

#[async_trait]
impl<M: ChatModel + 'static> Runnable for ChatModelRunnable<M> {
    type Input = Vec<Message>;
    type Output = Vec<Message>;

    async fn invoke(&self, input: Self::Input, _config: &RunnableConfig) -> Result<Self::Output> {
        let result: ChatResult = self.model.generate(&input, &self.options).await?;
        let mut messages = input;
        messages.push(result.message);
        Ok(messages)
    }

    async fn stream(
        &self,
        input: Self::Input,
        _config: &RunnableConfig,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Self::Output>> + Send>>>
    where
        Self::Output: 'static,
    {
        let event_stream = self.model.stream(&input, &self.options).await?;

        let output_stream = async_stream::stream! {
            let mut text = String::new();
            let mut tool_calls: Vec<ToolCall> = Vec::new();
            let mut usage: Option<UsageMetadata> = None;
            let mut tool_args: HashMap<String, String> = HashMap::new();

            let mut event_stream = Box::pin(event_stream);
            while let Some(event_result) = event_stream.next().await {
                match event_result {
                    Ok(ChatStreamEvent::Token(t)) => text.push_str(&t),
                    Ok(ChatStreamEvent::ToolCallStart { id, name }) => {
                        tool_calls.push(ToolCall {
                            id: id.clone(),
                            name,
                            arguments: serde_json::Value::Null,
                        });
                        tool_args.insert(id, String::new());
                    }
                    Ok(ChatStreamEvent::ToolCallDelta { id, arguments }) => {
                        if let Some(buf) = tool_args.get_mut(&id) {
                            buf.push_str(&arguments);
                        }
                    }
                    Ok(ChatStreamEvent::Usage(u)) => usage = Some(u),
                    Ok(ChatStreamEvent::Done) => break,
                    Err(e) => {
                        yield Err(e);
                        return;
                    }
                }
            }

            // Finalize tool call arguments
            for tc in &mut tool_calls {
                if let Some(args_str) = tool_args.get(&tc.id) {
                    tc.arguments = serde_json::from_str(args_str)
                        .unwrap_or(serde_json::Value::String(args_str.clone()));
                }
            }

            let mut messages = input;
            messages.push(Message::AI(AIContent {
                content: text,
                tool_calls,
                usage,
            }));
            yield Ok(messages);
        };

        Ok(Box::pin(output_stream))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ayas_core::error::{AyasError, ModelError};
    use ayas_core::message::{AIContent, UsageMetadata};

    struct MockModel {
        response: String,
    }

    #[async_trait]
    impl ChatModel for MockModel {
        async fn generate(
            &self,
            _messages: &[Message],
            _options: &CallOptions,
        ) -> ayas_core::error::Result<ChatResult> {
            Ok(ChatResult {
                message: Message::AI(AIContent {
                    content: self.response.clone(),
                    tool_calls: Vec::new(),
                    usage: Some(UsageMetadata {
                        input_tokens: 5,
                        output_tokens: 3,
                        total_tokens: 8,
                    }),
                }),
                usage: Some(UsageMetadata {
                    input_tokens: 5,
                    output_tokens: 3,
                    total_tokens: 8,
                }),
            })
        }
        fn model_name(&self) -> &str {
            "mock"
        }
    }

    struct ErrorModel;

    #[async_trait]
    impl ChatModel for ErrorModel {
        async fn generate(
            &self,
            _: &[Message],
            _: &CallOptions,
        ) -> ayas_core::error::Result<ChatResult> {
            Err(AyasError::Model(ModelError::ApiRequest(
                "mock error".into(),
            )))
        }
        fn model_name(&self) -> &str {
            "error-mock"
        }
    }

    #[tokio::test]
    async fn invoke_appends_response() {
        let r = ChatModelRunnable::new(
            MockModel {
                response: "Hello".into(),
            },
            CallOptions::default(),
        );
        let input = vec![Message::user("Hi")];
        let result = r.invoke(input, &RunnableConfig::default()).await.unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[1].content(), "Hello");
    }

    #[tokio::test]
    async fn invoke_propagates_error() {
        let r = ChatModelRunnable::new(ErrorModel, CallOptions::default());
        let input = vec![Message::user("Hi")];
        let result = r.invoke(input, &RunnableConfig::default()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn stream_uses_model_stream() {
        let r = ChatModelRunnable::new(
            MockModel {
                response: "Streamed!".into(),
            },
            CallOptions::default(),
        );
        let input = vec![Message::user("Hi")];
        let mut stream = r.stream(input, &RunnableConfig::default()).await.unwrap();
        let result = stream.next().await.unwrap().unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[1].content(), "Streamed!");
    }

    #[tokio::test]
    async fn stream_propagates_error() {
        let r = ChatModelRunnable::new(ErrorModel, CallOptions::default());
        let input = vec![Message::user("Hi")];
        let result = r.stream(input, &RunnableConfig::default()).await;
        assert!(result.is_err());
    }
}
