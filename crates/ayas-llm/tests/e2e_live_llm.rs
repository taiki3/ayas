//! End-to-end tests with real LLM APIs.
//!
//! Verifies the full ayas framework pipeline against live provider endpoints.
//! Required environment variables:
//!   - `ANTHROPIC_API_KEY`  — Claude Sonnet 4.5
//!   - `OPENAI_API_KEY`     — GPT-5.3
//!   - `GEMINI_API_KEY`     — Gemini 3.0 Flash
//!
//! Run:
//!   cargo test -p ayas-llm --test e2e_live_llm -- --ignored --nocapture
//!
//! Each test exercises 5 phases:
//!   1. Basic chat generation
//!   2. Low-level tool calling (model → tool → model)
//!   3. Full agent graph loop (`create_tool_calling_agent`)
//!   4. Structured output chain (PromptTemplate → Model → StructuredOutputParser<T>)
//!   5. Multi-turn conversation with context retention

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use ayas_core::config::RunnableConfig;
use ayas_core::error::Result;
use ayas_core::message::Message;
use ayas_core::model::{CallOptions, ChatModel};
use ayas_core::runnable::{Runnable, RunnableExt};
use ayas_core::tool::{Tool, ToolDefinition};

use ayas_agent::tool_calling::create_tool_calling_agent;
use ayas_chain::parser::StructuredOutputParser;
use ayas_chain::prompt::PromptTemplate;
use ayas_llm::claude::ClaudeChatModel;
use ayas_llm::gemini::GeminiChatModel;
use ayas_llm::openai::OpenAIChatModel;
use ayas_llm::runnable::ChatModelRunnable;

// ---------------------------------------------------------------------------
// Tools — deterministic implementations for verifiable assertions
// ---------------------------------------------------------------------------

/// Returns a secret value the model cannot know without calling the tool.
struct SecretVaultTool;

#[async_trait]
impl Tool for SecretVaultTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "lookup_secret".into(),
            description: "Look up a secret value from the secure vault by its key.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "key": {
                        "type": "string",
                        "description": "The secret key to look up"
                    }
                },
                "required": ["key"]
            }),
        }
    }

    async fn call(&self, _input: Value) -> Result<String> {
        Ok("AYAS-2026-DELTA-42".to_string())
    }
}

/// Deterministic addition — lets us verify the result exactly.
struct AddTool;

#[async_trait]
impl Tool for AddTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "add".into(),
            description: "Add two numbers and return the sum.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "a": { "type": "number", "description": "First number" },
                    "b": { "type": "number", "description": "Second number" }
                },
                "required": ["a", "b"]
            }),
        }
    }

    async fn call(&self, input: Value) -> Result<String> {
        let a = input["a"].as_f64().unwrap_or(0.0);
        let b = input["b"].as_f64().unwrap_or(0.0);
        Ok(format!("{}", a + b))
    }
}

// ---------------------------------------------------------------------------
// Structured output target
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CountryInfo {
    name: String,
    capital: String,
    continent: String,
}

// ---------------------------------------------------------------------------
// Core test runner — all 5 phases, parameterised by model factory
// ---------------------------------------------------------------------------

