//! End-to-end ADL + Gemini example.
//!
//! 1. Gemini に ADL YAML パイプライン定義を生成させる
//! 2. その YAML をパースしてグラフを構築する
//! 3. パイプラインを実行して結果を得る
//!
//! ```bash
//! GEMINI_API_KEY=... cargo run --example adl_gemini
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use ayas_adl::prelude::*;
use ayas_adl::registry::NodeFactory;
use ayas_core::config::RunnableConfig;
use ayas_core::error::{AyasError, ModelError};
use ayas_core::runnable::Runnable;
use ayas_graph::prelude::NodeFn;

// ---------------------------------------------------------------------------
// Gemini API types (minimal)
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
}

#[derive(Deserialize)]
struct GeminiResponse {
    candidates: Option<Vec<GeminiCandidate>>,
}

#[derive(Deserialize)]
struct GeminiCandidate {
    content: GeminiContent,
}

// ---------------------------------------------------------------------------
// Gemini helper
// ---------------------------------------------------------------------------

async fn gemini_generate(
    client: &reqwest::Client,
    api_key: &str,
    model: &str,
    system: &str,
    user_prompt: &str,
    temperature: f64,
    max_tokens: u32,
) -> Result<String, Box<dyn std::error::Error>> {
    let url = format!(
        "https://generativelanguage.googleapis.com/v1alpha/models/{model}:generateContent?key={api_key}"
    );

    let request = GeminiRequest {
        system_instruction: Some(GeminiContent {
            role: None,
            parts: vec![GeminiPart {
                text: system.to_string(),
            }],
        }),
        contents: vec![GeminiContent {
            role: Some("user".into()),
            parts: vec![GeminiPart {
                text: user_prompt.to_string(),
            }],
        }],
        generation_config: Some(GenerationConfig {
            max_output_tokens: Some(max_tokens),
            temperature: Some(temperature),
        }),
    };

    let response = client.post(&url).json(&request).send().await?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Gemini API error: HTTP {status}: {body}").into());
    }

    let gemini_resp: GeminiResponse = response.json().await?;
    let text = gemini_resp
        .candidates
        .as_ref()
        .and_then(|c| c.first())
        .and_then(|c| c.content.parts.first())
        .map(|p| p.text.clone())
        .unwrap_or_default();

    Ok(text)
}

// ---------------------------------------------------------------------------
// LLM node factory for ADL
// ---------------------------------------------------------------------------

fn llm_node_factory(api_key: String, model: String) -> NodeFactory {
    Arc::new(move |node_id: &str, config: &HashMap<String, Value>| {
        let api_key = api_key.clone();
        let model = model.clone();
        let system_prompt = config
            .get("system_prompt")
            .and_then(|v| v.as_str())
            .unwrap_or("You are a helpful assistant.")
            .to_string();
        let input_key = config
            .get("input_key")
            .and_then(|v| v.as_str())
            .unwrap_or("input")
            .to_string();
        let output_key = config
            .get("output_key")
            .and_then(|v| v.as_str())
            .unwrap_or("output")
            .to_string();
        let temperature = config
            .get("temperature")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.7);

        Ok(NodeFn::new(
            node_id.to_string(),
            move |state: Value, _config: RunnableConfig| {
                let api_key = api_key.clone();
                let model = model.clone();
                let system_prompt = system_prompt.clone();
                let input_key = input_key.clone();
                let output_key = output_key.clone();
                async move {
                    let user_input = state
                        .get(&input_key)
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    let client = reqwest::Client::new();
                    let result = gemini_generate(
                        &client,
                        &api_key,
                        &model,
                        &system_prompt,
                        &user_input,
                        temperature,
                        1024,
                    )
                    .await
                    .map_err(|e| AyasError::Model(ModelError::ApiRequest(e.to_string())))?;

                    let mut output = serde_json::Map::new();
                    output.insert(output_key, Value::String(result));
                    Ok(Value::Object(output))
                }
            },
        ))
    })
}

// ---------------------------------------------------------------------------
// YAML extraction helper
// ---------------------------------------------------------------------------

