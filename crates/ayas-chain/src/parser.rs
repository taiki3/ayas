use std::marker::PhantomData;

use async_trait::async_trait;
use serde::de::DeserializeOwned;

use ayas_core::config::RunnableConfig;
use ayas_core::error::{AyasError, ChainError, Result};
use ayas_core::message::Message;
use ayas_core::runnable::Runnable;

/// Extracts the content of the last AI message from a `Vec<Message>`.
/// Returns a `ChainError::Parse` if no AI message is found.
fn extract_last_ai_content(messages: &[Message]) -> Result<String> {
    for msg in messages.iter().rev() {
        if let Message::AI(ai) = msg {
            return Ok(ai.content.clone());
        }
    }
    Err(AyasError::Chain(ChainError::Parse(
        "no AI message found in output".into(),
    )))
}

/// Strips markdown code block fences from a string.
/// Handles both ````json ... ```` and ```` ... ```` forms.
fn strip_code_blocks(s: &str) -> String {
    let trimmed = s.trim();
    if trimmed.starts_with("```") {
        // Remove opening fence (``` or ```json etc.)
        let after_open = if let Some(rest) = trimmed.strip_prefix("```") {
            // Skip the optional language tag on the first line
            rest.find('\n').map_or("", |i| &rest[i + 1..])
        } else {
            trimmed
        };
        // Remove closing fence
        let content = after_open
            .strip_suffix("```")
            .unwrap_or(after_open);
        content.trim().to_string()
    } else {
        trimmed.to_string()
    }
}

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
        extract_last_ai_content(&input)
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

/// Parses a `Vec<Message>` to extract the last AI message content as a `serde_json::Value`.
/// Strips markdown code block fences (````json ... ```` or ```` ... ````) before parsing.
pub struct JsonOutputParser;

#[async_trait]
impl Runnable for JsonOutputParser {
    type Input = Vec<Message>;
    type Output = serde_json::Value;

    async fn invoke(
        &self,
        input: Self::Input,
        _config: &RunnableConfig,
    ) -> Result<Self::Output> {
        let content = extract_last_ai_content(&input)?;
        let cleaned = strip_code_blocks(&content);
        serde_json::from_str(&cleaned).map_err(|e| {
            AyasError::Chain(ChainError::Parse(format!("JSON parse error: {e}")))
        })
    }
}

/// Parses a `Vec<Message>` to extract the last AI message content and deserialize it into `T`.
/// Strips markdown code block fences before parsing, just like `JsonOutputParser`.
pub struct StructuredOutputParser<T: DeserializeOwned + Send + Sync + 'static> {
    _phantom: PhantomData<T>,
}

impl<T: DeserializeOwned + Send + Sync + 'static> StructuredOutputParser<T> {
    pub fn new() -> Self {
        Self {
            _phantom: PhantomData,
        }
    }
}

impl<T: DeserializeOwned + Send + Sync + 'static> Default for StructuredOutputParser<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl<T: DeserializeOwned + Send + Sync + 'static> Runnable for StructuredOutputParser<T> {
    type Input = Vec<Message>;
    type Output = T;

    async fn invoke(
        &self,
        input: Self::Input,
        _config: &RunnableConfig,
    ) -> Result<Self::Output> {
        let content = extract_last_ai_content(&input)?;
        let cleaned = strip_code_blocks(&content);
        serde_json::from_str(&cleaned).map_err(|e| {
            AyasError::Chain(ChainError::Parse(format!(
                "structured parse error: {e}"
            )))
        })
    }
}

/// Parses a `Vec<Message>` to extract content matching a regex pattern from the last AI message.
/// Returns the first capture group if present, otherwise the full match.
pub struct RegexOutputParser {
    pattern: regex::Regex,
}

impl RegexOutputParser {
    pub fn new(pattern: &str) -> Result<Self> {
        let re = regex::Regex::new(pattern).map_err(|e| {
            AyasError::Chain(ChainError::Parse(format!("invalid regex: {e}")))
        })?;
        Ok(Self { pattern: re })
    }
}

#[async_trait]
impl Runnable for RegexOutputParser {
    type Input = Vec<Message>;
    type Output = String;

