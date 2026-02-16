use std::sync::Arc;
use std::time::Duration;

use axum::response::sse::Event;
use axum::response::Sse;
use axum::{Json, Router, routing::post};
use futures::Stream;
use tokio::sync::mpsc;
use tracing::{info, warn};

use ayas_core::config::RunnableConfig;
use ayas_core::message::Message;
use ayas_core::model::{CallOptions, ChatModel, ResponseFormat};
use ayas_core::runnable::Runnable;
use ayas_deep_research::file_search::{FileSearchClient, GeminiFileSearchClient};
use ayas_deep_research::gemini::GeminiInteractionsClient;
use ayas_deep_research::runnable::{DeepResearchInput, DeepResearchRunnable};
use ayas_deep_research::types::ToolConfig;
use ayas_llm::gemini::GeminiChatModel;

use crate::error::AppError;
use crate::extractors::ApiKeys;
use crate::sse::{sse_done, sse_event, sse_response};

// Embed demo files at compile time
const NEEDS_MD: &str = include_str!("../../../../demo/needs.md");
const SEEDS_MD: &str = include_str!("../../../../demo/seeds.md");
const STEP1_PROMPT: &str = include_str!("../../../../demo/step1_prompt.md");
const STEP2_PROMPT: &str = include_str!("../../../../demo/step2_prompt.md");
const STEP3_PROMPT: &str = include_str!("../../../../demo/step3_prompt.md");

const FILE_SEARCH_POLL_INTERVAL: Duration = Duration::from_secs(3);

pub fn routes() -> Router {
    Router::new().route("/pipeline/hypothesis", post(pipeline_hypothesis))
}

#[derive(Debug, serde::Deserialize)]
pub struct PipelineRequest {
    #[serde(default = "default_mode")]
    pub mode: String, // "llm" | "manual"
    #[serde(default = "default_hypothesis_count")]
    pub hypothesis_count: u32,
    pub needs: Option<String>,
    pub seeds: Option<String>,
    pub hypotheses: Option<Vec<ManualHypothesis>>, // Manual mode
}

#[derive(Debug, serde::Deserialize)]
pub struct ManualHypothesis {
    pub title: String,
}

fn default_mode() -> String {
    "llm".to_string()
}

