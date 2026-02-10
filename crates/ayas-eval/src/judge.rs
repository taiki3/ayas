use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

use ayas_core::error::Result;
use ayas_core::message::Message;
use ayas_core::model::{CallOptions, ChatModel};

use crate::dataset::Example;
use crate::evaluator::{EvalScore, Evaluator};

/// LLM-based evaluator that uses a ChatModel to judge outputs.
pub struct LlmJudge {
    model: Arc<dyn ChatModel>,
    criteria: String,
    metric_name: String,
}

impl LlmJudge {
    pub fn new(model: Arc<dyn ChatModel>, criteria: impl Into<String>) -> Self {
        Self {
            model,
            criteria: criteria.into(),
            metric_name: "llm_judge".into(),
        }
    }

    pub fn with_metric_name(mut self, name: impl Into<String>) -> Self {
        self.metric_name = name.into();
        self
    }
}

#[async_trait]
impl Evaluator for LlmJudge {
    fn name(&self) -> &str {
        &self.metric_name
    }

    async fn evaluate(&self, example: &Example, actual: &Value) -> Result<EvalScore> {
        let input_str = serde_json::to_string_pretty(&example.input).unwrap_or_default();
        let actual_str = serde_json::to_string_pretty(actual).unwrap_or_default();
        let expected_str = example
            .expected
            .as_ref()
            .map(|e| serde_json::to_string_pretty(e).unwrap_or_default())
            .unwrap_or_else(|| "N/A".into());

        let prompt = format!(
            "You are an expert evaluator. Score the following output on a scale of 0.0 to 1.0.\n\n\
            Criteria: {}\n\n\
            Input: {}\n\n\
            Expected output: {}\n\n\
            Actual output: {}\n\n\
            Respond with ONLY a JSON object: {{\"score\": <float>, \"explanation\": \"<reason>\"}}",
            self.criteria, input_str, expected_str, actual_str
        );

        let messages = vec![Message::user(prompt)];

        let result = self
            .model
            .generate(&messages, &CallOptions::default())
            .await?;
        let response_text = result.message.content();

        let (score, explanation) = parse_judge_response(response_text);

        Ok(EvalScore {
            value: score,
            metric: self.metric_name.clone(),
            explanation: Some(explanation),
        })
    }
}

/// Parse the judge's response to extract score and explanation.
fn parse_judge_response(text: &str) -> (f64, String) {
    // Try to parse as JSON
    if let Ok(val) = serde_json::from_str::<Value>(text) {
        let score = val
            .get("score")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0)
            .clamp(0.0, 1.0);
        let explanation = val
            .get("explanation")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        return (score, explanation);
    }
    // Fallback: try to find a number in the text
    for word in text.split_whitespace() {
        if let Ok(n) = word
            .trim_matches(|c: char| !c.is_ascii_digit() && c != '.')
            .parse::<f64>()
        {
            if (0.0..=1.0).contains(&n) {
                return (n, text.to_string());
            }
        }
    }
    (0.0, format!("Could not parse score from: {text}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ayas_core::message::AIContent;
    use ayas_core::model::ChatResult;
    use serde_json::json;

    // --- parse_judge_response tests ---

    #[test]
    fn parse_valid_json() {
        let (score, explanation) =
            parse_judge_response(r#"{"score": 0.85, "explanation": "Good answer"}"#);
        assert!((score - 0.85).abs() < 1e-10);
        assert_eq!(explanation, "Good answer");
    }

    #[test]
    fn parse_score_clamped() {
        let (score, _) = parse_judge_response(r#"{"score": 1.5, "explanation": "over"}"#);
        assert_eq!(score, 1.0);

        let (score, _) = parse_judge_response(r#"{"score": -0.5, "explanation": "under"}"#);
        assert_eq!(score, 0.0);
    }

    #[test]
    fn parse_plain_number() {
        let (score, explanation) = parse_judge_response("The score is 0.7 out of 1.0");
        assert!((score - 0.7).abs() < 1e-10);
        assert_eq!(explanation, "The score is 0.7 out of 1.0");
    }

    #[test]
    fn parse_unparseable() {
        let (score, explanation) = parse_judge_response("I cannot evaluate this");
        assert_eq!(score, 0.0);
        assert!(explanation.contains("Could not parse score"));
    }

    // --- MockChatModel for LlmJudge tests ---

    struct MockChatModel {
        response: String,
    }

    #[async_trait]
    impl ChatModel for MockChatModel {
        async fn generate(
            &self,
            _messages: &[Message],
            _options: &CallOptions,
        ) -> Result<ChatResult> {
            Ok(ChatResult {
                message: Message::AI(AIContent {
                    content: self.response.clone(),
                    tool_calls: Vec::new(),
                    usage: None,
                }),
                usage: None,
            })
        }

        fn model_name(&self) -> &str {
            "mock-judge"
        }
    }

    #[tokio::test]
    async fn llm_judge_evaluate() {
        let model = Arc::new(MockChatModel {
            response: r#"{"score": 0.9, "explanation": "Very relevant answer"}"#.into(),
        });
        let judge = LlmJudge::new(model, "relevance and accuracy");

        let example = Example {
            id: "test-1".into(),
            input: json!({"question": "What is Rust?"}),
            expected: Some(json!("A systems programming language")),
            metadata: Default::default(),
        };

        let score = judge
            .evaluate(&example, &json!("Rust is a systems programming language"))
            .await
            .unwrap();
        assert!((score.value - 0.9).abs() < 1e-10);
        assert_eq!(score.metric, "llm_judge");
        assert_eq!(
            score.explanation.as_deref(),
            Some("Very relevant answer")
        );
    }

    #[tokio::test]
    async fn llm_judge_custom_metric_name() {
        let model = Arc::new(MockChatModel {
            response: r#"{"score": 0.5, "explanation": "ok"}"#.into(),
        });
        let judge = LlmJudge::new(model, "helpfulness").with_metric_name("helpfulness");
        assert_eq!(judge.name(), "helpfulness");

        let example = Example {
            id: "test-2".into(),
            input: json!("test"),
            expected: None,
            metadata: Default::default(),
        };
        let score = judge.evaluate(&example, &json!("response")).await.unwrap();
        assert_eq!(score.metric, "helpfulness");
    }

    #[tokio::test]
    async fn llm_judge_unparseable_response() {
        let model = Arc::new(MockChatModel {
            response: "I think it's pretty good".into(),
        });
        let judge = LlmJudge::new(model, "quality");

        let example = Example {
            id: "test-3".into(),
            input: json!("test"),
            expected: None,
            metadata: Default::default(),
        };
        let score = judge.evaluate(&example, &json!("answer")).await.unwrap();
        // Falls back to 0.0 when no parseable score
        assert_eq!(score.value, 0.0);
        assert!(score.explanation.is_some());
    }
}