async fn run_essential_e2e<M, F>(create_model: F, provider_name: &str)
where
    M: ChatModel + 'static,
    F: Fn() -> M,
{
    let config = RunnableConfig::default();

    // ── Phase 1: Basic Chat ──────────────────────────────────────────────
    println!("[{}] Phase 1: Basic Chat", provider_name);
    {
        let model = create_model();
        let messages = vec![Message::user(
            "What is the capital of France? Answer with just the city name.",
        )];
        let result = model
            .generate(&messages, &CallOptions::default())
            .await
            .unwrap_or_else(|e| panic!("[{}] Phase 1: generate failed: {}", provider_name, e));

        let content = result.message.content().to_string();
        assert!(
            !content.is_empty(),
            "[{}] Phase 1: empty response",
            provider_name
        );
        assert!(
            content.to_lowercase().contains("paris"),
            "[{}] Phase 1: expected 'Paris' in response, got: {}",
            provider_name,
            content
        );
        println!("  OK: {}", content.trim());
    }

    // ── Phase 2: Tool Calling (low-level) ────────────────────────────────
    println!("[{}] Phase 2: Tool Calling", provider_name);
    {
        let model = create_model();
        let messages = vec![Message::user(
            "Look up the secret with key 'project-x'. \
             You MUST use the lookup_secret tool. Do not guess.",
        )];
        let options = CallOptions {
            tools: vec![SecretVaultTool.definition()],
            ..Default::default()
        };

        let result = model
            .generate(&messages, &options)
            .await
            .unwrap_or_else(|e| panic!("[{}] Phase 2: generate failed: {}", provider_name, e));

        match &result.message {
            Message::AI(ai) => {
                assert!(
                    !ai.tool_calls.is_empty(),
                    "[{}] Phase 2: expected tool_calls, got content only: {}",
                    provider_name,
                    ai.content
                );
                assert_eq!(
                    ai.tool_calls[0].name, "lookup_secret",
                    "[{}] Phase 2: wrong tool: {}",
                    provider_name,
                    ai.tool_calls[0].name
                );
                println!(
                    "  Tool call: {}({})",
                    ai.tool_calls[0].name, ai.tool_calls[0].arguments
                );

                // Execute tool, feed result back
                let tool_result = SecretVaultTool
                    .call(ai.tool_calls[0].arguments.clone())
                    .await
                    .unwrap();

                let follow_up = vec![
                    messages[0].clone(),
                    result.message.clone(),
                    Message::tool(&tool_result, &ai.tool_calls[0].id),
                ];
                let follow_up_options = CallOptions {
                    tools: vec![SecretVaultTool.definition()],
                    ..Default::default()
                };

                let final_result = model
                    .generate(&follow_up, &follow_up_options)
                    .await
                    .unwrap_or_else(|e| {
                        panic!("[{}] Phase 2: follow-up generate failed: {}", provider_name, e)
                    });

                let final_content = final_result.message.content().to_string();
                assert!(
                    final_content.contains("AYAS-2026-DELTA-42"),
                    "[{}] Phase 2: secret not found in response: {}",
                    provider_name,
                    final_content
                );
                println!("  OK: response contains secret value");
            }
            other => panic!(
                "[{}] Phase 2: expected AI message, got: {:?}",
                provider_name, other
            ),
        }
    }

    // ── Phase 3: Full Agent Graph Loop ───────────────────────────────────
    println!("[{}] Phase 3: Full Agent Graph Loop", provider_name);
    {
        let model_arc: Arc<dyn ChatModel> = Arc::new(create_model());
        let tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(SecretVaultTool),
            Arc::new(AddTool),
        ];

        let graph = create_tool_calling_agent(
            model_arc,
            tools,
            Some("You are a helpful assistant. Always use tools when asked to look something up.".into()),
        )
        .unwrap_or_else(|e| panic!("[{}] Phase 3: graph compile failed: {}", provider_name, e));

        let input = json!({
            "messages": [{
                "type": "user",
                "content": "Look up the secret with key 'alpha'. Report the exact value you find."
            }]
        });

        let result = graph
            .invoke(input, &config)
            .await
            .unwrap_or_else(|e| panic!("[{}] Phase 3: graph invoke failed: {}", provider_name, e));

        let messages = result["messages"]
            .as_array()
            .unwrap_or_else(|| panic!("[{}] Phase 3: messages not an array", provider_name));

        // Full tool loop: user → AI(tool_call) → tool_result → AI(final) = 4+
        assert!(
            messages.len() >= 4,
            "[{}] Phase 3: expected >=4 messages (full tool loop), got {}: {:#?}",
            provider_name,
            messages.len(),
            messages
        );

        let final_content = messages
            .last()
            .and_then(|m| m["content"].as_str())
            .unwrap_or("");
        assert!(
            final_content.contains("AYAS-2026-DELTA-42"),
            "[{}] Phase 3: final response should contain secret, got: {}",
            provider_name,
            final_content
        );
        println!(
            "  OK: agent loop completed ({} messages), secret in final response",
            messages.len()
        );
    }

    // ── Phase 4: Structured Output Chain ─────────────────────────────────
    println!("[{}] Phase 4: Structured Output Chain", provider_name);
    {
        let model = create_model();
        let model_runnable = ChatModelRunnable::new(
            model,
            CallOptions {
                temperature: Some(0.0),
                ..Default::default()
            },
        );

        let prompt = PromptTemplate::from_messages(vec![
            (
                "system",
                "You are a structured data API. Respond with ONLY a valid JSON object. \
                 No markdown, no explanation, no code blocks. Just the raw JSON.",
            ),
            (
                "user",
                "Return a JSON object about {country} with exactly these keys: \
                 \"name\" (full country name as a string), \
                 \"capital\" (capital city as a string), \
                 \"continent\" (continent name as a string).",
            ),
        ]);

        let parser = StructuredOutputParser::<CountryInfo>::new();
        let chain = prompt.pipe(model_runnable).pipe(parser);

        let mut vars = HashMap::new();
        vars.insert("country".into(), "Japan".into());

        let result = chain
            .invoke(vars, &config)
            .await
            .unwrap_or_else(|e| panic!("[{}] Phase 4: chain invoke failed: {}", provider_name, e));

        assert!(
            result.name.to_lowercase().contains("japan"),
            "[{}] Phase 4: name should contain 'japan', got: {}",
            provider_name,
            result.name
        );
        assert!(
            result.capital.to_lowercase().contains("tokyo"),
            "[{}] Phase 4: capital should be 'Tokyo', got: {}",
            provider_name,
            result.capital
        );
        assert!(
            result.continent.to_lowercase().contains("asia"),
            "[{}] Phase 4: continent should contain 'asia', got: {}",
            provider_name,
            result.continent
        );
        println!(
            "  OK: {{ name: {:?}, capital: {:?}, continent: {:?} }}",
            result.name, result.capital, result.continent
        );
    }

    // ── Phase 5: Multi-turn Conversation ─────────────────────────────────
    println!("[{}] Phase 5: Multi-turn Conversation", provider_name);
    {
        let model = create_model();
        let options = CallOptions::default();

        // Turn 1: establish context
        let turn1_messages = vec![Message::user(
            "My favorite color is chartreuse. Please remember that.",
        )];
        let turn1_result = model
            .generate(&turn1_messages, &options)
            .await
            .unwrap_or_else(|e| panic!("[{}] Phase 5: turn 1 failed: {}", provider_name, e));
        println!("  Turn 1: {}", turn1_result.message.content());

        // Turn 2: verify retention
        let turn2_messages = vec![
            turn1_messages[0].clone(),
            turn1_result.message.clone(),
            Message::user("What is my favorite color? Answer with just the color name."),
        ];
        let turn2_result = model
            .generate(&turn2_messages, &options)
            .await
            .unwrap_or_else(|e| panic!("[{}] Phase 5: turn 2 failed: {}", provider_name, e));

        let turn2_content = turn2_result.message.content().to_string().to_lowercase();
        assert!(
            turn2_content.contains("chartreuse"),
            "[{}] Phase 5: expected 'chartreuse' in follow-up, got: {}",
            provider_name,
            turn2_content
        );
        println!("  OK: context retained across turns");
    }

    println!("[{}] All 5 phases passed\n", provider_name);
}

// ---------------------------------------------------------------------------
// Provider-specific test entry points
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn e2e_anthropic_sonnet_4_5() {
    let key = std::env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY required");
    run_essential_e2e(
        move || ClaudeChatModel::new(key.clone(), "claude-sonnet-4-5-20250929".into()),
        "Claude Sonnet 4.5",
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn e2e_openai_gpt_5_2() {
    let key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY required");
    run_essential_e2e(
        move || OpenAIChatModel::new(key.clone(), "gpt-5.2".into()),
        "OpenAI GPT-5.2",
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn e2e_gemini_3_flash_preview() {
    let key = std::env::var("GEMINI_API_KEY").expect("GEMINI_API_KEY required");
    run_essential_e2e(
        move || GeminiChatModel::new(key.clone(), "gemini-3-flash-preview".into()),
        "Gemini 3 Flash Preview",
    )
    .await;
}