fn default_hypothesis_count() -> u32 {
    3
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum PipelineSseEvent {
    FileSearchSetup {
        status: String,
    },
    StepStart {
        step: u32,
        description: String,
    },
    StepComplete {
        step: u32,
        summary: String,
    },
    Hypothesis {
        index: u32,
        title: String,
        score: f64,
        physical_contradiction: String,
        cap_id_fingerprint: String,
        verdict_tag: String,
        verdict_reason: String,
    },
    Step3Start {
        index: u32,
        title: String,
    },
    Step3Complete {
        index: u32,
        title: String,
        text: String,
    },
    Step3Error {
        index: u32,
        title: String,
        message: String,
    },
    Complete {
        step1_text: String,
        hypotheses: serde_json::Value,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, serde::Deserialize)]
struct HypothesesOutput {
    hypotheses: Vec<HypothesisItem>,
}

#[derive(Debug, serde::Deserialize, Clone)]
struct HypothesisItem {
    title: String,
    physical_contradiction: String,
    cap_id_fingerprint: String,
    verdict_tag: String,
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

/// Send a pipeline SSE event via the channel.
async fn send_event(
    tx: &mpsc::Sender<Result<Event, std::convert::Infallible>>,
    event: &PipelineSseEvent,
) {
    let _ = tx.send(sse_event(event)).await;
}

/// Set up File Search Store: upload files, create store, import files, wait for indexing.
/// Returns (store_name, fs_client) on success.
async fn setup_file_search(
    api_key: &str,
    needs_text: &str,
    seeds_text: &str,
    tx: &mpsc::Sender<Result<Event, std::convert::Infallible>>,
) -> Result<(String, GeminiFileSearchClient), String> {
    let fs_client = GeminiFileSearchClient::new(api_key);

    // Upload files
    send_event(tx, &PipelineSseEvent::FileSearchSetup {
        status: "uploading_files".into(),
    })
    .await;

    let uploaded_needs = fs_client
        .upload_file("needs.md", "text/markdown", needs_text.as_bytes())
        .await
        .map_err(|e| format!("Failed to upload needs.md: {e}"))?;

    let uploaded_seeds = fs_client
        .upload_file("seeds.md", "text/markdown", seeds_text.as_bytes())
        .await
        .map_err(|e| format!("Failed to upload seeds.md: {e}"))?;

    info!(
        needs = %uploaded_needs.name,
        seeds = %uploaded_seeds.name,
        "Files uploaded"
    );

    // Create store
    send_event(tx, &PipelineSseEvent::FileSearchSetup {
        status: "creating_store".into(),
    })
    .await;

    let store = fs_client
        .create_store(&format!("pipeline-{}", uuid::Uuid::new_v4()))
        .await
        .map_err(|e| format!("Failed to create store: {e}"))?;

    info!(store = %store.name, "Store created");

    // Import files
    send_event(tx, &PipelineSseEvent::FileSearchSetup {
        status: "indexing".into(),
    })
    .await;

    let op1 = fs_client
        .import_file(&store.name, &uploaded_needs.name)
        .await
        .map_err(|e| format!("Failed to import needs.md: {e}"))?;

    let op2 = fs_client
        .import_file(&store.name, &uploaded_seeds.name)
        .await
        .map_err(|e| format!("Failed to import seeds.md: {e}"))?;

    // Wait for import operations to complete
    if !op1.done {
        wait_for_operation(&fs_client, &op1.name).await?;
    }
    if !op2.done {
        wait_for_operation(&fs_client, &op2.name).await?;
    }

    // Wait for store to finish indexing
    fs_client
        .wait_for_store_ready(&store.name, FILE_SEARCH_POLL_INTERVAL)
        .await
        .map_err(|e| format!("Store indexing failed: {e}"))?;

    send_event(tx, &PipelineSseEvent::FileSearchSetup {
        status: "ready".into(),
    })
    .await;

    info!(store = %store.name, "File Search Store ready");

    Ok((store.name, fs_client))
}

/// Poll an operation until done.
async fn wait_for_operation(
    client: &GeminiFileSearchClient,
    operation_name: &str,
) -> Result<(), String> {
    loop {
        let op = client
            .get_operation(operation_name)
            .await
            .map_err(|e| format!("Failed to poll operation {operation_name}: {e}"))?;

        if op.done {
            if let Some(err) = op.error {
                return Err(format!(
                    "Operation {operation_name} failed: {} (code {})",
                    err.message, err.code
                ));
            }
            return Ok(());
        }

        tokio::time::sleep(FILE_SEARCH_POLL_INTERVAL).await;
    }
}

async fn pipeline_hypothesis(
    api_keys: ApiKeys,
    Json(req): Json<PipelineRequest>,
) -> Result<Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>>, AppError> {
    let api_key = api_keys.get_key_for(&ayas_llm::provider::Provider::Gemini)?;
    let hypothesis_count = req.hypothesis_count;
    let mode = req.mode.clone();
    let needs_text = req.needs.unwrap_or_else(|| NEEDS_MD.to_string());
    let seeds_text = req.seeds.unwrap_or_else(|| SEEDS_MD.to_string());
    let manual_hypotheses = req.hypotheses;

    let (tx, rx) = mpsc::channel::<Result<Event, std::convert::Infallible>>(32);

    tokio::spawn(async move {
        run_pipeline(tx, api_key, hypothesis_count, mode, needs_text, seeds_text, manual_hypotheses).await;
    });

    let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
    Ok(sse_response(stream))
}

async fn run_pipeline(
    tx: mpsc::Sender<Result<Event, std::convert::Infallible>>,
    api_key: String,
    hypothesis_count: u32,
    mode: String,
    needs_text: String,
    seeds_text: String,
    manual_hypotheses: Option<Vec<ManualHypothesis>>,
) {
    info!(mode = %mode, hypothesis_count, "Pipeline started");

    // === Set up File Search Store ===
    let (store_name, fs_client) =
        match setup_file_search(&api_key, &needs_text, &seeds_text, &tx).await {
            Ok(result) => result,
            Err(msg) => {
                warn!(error = %msg, "File Search setup failed, falling back to inline text");
                // Fallback: run pipeline without File Search
                run_pipeline_inline(
                    tx,
                    api_key,
                    hypothesis_count,
                    mode,
                    needs_text,
                    seeds_text,
                    manual_hypotheses,
                )
                .await;
                return;
            }
        };

    let research_client = Arc::new(GeminiInteractionsClient::new(&api_key));

    if mode == "manual" {
        // === Manual mode: skip STEP 1 & 2, go straight to STEP 3 ===
        let titles: Vec<String> = manual_hypotheses
            .unwrap_or_default()
            .into_iter()
            .map(|h| h.title)
            .filter(|t| !t.trim().is_empty())
            .collect();

        if titles.is_empty() {
            send_event(&tx, &PipelineSseEvent::Error {
                message: "Manual mode requires at least one hypothesis title".into(),
            })
            .await;
            let _ = fs_client.delete_store(&store_name).await;
            let _ = tx.send(sse_done()).await;
            return;
        }

        // Emit hypotheses with score=0.0
        for (i, title) in titles.iter().enumerate() {
            send_event(&tx, &PipelineSseEvent::Hypothesis {
                index: i as u32,
                title: title.clone(),
                score: 0.0,
                physical_contradiction: String::new(),
                cap_id_fingerprint: String::new(),
                verdict_tag: String::new(),
                verdict_reason: String::new(),
            })
            .await;
        }

        // === STEP 3 ===
        send_event(&tx, &PipelineSseEvent::StepStart {
            step: 3,
            description: format!(
                "Deep Research x{}: 各仮説の深掘りレポートを並列実行中...",
                titles.len()
            ),
        })
        .await;

        let (step3_tx, mut step3_rx) = mpsc::channel::<(u32, String, Result<String, String>)>(16);

        for (i, title) in titles.iter().enumerate() {
            let tx_inner = tx.clone();
            let step3_tx = step3_tx.clone();
            let title = title.clone();
            let idx = i as u32;
            let prompt3 = STEP3_PROMPT.replace("{HYPOTHESIS_TITLE}", &title);
            let input3 = DeepResearchInput::new(&prompt3).with_tools(vec![
                ToolConfig::FileSearch {
                    file_search_store_names: vec![store_name.clone()],
                },
            ]);
            let research = DeepResearchRunnable::new(research_client.clone());
            let config = RunnableConfig::default();

            let _ = tx_inner
                .send(sse_event(&PipelineSseEvent::Step3Start {
                    index: idx,
                    title: title.clone(),
                }))
                .await;

            tokio::spawn(async move {
                info!(idx, title = %title, "STEP 3 Deep Research invoke start");
                let result = match research.invoke(input3, &config).await {
                    Ok(output) => {
                        info!(idx, "STEP 3 Deep Research invoke OK ({} chars)", output.text.len());
                        Ok(output.text)
                    }
                    Err(e) => {
                        warn!(idx, error = %e, "STEP 3 Deep Research invoke failed");
                        Err(e.to_string())
                    }
                };
                let _ = step3_tx.send((idx, title, result)).await;
            });
        }
        drop(step3_tx);

        let total = titles.len();
        let mut completed = 0u32;
        while let Some((idx, title, result)) = step3_rx.recv().await {
            completed += 1;
            info!(idx, completed, total, "STEP 3 result received");
            match result {
                Ok(text) => {
                    send_event(&tx, &PipelineSseEvent::Step3Complete {
                        index: idx,
                        title,
                        text,
                    })
                    .await;
                }
                Err(message) => {
                    send_event(&tx, &PipelineSseEvent::Step3Error {
                        index: idx,
                        title,
                        message,
                    })
                    .await;
                }
            }
            if completed as usize == total {
                break;
            }
        }

        info!(completed, "Manual mode pipeline complete");

        send_event(&tx, &PipelineSseEvent::StepComplete {
            step: 3,
            summary: format!("{}件の深掘りレポート完了", completed),
        })
        .await;

        send_event(&tx, &PipelineSseEvent::Complete {
            step1_text: String::new(),
            hypotheses: serde_json::Value::Null,
        })
        .await;

        // Cleanup store (best-effort)
        let _ = fs_client.delete_store(&store_name).await;
        let _ = tx.send(sse_done()).await;
        return;
    }

    // === LLM mode (default): STEP 1 → 2 → 3 ===

    // === STEP 1: Deep Research with File Search ===
    send_event(&tx, &PipelineSseEvent::StepStart {
        step: 1,
        description: "Deep Research: 仮説生成レポート作成中...".into(),
    })
    .await;

    let research = DeepResearchRunnable::new(research_client.clone());
    let config = RunnableConfig::default();

    let prompt1 = STEP1_PROMPT.replace("{HYPOTHESIS_COUNT}", &hypothesis_count.to_string());
    let input1 = DeepResearchInput::new(&prompt1).with_tools(vec![ToolConfig::FileSearch {
        file_search_store_names: vec![store_name.clone()],
    }]);

    let output1 = match research.invoke(input1, &config).await {
        Ok(o) => o,
        Err(e) => {
            send_event(&tx, &PipelineSseEvent::Error {
                message: format!("STEP 1 failed: {}", e),
            })
            .await;
            let _ = fs_client.delete_store(&store_name).await;
            let _ = tx.send(sse_done()).await;
            return;
        }
    };

    send_event(&tx, &PipelineSseEvent::StepComplete {
        step: 1,
        summary: format!("レポート生成完了 ({} chars)", output1.text.len()),
    })
    .await;

    // === STEP 2: Structured output extraction ===
    send_event(&tx, &PipelineSseEvent::StepStart {
        step: 2,
        description: "構造化出力: 仮説をJSON抽出中...".into(),
    })
    .await;

    let model = GeminiChatModel::new(api_key.clone(), "gemini-2.0-flash".into());
    let prompt2 = STEP2_PROMPT
        .replace("{HYPOTHESIS_COUNT}", &hypothesis_count.to_string())
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

    let result2 = match model.generate(&messages, &options).await {
        Ok(r) => r,
        Err(e) => {
            send_event(&tx, &PipelineSseEvent::Error {
                message: format!("STEP 2 failed: {}", e),
            })
            .await;
            let _ = fs_client.delete_store(&store_name).await;
            let _ = tx.send(sse_done()).await;
            return;
        }
    };

    let json_text = result2.message.content().to_string();
    let hypotheses: HypothesesOutput = match serde_json::from_str(&json_text) {
        Ok(h) => h,
        Err(e) => {
            send_event(&tx, &PipelineSseEvent::Error {
                message: format!("STEP 2 JSON parse failed: {}", e),
            })
            .await;
            let _ = fs_client.delete_store(&store_name).await;
            let _ = tx.send(sse_done()).await;
            return;
        }
    };

    let hypotheses_json: serde_json::Value =
        serde_json::from_str(&json_text).unwrap_or(serde_json::Value::Null);

    send_event(&tx, &PipelineSseEvent::StepComplete {
        step: 2,
        summary: format!("{}件の仮説を抽出", hypotheses.hypotheses.len()),
    })
    .await;

    for (i, h) in hypotheses.hypotheses.iter().enumerate() {
        send_event(&tx, &PipelineSseEvent::Hypothesis {
            index: i as u32,
            title: h.title.clone(),
            score: h.synthesis_score,
            physical_contradiction: h.physical_contradiction.clone(),
            cap_id_fingerprint: h.cap_id_fingerprint.clone(),
            verdict_tag: h.verdict_tag.clone(),
            verdict_reason: h.verdict_reason.clone(),
        })
        .await;
    }

    // === STEP 3: Independent parallel Deep Research per hypothesis with File Search ===
    send_event(&tx, &PipelineSseEvent::StepStart {
        step: 3,
        description: format!(
            "Deep Research x{}: 各仮説の深掘りレポートを並列実行中...",
            hypotheses.hypotheses.len()
        ),
    })
    .await;

    let (step3_tx, mut step3_rx) = mpsc::channel::<(u32, String, Result<String, String>)>(16);

    for (i, h) in hypotheses.hypotheses.iter().enumerate() {
        let tx_inner = tx.clone();
        let step3_tx = step3_tx.clone();
        let title = h.title.clone();
        let idx = i as u32;
        let prompt3 = STEP3_PROMPT.replace("{HYPOTHESIS_TITLE}", &title);

        // Use File Search tool instead of inline text attachments
        let input3 = DeepResearchInput::new(&prompt3).with_tools(vec![
            ToolConfig::FileSearch {
                file_search_store_names: vec![store_name.clone()],
            },
        ]);
        let research = DeepResearchRunnable::new(research_client.clone());
        let config = RunnableConfig::default();

        let _ = tx_inner
            .send(sse_event(&PipelineSseEvent::Step3Start {
                index: idx,
                title: title.clone(),
            }))
            .await;

        tokio::spawn(async move {
            let result = match research.invoke(input3, &config).await {
                Ok(output) => Ok(output.text),
                Err(e) => Err(e.to_string()),
            };
            let _ = step3_tx.send((idx, title, result)).await;
        });
    }
    drop(step3_tx);

    let total = hypotheses.hypotheses.len();
    let mut completed = 0u32;
    while let Some((idx, title, result)) = step3_rx.recv().await {
        completed += 1;
        match result {
            Ok(text) => {
                send_event(&tx, &PipelineSseEvent::Step3Complete {
                    index: idx,
                    title,
                    text,
                })
                .await;
            }
            Err(message) => {
                send_event(&tx, &PipelineSseEvent::Step3Error {
                    index: idx,
                    title,
                    message,
                })
                .await;
            }
        }
        if completed as usize == total {
            break;
        }
    }

    send_event(&tx, &PipelineSseEvent::StepComplete {
        step: 3,
        summary: format!("{}件の深掘りレポート完了", completed),
    })
    .await;

    send_event(&tx, &PipelineSseEvent::Complete {
        step1_text: output1.text,
        hypotheses: hypotheses_json,
    })
    .await;

    // Cleanup store (best-effort)
    let _ = fs_client.delete_store(&store_name).await;
    let _ = tx.send(sse_done()).await;
}

/// Fallback pipeline using inline text attachments (when File Search setup fails).
async fn run_pipeline_inline(
    tx: mpsc::Sender<Result<Event, std::convert::Infallible>>,
    api_key: String,
    hypothesis_count: u32,
    mode: String,
    needs_text: String,
    seeds_text: String,
    manual_hypotheses: Option<Vec<ManualHypothesis>>,
) {
    use ayas_core::message::ContentPart;

    let send = |event: &PipelineSseEvent| {
        let tx = tx.clone();
        let e = sse_event(event);
        async move {
            let _ = tx.send(e).await;
        }
    };

    let research_client = Arc::new(GeminiInteractionsClient::new(&api_key));

    if mode == "manual" {
        let titles: Vec<String> = manual_hypotheses
            .unwrap_or_default()
            .into_iter()
            .map(|h| h.title)
            .filter(|t| !t.trim().is_empty())
            .collect();

        if titles.is_empty() {
            send(&PipelineSseEvent::Error {
                message: "Manual mode requires at least one hypothesis title".into(),
            })
            .await;
            let _ = tx.send(sse_done()).await;
            return;
        }

        for (i, title) in titles.iter().enumerate() {
            send(&PipelineSseEvent::Hypothesis {
                index: i as u32,
                title: title.clone(),
                score: 0.0,
                physical_contradiction: String::new(),
                cap_id_fingerprint: String::new(),
                verdict_tag: String::new(),
                verdict_reason: String::new(),
            })
            .await;
        }

        send(&PipelineSseEvent::StepStart {
            step: 3,
            description: format!(
                "Deep Research x{}: 各仮説の深掘りレポートを並列実行中...",
                titles.len()
            ),
        })
        .await;

        let (step3_tx, mut step3_rx) = mpsc::channel::<(u32, String, Result<String, String>)>(16);

        for (i, title) in titles.iter().enumerate() {
            let tx_inner = tx.clone();
            let step3_tx = step3_tx.clone();
            let title = title.clone();
            let idx = i as u32;
            let prompt3 = STEP3_PROMPT.replace("{HYPOTHESIS_TITLE}", &title);
            let input3 = DeepResearchInput::new(&prompt3).with_attachments(vec![
                ContentPart::Text {
                    text: format!("=== technical_assets.json ===\n{}", seeds_text),
                },
                ContentPart::Text {
                    text: format!("=== target_specification.txt ===\n{}", needs_text),
                },
            ]);
            let research = DeepResearchRunnable::new(research_client.clone());
            let config = RunnableConfig::default();

            let _ = tx_inner
                .send(sse_event(&PipelineSseEvent::Step3Start {
                    index: idx,
                    title: title.clone(),
                }))
                .await;

            tokio::spawn(async move {
                let result = match research.invoke(input3, &config).await {
                    Ok(output) => Ok(output.text),
                    Err(e) => Err(e.to_string()),
                };
                let _ = step3_tx.send((idx, title, result)).await;
            });
        }
        drop(step3_tx);

        let total = titles.len();
        let mut completed = 0u32;
        while let Some((idx, title, result)) = step3_rx.recv().await {
            completed += 1;
            match result {
                Ok(text) => {
                    send(&PipelineSseEvent::Step3Complete {
                        index: idx,
                        title,
                        text,
                    })
                    .await;
                }
                Err(message) => {
                    send(&PipelineSseEvent::Step3Error {
                        index: idx,
                        title,
                        message,
                    })
                    .await;
                }
            }
            if completed as usize == total {
                break;
            }
        }

        send(&PipelineSseEvent::StepComplete {
            step: 3,
            summary: format!("{}件の深掘りレポート完了", completed),
        })
        .await;

        send(&PipelineSseEvent::Complete {
            step1_text: String::new(),
            hypotheses: serde_json::Value::Null,
        })
        .await;

        let _ = tx.send(sse_done()).await;
        return;
    }

    // LLM mode fallback with inline text

    send(&PipelineSseEvent::StepStart {
        step: 1,
        description: "Deep Research: 仮説生成レポート作成中...".into(),
    })
    .await;

    let research = DeepResearchRunnable::new(research_client.clone());
    let config = RunnableConfig::default();

    let prompt1 = STEP1_PROMPT.replace("{HYPOTHESIS_COUNT}", &hypothesis_count.to_string());
    let input1 = DeepResearchInput::new(&prompt1).with_attachments(vec![
        ContentPart::Text {
            text: format!("=== target_specification.txt ===\n{}", needs_text),
        },
        ContentPart::Text {
            text: format!("=== technical_assets.json ===\n{}", seeds_text),
        },
    ]);

    let output1 = match research.invoke(input1, &config).await {
        Ok(o) => o,
        Err(e) => {
            send(&PipelineSseEvent::Error {
                message: format!("STEP 1 failed: {}", e),
            })
            .await;
            let _ = tx.send(sse_done()).await;
            return;
        }
    };

    send(&PipelineSseEvent::StepComplete {
        step: 1,
        summary: format!("レポート生成完了 ({} chars)", output1.text.len()),
    })
    .await;

    send(&PipelineSseEvent::StepStart {
        step: 2,
        description: "構造化出力: 仮説をJSON抽出中...".into(),
    })
    .await;

    let model = GeminiChatModel::new(api_key.clone(), "gemini-2.0-flash".into());
    let prompt2 = STEP2_PROMPT
        .replace("{HYPOTHESIS_COUNT}", &hypothesis_count.to_string())
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

    let result2 = match model.generate(&messages, &options).await {
        Ok(r) => r,
        Err(e) => {
            send(&PipelineSseEvent::Error {
                message: format!("STEP 2 failed: {}", e),
            })
            .await;
            let _ = tx.send(sse_done()).await;
            return;
        }
    };

    let json_text = result2.message.content().to_string();
    let hypotheses: HypothesesOutput = match serde_json::from_str(&json_text) {
        Ok(h) => h,
        Err(e) => {
            send(&PipelineSseEvent::Error {
                message: format!("STEP 2 JSON parse failed: {}", e),
            })
            .await;
            let _ = tx.send(sse_done()).await;
            return;
        }
    };

    let hypotheses_json: serde_json::Value =
        serde_json::from_str(&json_text).unwrap_or(serde_json::Value::Null);

    send(&PipelineSseEvent::StepComplete {
        step: 2,
        summary: format!("{}件の仮説を抽出", hypotheses.hypotheses.len()),
    })
    .await;

    for (i, h) in hypotheses.hypotheses.iter().enumerate() {
        send(&PipelineSseEvent::Hypothesis {
            index: i as u32,
            title: h.title.clone(),
            score: h.synthesis_score,
            physical_contradiction: h.physical_contradiction.clone(),
            cap_id_fingerprint: h.cap_id_fingerprint.clone(),
            verdict_tag: h.verdict_tag.clone(),
            verdict_reason: h.verdict_reason.clone(),
        })
        .await;
    }

    send(&PipelineSseEvent::StepStart {
        step: 3,
        description: format!(
            "Deep Research x{}: 各仮説の深掘りレポートを並列実行中...",
            hypotheses.hypotheses.len()
        ),
    })
    .await;

    let (step3_tx, mut step3_rx) = mpsc::channel::<(u32, String, Result<String, String>)>(16);

    for (i, h) in hypotheses.hypotheses.iter().enumerate() {
        let tx_inner = tx.clone();
        let step3_tx = step3_tx.clone();
        let title = h.title.clone();
        let idx = i as u32;
        let prompt3 = STEP3_PROMPT.replace("{HYPOTHESIS_TITLE}", &title);
        let input3 = DeepResearchInput::new(&prompt3).with_attachments(vec![
            ContentPart::Text {
                text: format!("=== hypothesis_context ===\n{}", output1.text),
            },
            ContentPart::Text {
                text: format!("=== technical_assets.json ===\n{}", seeds_text),
            },
            ContentPart::Text {
                text: format!("=== target_specification.txt ===\n{}", needs_text),
            },
        ]);
        let research = DeepResearchRunnable::new(research_client.clone());
        let config = RunnableConfig::default();

        let _ = tx_inner
            .send(sse_event(&PipelineSseEvent::Step3Start {
                index: idx,
                title: title.clone(),
            }))
            .await;

        tokio::spawn(async move {
            let result = match research.invoke(input3, &config).await {
                Ok(output) => Ok(output.text),
                Err(e) => Err(e.to_string()),
            };
            let _ = step3_tx.send((idx, title, result)).await;
        });
    }
    drop(step3_tx);

    let total = hypotheses.hypotheses.len();
    let mut completed = 0u32;
    while let Some((idx, title, result)) = step3_rx.recv().await {
        completed += 1;
        match result {
            Ok(text) => {
                send(&PipelineSseEvent::Step3Complete {
                    index: idx,
                    title,
                    text,
                })
                .await;
            }
            Err(message) => {
                send(&PipelineSseEvent::Step3Error {
                    index: idx,
                    title,
                    message,
                })
                .await;
            }
        }
        if completed as usize == total {
            break;
        }
    }

    send(&PipelineSseEvent::StepComplete {
        step: 3,
        summary: format!("{}件の深掘りレポート完了", completed),
    })
    .await;

    send(&PipelineSseEvent::Complete {
        step1_text: output1.text,
        hypotheses: hypotheses_json,
    })
    .await;

    let _ = tx.send(sse_done()).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use tower::ServiceExt;

    fn app() -> Router {
        Router::new().nest("/api", routes())
    }

    #[tokio::test]
    async fn pipeline_missing_key() {
        unsafe {
            std::env::remove_var("GEMINI_API_KEY");
        }
        let app = app();
        let body = serde_json::json!({ "hypothesis_count": 3 });

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/pipeline/hypothesis")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn pipeline_invalid_json() {
        let app = app();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/pipeline/hypothesis")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("not json"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
