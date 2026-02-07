//! OpenAI API integration example.
//!
//! Demonstrates using `ChatModel` trait with OpenAI Chat Completions API,
//! chain composition with `PromptTemplate` -> model -> `StringOutputParser`,
//! and batch processing.
//!
//! ```bash
//! OPENAI_API_KEY=... cargo run --example openai_chat -p ayas-examples
//! ```

use std::collections::HashMap;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use ayas_chain::parser::StringOutputParser;
use ayas_chain::prompt::PromptTemplate;
use ayas_core::config::RunnableConfig;
use ayas_core::error::{AyasError, ModelError, Result};
use ayas_core::message::{AIContent, Message, UsageMetadata};
use ayas_core::model::{CallOptions, ChatModel, ChatResult};
use ayas_core::runnable::{Runnable, RunnableExt};

// ---------------------------------------------------------------------------
// OpenAI Chat Completions API request/response types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct OpenAIRequest {
    model: String,
    messages: Vec<OpenAIMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize)]
struct OpenAIMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct OpenAIResponse {
    choices: Vec<OpenAIChoice>,
    usage: Option<OpenAIUsage>,
}

#[derive(Deserialize)]
struct OpenAIChoice {
    message: OpenAIMessage,
}

#[derive(Deserialize)]
struct OpenAIUsage {
    prompt_tokens: u64,
    completion_tokens: u64,
    total_tokens: u64,
}

#[derive(Deserialize)]
struct OpenAIError {
    error: OpenAIErrorDetail,
}

#[derive(Deserialize)]
struct OpenAIErrorDetail {
    message: String,
}

// ---------------------------------------------------------------------------
// OpenAIChatModel
// ---------------------------------------------------------------------------

struct OpenAIChatModel {
    api_key: String,
    model_id: String,
    client: reqwest::Client,
}

impl OpenAIChatModel {
    fn new(api_key: String, model_id: String) -> Self {
        Self {
            api_key,
            model_id,
            client: reqwest::Client::new(),
        }
    }

    fn build_request(&self, messages: &[Message], options: &CallOptions) -> OpenAIRequest {
        let api_messages: Vec<OpenAIMessage> = messages
            .iter()
            .map(|msg| match msg {
                Message::System { content } => OpenAIMessage {
                    role: "system".into(),
                    content: content.clone(),
                },
                Message::User { content } => OpenAIMessage {
                    role: "user".into(),
                    content: content.clone(),
                },
                Message::AI(ai) => OpenAIMessage {
                    role: "assistant".into(),
                    content: ai.content.clone(),
                },
                Message::Tool { content, .. } => OpenAIMessage {
                    role: "user".into(),
                    content: content.clone(),
                },
            })
            .collect();

        OpenAIRequest {
            model: self.model_id.clone(),
            messages: api_messages,
            max_tokens: options.max_tokens,
            temperature: options.temperature,
            stop: if options.stop.is_empty() {
                None
            } else {
                Some(options.stop.clone())
            },
        }
    }
}

#[async_trait]
impl ChatModel for OpenAIChatModel {
    async fn generate(&self, messages: &[Message], options: &CallOptions) -> Result<ChatResult> {
        let request_body = self.build_request(messages, options);

        let response = self
            .client
            .post("https://api.openai.com/v1/chat/completions")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&request_body)
            .send()
            .await
            .map_err(|e| AyasError::Model(ModelError::ApiRequest(e.to_string())))?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "failed to read response body".into());
            let error_msg = serde_json::from_str::<OpenAIError>(&body)
                .map(|e| e.error.message)
                .unwrap_or(body);
            return Err(AyasError::Model(match status.as_u16() {
                401 => ModelError::Auth(error_msg),
                429 => ModelError::RateLimited {
                    retry_after_secs: None,
                },
                _ => ModelError::ApiRequest(format!("HTTP {status}: {error_msg}")),
            }));
        }

        let api_response: OpenAIResponse = response
            .json()
            .await
            .map_err(|e| AyasError::Model(ModelError::InvalidResponse(e.to_string())))?;

        let text = api_response
            .choices
            .first()
            .map(|c| c.message.content.clone())
            .unwrap_or_default();

        let usage = api_response.usage.map(|u| UsageMetadata {
            input_tokens: u.prompt_tokens,
            output_tokens: u.completion_tokens,
            total_tokens: u.total_tokens,
        });

        Ok(ChatResult {
            message: Message::AI(AIContent {
                content: text,
                tool_calls: Vec::new(),
                usage: usage.clone(),
            }),
            usage,
        })
    }

    fn model_name(&self) -> &str {
        &self.model_id
    }
}

