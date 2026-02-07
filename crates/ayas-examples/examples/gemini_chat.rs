//! Gemini API integration example.
//!
//! Demonstrates using `ChatModel` trait with Google Gemini API,
//! chain composition with `PromptTemplate` → model → `StringOutputParser`,
//! and batch processing.
//!
//! ```bash
//! GEMINI_API_KEY=... cargo run --example gemini_chat
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
// Gemini API request/response types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct GeminiRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<GeminiContent>,
    contents: Vec<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_config: Option<GenerationConfig>,
}

#[derive(Serialize, Deserialize)]
struct GeminiContent {
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
    parts: Vec<GeminiPart>,
}

#[derive(Serialize, Deserialize)]
struct GeminiPart {
    text: String,
}

#[derive(Serialize)]
struct GenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop_sequences: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct GeminiResponse {
    candidates: Option<Vec<GeminiCandidate>>,
    #[serde(rename = "usageMetadata")]
    usage_metadata: Option<GeminiUsageMetadata>,
}

#[derive(Deserialize)]
struct GeminiCandidate {
    content: GeminiContent,
}

#[derive(Deserialize)]
struct GeminiUsageMetadata {
    #[serde(rename = "promptTokenCount", default)]
    prompt_token_count: u64,
    #[serde(rename = "candidatesTokenCount", default)]
    candidates_token_count: u64,
    #[serde(rename = "totalTokenCount", default)]
    total_token_count: u64,
}

// ---------------------------------------------------------------------------
// GeminiChatModel
// ---------------------------------------------------------------------------

struct GeminiChatModel {
    api_key: String,
    model_id: String,
    client: reqwest::Client,
}

impl GeminiChatModel {
    fn new(api_key: String, model_id: String) -> Self {
        Self {
            api_key,
            model_id,
            client: reqwest::Client::new(),
        }
    }

    fn build_request(&self, messages: &[Message], options: &CallOptions) -> GeminiRequest {
        let mut system_instruction: Option<GeminiContent> = None;
        let mut contents: Vec<GeminiContent> = Vec::new();

        for msg in messages {
            match msg {
                Message::System { content } => {
                    system_instruction = Some(GeminiContent {
                        role: None,
                        parts: vec![GeminiPart {
                            text: content.clone(),
                        }],
                    });
                }
                Message::User { content } => {
                    contents.push(GeminiContent {
                        role: Some("user".into()),
                        parts: vec![GeminiPart {
                            text: content.clone(),
                        }],
                    });
                }
                Message::AI(ai) => {
                    contents.push(GeminiContent {
                        role: Some("model".into()),
                        parts: vec![GeminiPart {
                            text: ai.content.clone(),
                        }],
                    });
                }
                Message::Tool { content, .. } => {
                    // Treat tool results as user messages for simplicity
                    contents.push(GeminiContent {
                        role: Some("user".into()),
                        parts: vec![GeminiPart {
                            text: content.clone(),
                        }],
                    });
                }
            }
        }

        let generation_config = if options.max_tokens.is_some()
            || options.temperature.is_some()
            || !options.stop.is_empty()
        {
            Some(GenerationConfig {
                max_output_tokens: options.max_tokens,
                temperature: options.temperature,
                stop_sequences: if options.stop.is_empty() {
                    None
                } else {
                    Some(options.stop.clone())
                },
            })
        } else {
            None
        };

        GeminiRequest {
            system_instruction,
            contents,
            generation_config,
        }
    }
}

#[async_trait]
impl ChatModel for GeminiChatModel {
    async fn generate(&self, messages: &[Message], options: &CallOptions) -> Result<ChatResult> {
        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
            self.model_id, self.api_key
        );

        let request_body = self.build_request(messages, options);

        let response = self
            .client
            .post(&url)
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
            return Err(AyasError::Model(if status.as_u16() == 401 || status.as_u16() == 403 {
                ModelError::Auth(body)
            } else if status.as_u16() == 429 {
                ModelError::RateLimited {
                    retry_after_secs: None,
                }
            } else {
                ModelError::ApiRequest(format!("HTTP {status}: {body}"))
            }));
        }

        let gemini_response: GeminiResponse = response
            .json()
            .await
            .map_err(|e| AyasError::Model(ModelError::InvalidResponse(e.to_string())))?;

        let text = gemini_response
            .candidates
            .as_ref()
            .and_then(|c| c.first())
            .and_then(|c| c.content.parts.first())
            .map(|p| p.text.clone())
            .unwrap_or_default();

        let usage = gemini_response.usage_metadata.map(|u| UsageMetadata {
            input_tokens: u.prompt_token_count,
            output_tokens: u.candidates_token_count,
            total_tokens: u.total_token_count,
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
    let api_key = std::env::var("GEMINI_API_KEY").expect("GEMINI_API_KEY environment variable required");
    let model_id = std::env::var("GEMINI_MODEL").unwrap_or_else(|_| "gemini-2.0-flash".into());

    let model = GeminiChatModel::new(api_key, model_id);

    // -----------------------------------------------------------------------
    // Demo 1: Basic generate() call
    // -----------------------------------------------------------------------
    println!("=== Demo 1: Basic generate() ===\n");

    let messages = vec![
        Message::system("You are a concise assistant. Answer in one sentence."),
        Message::user("What is Rust programming language?"),
    ];
    let options = CallOptions {
        temperature: Some(0.7),
        max_tokens: Some(256),
        ..Default::default()
    };

    let result = model.generate(&messages, &options).await?;
    println!("Response: {}", result.message.content());
    if let Some(usage) = &result.usage {
        println!(
            "Tokens: input={}, output={}, total={}",
            usage.input_tokens, usage.output_tokens, usage.total_tokens
        );
    }

    // -----------------------------------------------------------------------
    // Demo 2: Chain composition (PromptTemplate → Model → StringOutputParser)
    // -----------------------------------------------------------------------
    println!("\n=== Demo 2: Chain composition ===\n");

    let prompt = PromptTemplate::from_messages(vec![
        ("system", "You are a helpful assistant. Answer in one sentence."),
        ("user", "Explain {topic} in simple terms."),
    ]);

    let model_runnable = ChatModelRunnable::new(
        GeminiChatModel::new(
            std::env::var("GEMINI_API_KEY").unwrap(),
            std::env::var("GEMINI_MODEL").unwrap_or_else(|_| "gemini-2.0-flash".into()),
        ),
        CallOptions {
            temperature: Some(0.7),
            max_tokens: Some(256),
            ..Default::default()
        },
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
        GeminiChatModel::new(
            std::env::var("GEMINI_API_KEY").unwrap(),
            std::env::var("GEMINI_MODEL").unwrap_or_else(|_| "gemini-2.0-flash".into()),
        ),
        CallOptions {
            temperature: Some(0.7),
            max_tokens: Some(256),
            ..Default::default()
        },
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
