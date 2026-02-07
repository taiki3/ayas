use std::collections::HashMap;

use async_trait::async_trait;

use ayas_core::config::RunnableConfig;
use ayas_core::error::{AyasError, ChainError, Result};
use ayas_core::message::Message;
use ayas_core::runnable::Runnable;

/// A template that formats messages by substituting `{variable}` placeholders.
pub struct PromptTemplate {
    /// Message templates with `{variable}` placeholders.
    templates: Vec<MessageTemplate>,
}

/// A single message template.
enum MessageTemplate {
    System(String),
    User(String),
    AI(String),
}

impl PromptTemplate {
    /// Create a simple user prompt template.
    pub fn from_template(template: &str) -> Self {
        Self {
            templates: vec![MessageTemplate::User(template.to_string())],
        }
    }

    /// Create a prompt with a system message and a user message template.
    pub fn from_messages(messages: Vec<(&str, &str)>) -> Self {
        let templates = messages
            .into_iter()
            .map(|(role, content)| match role {
                "system" => MessageTemplate::System(content.to_string()),
                "user" | "human" => MessageTemplate::User(content.to_string()),
                "ai" | "assistant" => MessageTemplate::AI(content.to_string()),
                _ => MessageTemplate::User(content.to_string()),
            })
            .collect();
        Self { templates }
    }
}

fn substitute(template: &str, variables: &HashMap<String, String>) -> Result<String> {
    let mut result = String::with_capacity(template.len());
    let mut chars = template.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '{' {
            let mut var_name = String::new();
            let mut found_close = false;
            for next_ch in chars.by_ref() {
                if next_ch == '}' {
                    found_close = true;
                    break;
                }
                var_name.push(next_ch);
            }
            if !found_close {
                return Err(AyasError::Chain(ChainError::Template(
                    "unclosed '{' in template".into(),
                )));
            }
            let value = variables.get(&var_name).ok_or_else(|| {
                AyasError::Chain(ChainError::MissingVariable(var_name.clone()))
            })?;
            result.push_str(value);
        } else {
            result.push(ch);
        }
    }

    Ok(result)
}

#[async_trait]
impl Runnable for PromptTemplate {
    type Input = HashMap<String, String>;
    type Output = Vec<Message>;

    async fn invoke(
        &self,
        input: Self::Input,
        _config: &RunnableConfig,
    ) -> Result<Self::Output> {
        let mut messages = Vec::with_capacity(self.templates.len());
        for template in &self.templates {
            let msg = match template {
                MessageTemplate::System(t) => Message::system(substitute(t, &input)?),
                MessageTemplate::User(t) => Message::user(substitute(t, &input)?),
                MessageTemplate::AI(t) => Message::ai(substitute(t, &input)?),
            };
            messages.push(msg);
        }
        Ok(messages)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn simple_template() {
        let prompt = PromptTemplate::from_template("Hello, {name}!");
        let mut vars = HashMap::new();
        vars.insert("name".into(), "world".into());

        let config = RunnableConfig::default();
        let messages = prompt.invoke(vars, &config).await.unwrap();

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content(), "Hello, world!");
    }

    #[tokio::test]
    async fn multi_message_template() {
        let prompt = PromptTemplate::from_messages(vec![
            ("system", "You are a {role}."),
            ("user", "Tell me about {topic}."),
        ]);
        let mut vars = HashMap::new();
        vars.insert("role".into(), "helpful assistant".into());
        vars.insert("topic".into(), "Rust".into());

        let config = RunnableConfig::default();
        let messages = prompt.invoke(vars, &config).await.unwrap();

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].content(), "You are a helpful assistant.");
        assert_eq!(messages[1].content(), "Tell me about Rust.");
    }

    #[tokio::test]
    async fn missing_variable_error() {
        let prompt = PromptTemplate::from_template("Hello, {name}!");
        let vars = HashMap::new();

        let config = RunnableConfig::default();
        let result = prompt.invoke(vars, &config).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(
            err,
            AyasError::Chain(ChainError::MissingVariable(_))
        ));
    }

    #[tokio::test]
    async fn unclosed_brace_error() {
        let prompt = PromptTemplate::from_template("Hello, {name");
        let mut vars = HashMap::new();
        vars.insert("name".into(), "world".into());

        let config = RunnableConfig::default();
        let result = prompt.invoke(vars, &config).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, AyasError::Chain(ChainError::Template(_))));
    }

    #[tokio::test]
    async fn multiple_variables() {
        let prompt = PromptTemplate::from_template("{greeting}, {name}! You are {age} years old.");
        let mut vars = HashMap::new();
        vars.insert("greeting".into(), "Hi".into());
        vars.insert("name".into(), "Alice".into());
        vars.insert("age".into(), "30".into());

        let config = RunnableConfig::default();
        let messages = prompt.invoke(vars, &config).await.unwrap();
        assert_eq!(messages[0].content(), "Hi, Alice! You are 30 years old.");
    }

    #[tokio::test]
    async fn no_variables_template() {
        let prompt = PromptTemplate::from_template("Hello, world!");
        let vars = HashMap::new();

        let config = RunnableConfig::default();
        let messages = prompt.invoke(vars, &config).await.unwrap();
        assert_eq!(messages[0].content(), "Hello, world!");
    }
}