// ---------------------------------------------------------------------------
// ChatModelRunnable – adapts ChatModel to Runnable trait
// ---------------------------------------------------------------------------

struct ChatModelRunnable<M: ChatModel> {
    model: M,
    options: CallOptions,
}

impl<M: ChatModel> ChatModelRunnable<M> {
    fn new(model: M, options: CallOptions) -> Self {
        Self { model, options }
    }
}

#[async_trait]
impl<M: ChatModel + 'static> Runnable for ChatModelRunnable<M> {
    type Input = Vec<Message>;
    type Output = Vec<Message>;

    async fn invoke(&self, input: Self::Input, _config: &RunnableConfig) -> Result<Self::Output> {
        let result = self.model.generate(&input, &self.options).await?;
        let mut messages = input;
        messages.push(result.message);
        Ok(messages)
    }
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let api_key =
        std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY environment variable required");
    let model_id = std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into());

    let default_options = CallOptions {
        temperature: Some(0.7),
        max_tokens: Some(256),
        ..Default::default()
    };

    // -----------------------------------------------------------------------
    // Demo 1: Basic generate() call
    // -----------------------------------------------------------------------
    println!("=== Demo 1: Basic generate() ===\n");

    let model = OpenAIChatModel::new(api_key.clone(), model_id.clone());
    let messages = vec![
        Message::system("You are a concise assistant. Answer in one sentence."),
        Message::user("What is Rust programming language?"),
    ];

    let result = model.generate(&messages, &default_options).await?;
    println!("Response: {}", result.message.content());
    if let Some(usage) = &result.usage {
        println!(
            "Tokens: input={}, output={}, total={}",
            usage.input_tokens, usage.output_tokens, usage.total_tokens
        );
    }

    // -----------------------------------------------------------------------
    // Demo 2: Chain composition (PromptTemplate -> Model -> StringOutputParser)
    // -----------------------------------------------------------------------
    println!("\n=== Demo 2: Chain composition ===\n");

    let prompt = PromptTemplate::from_messages(vec![
        ("system", "You are a helpful assistant. Answer in one sentence."),
        ("user", "Explain {topic} in simple terms."),
    ]);

    let model_runnable = ChatModelRunnable::new(
        OpenAIChatModel::new(api_key.clone(), model_id.clone()),
        default_options.clone(),
    );

    let chain = prompt.pipe(model_runnable).pipe(StringOutputParser);
    let config = RunnableConfig::default();

    let mut vars = HashMap::new();
    vars.insert("topic".into(), "ownership in Rust".into());

    let answer = chain.invoke(vars, &config).await?;
    println!("Chain output: {answer}");

    // -----------------------------------------------------------------------
    // Demo 3: Batch processing
    // -----------------------------------------------------------------------
    println!("\n=== Demo 3: Batch processing ===\n");

    let prompt = PromptTemplate::from_messages(vec![
        ("system", "You are a helpful assistant. Answer in one sentence."),
        ("user", "What is {topic}?"),
    ]);

    let model_runnable = ChatModelRunnable::new(
        OpenAIChatModel::new(api_key, model_id),
        default_options,
    );

    let chain = prompt.pipe(model_runnable).pipe(StringOutputParser);

    let topics = ["borrow checker", "trait objects", "async/await"];
    let inputs: Vec<HashMap<String, String>> = topics
        .iter()
        .map(|t| {
            let mut m = HashMap::new();
            m.insert("topic".into(), t.to_string());
            m
        })
        .collect();

    let results = chain.batch(inputs, &config).await?;
    for (topic, result) in topics.iter().zip(results.iter()) {
        println!("  {topic}: {result}");
    }

    println!("\nAll demos completed successfully!");
    Ok(())
}