    async fn invoke(
        &self,
        input: Self::Input,
        _config: &RunnableConfig,
    ) -> Result<Self::Output> {
        let content = extract_last_ai_content(&input)?;
        let caps = self.pattern.captures(&content).ok_or_else(|| {
            AyasError::Chain(ChainError::Parse(format!(
                "regex pattern '{}' did not match",
                self.pattern
            )))
        })?;
        // Return the first capture group if it exists, otherwise the full match.
        let result = caps
            .get(1)
            .unwrap_or_else(|| caps.get(0).unwrap())
            .as_str()
            .to_string();
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

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

    // --- JsonOutputParser tests ---

    #[tokio::test]
    async fn json_output_parser_plain_json() {
        let parser = JsonOutputParser;
        let messages = vec![Message::ai(r#"{"name": "Alice", "age": 30}"#)];
        let config = RunnableConfig::default();
        let result = parser.invoke(messages, &config).await.unwrap();
        assert_eq!(result["name"], "Alice");
        assert_eq!(result["age"], 30);
    }

    #[tokio::test]
    async fn json_output_parser_with_json_code_block() {
        let parser = JsonOutputParser;
        let messages = vec![Message::ai(
            "```json\n{\"key\": \"value\"}\n```",
        )];
        let config = RunnableConfig::default();
        let result = parser.invoke(messages, &config).await.unwrap();
        assert_eq!(result["key"], "value");
    }

    #[tokio::test]
    async fn json_output_parser_with_bare_code_block() {
        let parser = JsonOutputParser;
        let messages = vec![Message::ai("```\n{\"foo\": 42}\n```")];
        let config = RunnableConfig::default();
        let result = parser.invoke(messages, &config).await.unwrap();
        assert_eq!(result["foo"], 42);
    }

    #[tokio::test]
    async fn json_output_parser_nested_json() {
        let parser = JsonOutputParser;
        let messages = vec![Message::ai(
            r#"{"outer": {"inner": [1, 2, 3]}, "flag": true}"#,
        )];
        let config = RunnableConfig::default();
        let result = parser.invoke(messages, &config).await.unwrap();
        assert_eq!(result["outer"]["inner"][1], 2);
        assert_eq!(result["flag"], true);
    }

    #[tokio::test]
    async fn json_output_parser_malformed_json() {
        let parser = JsonOutputParser;
        let messages = vec![Message::ai("{not valid json}")];
        let config = RunnableConfig::default();
        let result = parser.invoke(messages, &config).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("JSON parse error"));
    }

    #[tokio::test]
    async fn json_output_parser_no_ai_message() {
        let parser = JsonOutputParser;
        let messages = vec![Message::user("hello")];
        let config = RunnableConfig::default();
        let result = parser.invoke(messages, &config).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("no AI message"));
    }

    // --- StructuredOutputParser tests ---

    #[derive(Debug, Deserialize, PartialEq)]
    struct Person {
        name: String,
        age: u32,
    }

    #[tokio::test]
    async fn structured_output_parser_success() {
        let parser = StructuredOutputParser::<Person>::new();
        let messages = vec![Message::ai(r#"{"name": "Bob", "age": 25}"#)];
        let config = RunnableConfig::default();
        let result = parser.invoke(messages, &config).await.unwrap();
        assert_eq!(
            result,
            Person {
                name: "Bob".to_string(),
                age: 25,
            }
        );
    }

    #[tokio::test]
    async fn structured_output_parser_with_code_block() {
        let parser = StructuredOutputParser::<Person>::new();
        let messages =
            vec![Message::ai("```json\n{\"name\": \"Eve\", \"age\": 99}\n```")];
        let config = RunnableConfig::default();
        let result = parser.invoke(messages, &config).await.unwrap();
        assert_eq!(result.name, "Eve");
        assert_eq!(result.age, 99);
    }

    #[tokio::test]
    async fn structured_output_parser_missing_field() {
        let parser = StructuredOutputParser::<Person>::new();
        let messages = vec![Message::ai(r#"{"name": "Carol"}"#)];
        let config = RunnableConfig::default();
        let result = parser.invoke(messages, &config).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("structured parse error"));
    }

    // --- RegexOutputParser tests ---

    #[tokio::test]
    async fn regex_output_parser_capture_group() {
        let parser = RegexOutputParser::new(r"Answer:\s*(.+)").unwrap();
        let messages = vec![Message::ai("The final Answer: 42")];
        let config = RunnableConfig::default();
        let result = parser.invoke(messages, &config).await.unwrap();
        assert_eq!(result, "42");
    }

    #[tokio::test]
    async fn regex_output_parser_full_match() {
        let parser = RegexOutputParser::new(r"\d+\.\d+").unwrap();
        let messages = vec![Message::ai("The temperature is 36.5 degrees")];
        let config = RunnableConfig::default();
        let result = parser.invoke(messages, &config).await.unwrap();
        assert_eq!(result, "36.5");
    }

    #[tokio::test]
    async fn regex_output_parser_no_match() {
        let parser = RegexOutputParser::new(r"\d{4}-\d{2}-\d{2}").unwrap();
        let messages = vec![Message::ai("No date here")];
        let config = RunnableConfig::default();
        let result = parser.invoke(messages, &config).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("did not match"));
    }

    #[tokio::test]
    async fn regex_output_parser_invalid_regex() {
        let result = RegexOutputParser::new(r"[invalid");
        assert!(result.is_err());
    }
}
