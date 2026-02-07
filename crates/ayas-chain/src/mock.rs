use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;

use ayas_core::config::RunnableConfig;
use ayas_core::error::Result;
use ayas_core::message::{AIContent, Message};
use ayas_core::runnable::Runnable;

/// A mock ChatModel that returns preset responses and tracks call counts.
///
/// This implements `Runnable<Input = Vec<Message>, Output = Vec<Message>>`
/// to be composable in chains with PromptTemplate and OutputParser.
pub struct MockChatModel {
    responses: Vec<String>,
    call_count: AtomicUsize,
}

impl MockChatModel {
    /// Create a `MockChatModel` that cycles through the given responses.
    pub fn new(responses: Vec<String>) -> Self {
        Self {
            responses,
            call_count: AtomicUsize::new(0),
        }
    }

    /// Create a `MockChatModel` that always returns the same response.
    pub fn with_response(response: impl Into<String>) -> Self {
        Self::new(vec![response.into()])
    }

    /// Get the number of times this model has been invoked.
    pub fn call_count(&self) -> usize {
        self.call_count.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl Runnable for MockChatModel {
    type Input = Vec<Message>;
    type Output = Vec<Message>;

    async fn invoke(
        &self,
        mut input: Self::Input,
        _config: &RunnableConfig,
    ) -> Result<Self::Output> {
        let idx = self.call_count.fetch_add(1, Ordering::Relaxed);
        let response = &self.responses[idx % self.responses.len()];

        input.push(Message::AI(AIContent {
            content: response.clone(),
            tool_calls: Vec::new(),
            usage: None,
        }));

        Ok(input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_single_response() {
        let model = MockChatModel::with_response("Hello!");
        let config = RunnableConfig::default();

        let messages = vec![Message::user("Hi")];
        let result = model.invoke(messages, &config).await.unwrap();

        assert_eq!(result.len(), 2);
        assert_eq!(result[1].content(), "Hello!");
        assert_eq!(model.call_count(), 1);
    }

    #[tokio::test]
    async fn mock_cycling_responses() {
        let model = MockChatModel::new(vec!["First".into(), "Second".into()]);
        let config = RunnableConfig::default();

        let r1 = model
            .invoke(vec![Message::user("1")], &config)
            .await
            .unwrap();
        assert_eq!(r1.last().unwrap().content(), "First");

        let r2 = model
            .invoke(vec![Message::user("2")], &config)
            .await
            .unwrap();
        assert_eq!(r2.last().unwrap().content(), "Second");

        let r3 = model
            .invoke(vec![Message::user("3")], &config)
            .await
            .unwrap();
        assert_eq!(r3.last().unwrap().content(), "First"); // cycles back

        assert_eq!(model.call_count(), 3);
    }

    #[tokio::test]
    async fn mock_preserves_input_messages() {
        let model = MockChatModel::with_response("Response");
        let config = RunnableConfig::default();

        let messages = vec![Message::system("Be helpful"), Message::user("Question")];
        let result = model.invoke(messages, &config).await.unwrap();

        assert_eq!(result.len(), 3);
        assert_eq!(result[0].content(), "Be helpful");
        assert_eq!(result[1].content(), "Question");
        assert_eq!(result[2].content(), "Response");
    }
}
