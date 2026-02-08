use async_trait::async_trait;

use ayas_core::config::RunnableConfig;
use ayas_core::error::Result;
use ayas_core::message::Message;
use ayas_core::model::{CallOptions, ChatModel, ChatResult};
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
}
