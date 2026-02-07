use async_trait::async_trait;

use ayas_core::config::RunnableConfig;
use ayas_core::error::{AyasError, ChainError, Result};
use ayas_core::message::Message;
use ayas_core::runnable::Runnable;

/// Parses a `Vec<Message>` to extract the last AI message content as a String.
pub struct StringOutputParser;

#[async_trait]
impl Runnable for StringOutputParser {
    type Input = Vec<Message>;
    type Output = String;

    async fn invoke(
        &self,
        input: Self::Input,
        _config: &RunnableConfig,
    ) -> Result<Self::Output> {
        // Find the last AI message and return its content.
        for msg in input.iter().rev() {
            if let Message::AI(ai) = msg {
                return Ok(ai.content.clone());
            }
        }
        Err(AyasError::Chain(ChainError::Parse(
            "no AI message found in output".into(),
        )))
    }
}

/// Parses a single `Message` to extract its text content.
pub struct MessageContentParser;

#[async_trait]
impl Runnable for MessageContentParser {
    type Input = Message;
    type Output = String;

    async fn invoke(
        &self,
        input: Self::Input,
        _config: &RunnableConfig,
    ) -> Result<Self::Output> {
        Ok(input.content().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn string_output_parser_success() {
        let parser = StringOutputParser;
        let messages = vec![
            Message::user("Hi"),
            Message::ai("Hello! How can I help you?"),
        ];
        let config = RunnableConfig::default();
        let result = parser.invoke(messages, &config).await.unwrap();
        assert_eq!(result, "Hello! How can I help you?");
    }

    #[tokio::test]
    async fn string_output_parser_last_ai_message() {
        let parser = StringOutputParser;
        let messages = vec![
            Message::user("Hi"),
            Message::ai("First response"),
            Message::user("Continue"),
            Message::ai("Second response"),
        ];
        let config = RunnableConfig::default();
        let result = parser.invoke(messages, &config).await.unwrap();
        assert_eq!(result, "Second response");
    }

    #[tokio::test]
    async fn string_output_parser_no_ai_message() {
        let parser = StringOutputParser;
        let messages = vec![Message::user("Hi"), Message::system("You are helpful")];
        let config = RunnableConfig::default();
        let result = parser.invoke(messages, &config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn message_content_parser() {
        let parser = MessageContentParser;
        let config = RunnableConfig::default();

        let result = parser
            .invoke(Message::ai("hello"), &config)
            .await
            .unwrap();
        assert_eq!(result, "hello");

        let result = parser
            .invoke(Message::user("question"), &config)
            .await
            .unwrap();
        assert_eq!(result, "question");
    }
}
