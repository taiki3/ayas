//! Hypothesis generation pipeline (3-step Deep Research + Structured Output).
//!
//! STEP 1: Deep Research with needs.md + seeds.md → hypothesis report
//! STEP 2: Structured output extraction → JSON hypotheses
//! STEP 3: Parallel Deep Research per hypothesis → detailed reports
//!
//! Usage: GEMINI_API_KEY=xxx cargo run --example hypothesis_pipeline
//!
//! TODO: File Search対応 — 現在はプロンプト内のファイル参照（target_specification.txt /
//!       technical_assets.json / hypothesis_context）をインラインテキスト添付で代替している。
//!       Gemini File Search（ベクトルストア）APIが利用可能になったら、ファイルアップロード→
//!       ベクトルストア作成→File Search tool設定に置き換えること。

use std::sync::Arc;

use ayas_core::config::RunnableConfig;
use ayas_core::message::{ContentPart, Message};
use ayas_core::model::{CallOptions, ChatModel, ResponseFormat};
use ayas_core::runnable::Runnable;
use ayas_deep_research::gemini::GeminiInteractionsClient;
use ayas_deep_research::runnable::{DeepResearchInput, DeepResearchRunnable};
use ayas_llm::gemini::GeminiChatModel;
use serde::Deserialize;

const HYPOTHESIS_COUNT: u32 = 3;

#[derive(Debug, Deserialize)]
struct HypothesesOutput {
    hypotheses: Vec<Hypothesis>,
}

#[derive(Debug, Deserialize)]
struct Hypothesis {
    title: String,
    #[allow(dead_code)]
    physical_contradiction: String,
    #[allow(dead_code)]
    cap_id_fingerprint: String,
    #[allow(dead_code)]
    verdict_tag: String,
    #[allow(dead_code)]
    verdict_reason: String,
    synthesis_score: f64,
}

fn hypothesis_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "hypotheses": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "title": { "type": "string" },
                        "physical_contradiction": { "type": "string" },
                        "cap_id_fingerprint": { "type": "string" },
                        "verdict_tag": { "type": "string" },
                        "verdict_reason": { "type": "string" },
                        "synthesis_score": { "type": "number" }
                    },
                    "required": [
                        "title", "physical_contradiction", "cap_id_fingerprint",
                        "verdict_tag", "verdict_reason", "synthesis_score"
                    ]
                }
            }
        },
        "required": ["hypotheses"]
    })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let api_key = std::env::var("GEMINI_API_KEY")
        .map_err(|_| "GEMINI_API_KEY environment variable is required")?;

    // Load demo files
    let needs = std::fs::read_to_string("demo/needs.md")?;
    let seeds = std::fs::read_to_string("demo/seeds.md")?;
    let step1_prompt = std::fs::read_to_string("demo/step1_prompt.md")?;
    let step2_prompt = std::fs::read_to_string("demo/step2_prompt.md")?;
    let step3_prompt = std::fs::read_to_string("demo/step3_prompt.md")?;

    let client = Arc::new(GeminiInteractionsClient::new(&api_key));
    let research = DeepResearchRunnable::new(client);
    let config = RunnableConfig::default();

    // === STEP 1: Deep Research — generate hypothesis report ===
    println!("=== STEP 1: Deep Research (hypothesis generation) ===");

    let prompt1 = step1_prompt.replace("{HYPOTHESIS_COUNT}", &HYPOTHESIS_COUNT.to_string());
    let input1 = DeepResearchInput::new(&prompt1).with_attachments(vec![
        ContentPart::Text {
            text: format!("=== target_specification.txt ===\n{}", needs),
        },
        ContentPart::Text {
            text: format!("=== technical_assets.json ===\n{}", seeds),
        },
    ]);

    let output1 = research.invoke(input1, &config).await?;
    println!(
        "STEP 1 complete: {} chars, interaction_id={}",
        output1.text.len(),
        output1.interaction_id
    );

    // === STEP 2: Structured output — extract hypotheses as JSON ===
    println!("\n=== STEP 2: Structured Output (hypothesis extraction) ===");

    let model = GeminiChatModel::new(api_key.clone(), "gemini-2.0-flash".into());
    let prompt2 = step2_prompt
        .replace("{HYPOTHESIS_COUNT}", &HYPOTHESIS_COUNT.to_string())
        .replace("{STEP21_OUTPUT}", &output1.text);

    let messages = vec![Message::user(prompt2.as_str())];
    let options = CallOptions {
        response_format: Some(ResponseFormat::JsonSchema {
            name: "hypotheses".into(),
            schema: hypothesis_schema(),
            strict: true,
        }),
        ..Default::default()
    };

    let result2 = model.generate(&messages, &options).await?;
    let json_text = result2.message.content();
    let hypotheses: HypothesesOutput = serde_json::from_str(json_text)?;

    println!("STEP 2 complete: extracted {} hypotheses", hypotheses.hypotheses.len());
    for (i, h) in hypotheses.hypotheses.iter().enumerate() {
        println!(
            "  [{}] {} (score: {:.2})",
            i + 1,
            h.title,
            h.synthesis_score
        );
    }

    // === STEP 3: Parallel Deep Research — deep dive per hypothesis ===
    println!("\n=== STEP 3: Deep Research x{} (parallel deep dive) ===", hypotheses.hypotheses.len());

    let futures: Vec<_> = hypotheses
        .hypotheses
        .iter()
        .map(|h| {
            let prompt3 = step3_prompt.replace("{HYPOTHESIS_TITLE}", &h.title);
            let input3 = DeepResearchInput::new(&prompt3).with_attachments(vec![
                ContentPart::Text {
                    text: format!("=== hypothesis_context ===\n{}", output1.text),
                },
                ContentPart::Text {
                    text: format!("=== technical_assets.json ===\n{}", seeds),
                },
                ContentPart::Text {
                    text: format!("=== target_specification.txt ===\n{}", needs),
                },
            ]);
            research.invoke(input3, &config)
        })
        .collect();

    let results3 = futures::future::join_all(futures).await;

    println!("\n=== Results ===");
    for (i, result) in results3.iter().enumerate() {
        match result {
            Ok(output) => {
                println!(
                    "\n--- Hypothesis {} ---\nTitle: {}\nReport length: {} chars\n",
                    i + 1,
                    hypotheses.hypotheses[i].title,
                    output.text.len()
                );
                // Print first 500 chars as preview
                let preview: String = output.text.chars().take(500).collect();
                println!("{}", preview);
                if output.text.len() > 500 {
                    println!("...(truncated)");
                }
            }
            Err(e) => {
                println!("\n--- Hypothesis {} FAILED ---\nError: {}", i + 1, e);
            }
        }
    }

    println!("\nPipeline complete.");
    Ok(())
}
