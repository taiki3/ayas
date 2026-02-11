use std::sync::Arc;

use axum::response::sse::Event;
use axum::response::Sse;
use axum::{Json, Router, routing::post};
use futures::Stream;
use tokio::sync::mpsc;

use ayas_core::config::RunnableConfig;
use ayas_core::message::{ContentPart, Message};
use ayas_core::model::{CallOptions, ChatModel, ResponseFormat};
use ayas_core::runnable::Runnable;
use ayas_deep_research::gemini::GeminiInteractionsClient;
use ayas_deep_research::runnable::{DeepResearchInput, DeepResearchRunnable};
use ayas_llm::gemini::GeminiChatModel;

use crate::error::AppError;
use crate::extractors::ApiKeys;
use crate::sse::{sse_done, sse_event};

// Embed demo files at compile time
const NEEDS_MD: &str = include_str!("../../../../demo/needs.md");
const SEEDS_MD: &str = include_str!("../../../../demo/seeds.md");
const STEP1_PROMPT: &str = include_str!("../../../../demo/step1_prompt.md");
const STEP2_PROMPT: &str = include_str!("../../../../demo/step2_prompt.md");
const STEP3_PROMPT: &str = include_str!("../../../../demo/step3_prompt.md");

pub fn routes() -> Router {
    Router::new().route("/pipeline/hypothesis", post(pipeline_hypothesis))
}

#[derive(Debug, serde::Deserialize)]
pub struct PipelineRequest {
    #[serde(default = "default_hypothesis_count")]
    pub hypothesis_count: u32,
}

fn default_hypothesis_count() -> u32 {
    3
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum PipelineSseEvent {
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
    },
    Complete {
        step1_text: String,
        hypotheses: serde_json::Value,
        step3_results: Vec<Step3Result>,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct Step3Result {
    title: String,
    text: String,
}

#[derive(Debug, serde::Deserialize)]
struct HypothesesOutput {
    hypotheses: Vec<HypothesisItem>,
}

#[derive(Debug, serde::Deserialize, Clone)]
struct HypothesisItem {
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

async fn pipeline_hypothesis(
    api_keys: ApiKeys,
    Json(req): Json<PipelineRequest>,
) -> Result<Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>>, AppError> {
    let api_key = api_keys.get_key_for(&ayas_llm::provider::Provider::Gemini)?;
    let hypothesis_count = req.hypothesis_count;

    let (tx, rx) = mpsc::channel::<Result<Event, std::convert::Infallible>>(32);

    tokio::spawn(async move {
        run_pipeline(tx, api_key, hypothesis_count).await;
    });

    let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
    Ok(Sse::new(stream))
}

async fn run_pipeline(
    tx: mpsc::Sender<Result<Event, std::convert::Infallible>>,
    api_key: String,
    hypothesis_count: u32,
) {
    let send = |event: &PipelineSseEvent| {
        let tx = tx.clone();
        let e = sse_event(event);
        async move {
            let _ = tx.send(e).await;
        }
    };

    // === STEP 1: Deep Research ===
    send(&PipelineSseEvent::StepStart {
        step: 1,
        description: "Deep Research: 仮説生成レポート作成中...".into(),
    })
    .await;

    let client = Arc::new(GeminiInteractionsClient::new(&api_key));
    let research = DeepResearchRunnable::new(client);
    let config = RunnableConfig::default();

    let prompt1 = STEP1_PROMPT.replace("{HYPOTHESIS_COUNT}", &hypothesis_count.to_string());
    let input1 = DeepResearchInput::new(&prompt1).with_attachments(vec![
        ContentPart::Text {
            text: format!("=== target_specification.txt ===\n{}", NEEDS_MD),
        },
        ContentPart::Text {
            text: format!("=== technical_assets.json ===\n{}", SEEDS_MD),
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

    // === STEP 2: Structured output extraction ===
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
        })
        .await;
    }

    // === STEP 3: Parallel Deep Research per hypothesis ===
    send(&PipelineSseEvent::StepStart {
        step: 3,
        description: format!(
            "Deep Research x{}: 各仮説の深掘りレポート作成中...",
            hypotheses.hypotheses.len()
        ),
    })
    .await;

    let futures: Vec<_> = hypotheses
        .hypotheses
        .iter()
        .map(|h| {
            let prompt3 = STEP3_PROMPT.replace("{HYPOTHESIS_TITLE}", &h.title);
            let input3 = DeepResearchInput::new(&prompt3).with_attachments(vec![
                ContentPart::Text {
                    text: format!("=== hypothesis_context ===\n{}", output1.text),
                },
                ContentPart::Text {
                    text: format!("=== technical_assets.json ===\n{}", SEEDS_MD),
                },
                ContentPart::Text {
                    text: format!("=== target_specification.txt ===\n{}", NEEDS_MD),
                },
            ]);
            research.invoke(input3, &config)
        })
        .collect();

    let results3 = futures::future::join_all(futures).await;

    let mut step3_results = Vec::new();
    for (i, result) in results3.into_iter().enumerate() {
        let title = hypotheses
            .hypotheses
            .get(i)
            .map(|h| h.title.clone())
            .unwrap_or_default();
        match result {
            Ok(output) => {
                step3_results.push(Step3Result {
                    title,
                    text: output.text,
                });
            }
            Err(e) => {
                step3_results.push(Step3Result {
                    title,
                    text: format!("Error: {}", e),
                });
            }
        }
    }

    send(&PipelineSseEvent::StepComplete {
        step: 3,
        summary: format!("{}件の深掘りレポート完了", step3_results.len()),
    })
    .await;

    send(&PipelineSseEvent::Complete {
        step1_text: output1.text,
        hypotheses: hypotheses_json,
        step3_results,
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