fn extract_yaml(text: &str) -> &str {
    // Try to extract from ```yaml ... ``` block
    if let Some(start) = text.find("```yaml") {
        let content_start = start + 7; // len of "```yaml"
        if let Some(end) = text[content_start..].find("```") {
            return text[content_start..content_start + end].trim();
        }
    }
    if let Some(start) = text.find("```") {
        let content_start = start + 3;
        if let Some(end) = text[content_start..].find("```") {
            return text[content_start..content_start + end].trim();
        }
    }
    text.trim()
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let api_key =
        std::env::var("GEMINI_API_KEY").expect("GEMINI_API_KEY environment variable required");
    let model = std::env::var("GEMINI_MODEL")
        .unwrap_or_else(|_| "gemini-3-flash-preview".into());

    let client = reqwest::Client::new();

    // =========================================================================
    // Step 1: Gemini に ADL YAML パイプライン定義を生成させる
    // =========================================================================
    println!("=== Step 1: Gemini にパイプライン YAML を生成させる ===\n");
    println!("Model: {model}");

    let system = r#"You are an expert at writing ADL (Agent Design Language) pipeline definitions in YAML format.

ADL format specification:
- version: "1.0" (required)
- channels: list of state channels
  - name: channel name (state key)
  - type: "last_value" or "append"
  - default: default value (for last_value)
- nodes: list of processing nodes
  - id: unique node identifier
  - type: one of "passthrough", "transform", or "llm"
  - config: node-specific configuration
    - For "llm" nodes: system_prompt, input_key, output_key, temperature
    - For "transform" nodes: mapping (object mapping output_key to input_key)
- edges: list of edges connecting nodes
  - from/to: node ids or "__start__"/"__end__" sentinels
  - type: "static" (default) or "conditional"
  - conditions: (for conditional edges) list of {expression, to}

Rules:
- Every channel used by any node must be declared
- There must be an edge from __start__ to the first node
- There must be an edge from the last node to __end__
- Output ONLY the YAML in a ```yaml``` code block, nothing else"#;

    let user_prompt = r#"Create an ADL pipeline that:
1. Takes a "topic" as input
2. Uses an LLM node to generate a brief explanation of that topic (max 2 sentences)
3. Uses another LLM node to translate the explanation into Japanese
4. Outputs the final Japanese translation

Use channels: "topic" (last_value, the input), "explanation" (last_value, intermediate), "translation" (last_value, final output).
The first LLM reads from "topic" and writes to "explanation". The second LLM reads from "explanation" and writes to "translation"."#;

    let yaml_response = gemini_generate(
        &client, &api_key, &model, system, user_prompt, 0.3, 2048,
    )
    .await?;

    let generated_yaml = extract_yaml(&yaml_response);
    println!("--- Generated YAML ---");
    println!("{generated_yaml}");
    println!("--- End YAML ---\n");

    // Validate that it parses as YAML
    let _doc: serde_yaml::Value = serde_yaml::from_str(generated_yaml)
        .map_err(|e| format!("Generated YAML parse error: {e}"))?;
    println!("YAML parse: OK\n");

    // =========================================================================
    // Step 2: ADL Builder でグラフを構築
    // =========================================================================
    println!("=== Step 2: ADL Builder でグラフを構築 ===\n");

    let mut registry = ComponentRegistry::with_builtins();
    registry.register("llm", llm_node_factory(api_key.clone(), model.clone()));

    let builder = AdlBuilder::new(registry);
    let compiled = builder.build_from_yaml(generated_yaml)?;

    println!("Graph compiled successfully!");
    println!("  Entry point: {}", compiled.entry_point());
    println!("  Nodes: {:?}", compiled.node_names());
    println!("  Finish points: {:?}", compiled.finish_points());
    println!();

    // =========================================================================
    // Step 3: パイプラインを実行
    // =========================================================================
    println!("=== Step 3: パイプラインを実行 ===\n");

    let input = json!({
        "topic": "Rust's ownership system",
        "explanation": "",
        "translation": ""
    });

    println!("Input: {}", serde_json::to_string_pretty(&input)?);
    println!("\nExecuting pipeline...\n");

    let config = RunnableConfig::default();
    let result = compiled.invoke(input, &config).await?;

    println!("=== Result ===\n");
    println!(
        "Topic: {}",
        result["topic"].as_str().unwrap_or("(none)")
    );
    println!(
        "\nExplanation (EN):\n  {}",
        result["explanation"].as_str().unwrap_or("(none)")
    );
    println!(
        "\nTranslation (JP):\n  {}",
        result["translation"].as_str().unwrap_or("(none)")
    );

    println!("\n=== All steps completed successfully! ===");
    Ok(())
}
