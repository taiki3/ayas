use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use ayas_core::error::Result;

use crate::dataset::Example;

/// Score from an evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalScore {
    /// Score value, typically 0.0 to 1.0.
    pub value: f64,
    /// Name of the metric.
    pub metric: String,
    /// Optional explanation.
    #[serde(default)]
    pub explanation: Option<String>,
}

/// Result of evaluating a single example.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalResult {
    /// The example ID.
    pub example_id: String,
    /// The actual output from the system.
    pub actual_output: Value,
    /// Scores from evaluators.
    pub scores: Vec<EvalScore>,
    /// Latency in milliseconds.
    pub latency_ms: u64,
}

/// Trait for evaluators.
#[async_trait]
pub trait Evaluator: Send + Sync {
    /// Name of this evaluator.
    fn name(&self) -> &str;
    /// Evaluate the actual output against the example.
    async fn evaluate(&self, example: &Example, actual: &Value) -> Result<EvalScore>;
}

/// Exact string match evaluator.
pub struct ExactMatchEvaluator;

#[async_trait]
impl Evaluator for ExactMatchEvaluator {
    fn name(&self) -> &str {
        "exact_match"
    }

    async fn evaluate(&self, example: &Example, actual: &Value) -> Result<EvalScore> {
        let score = match &example.expected {
            Some(expected) => {
                if expected == actual {
                    1.0
                } else {
                    0.0
                }
            }
            None => 0.0,
        };
        Ok(EvalScore {
            value: score,
            metric: "exact_match".into(),
            explanation: if score == 1.0 {
                Some("Exact match".into())
            } else {
                Some("No match".into())
            },
        })
    }
}

/// Contains evaluator — checks if actual output contains expected value as substring.
pub struct ContainsEvaluator;

#[async_trait]
impl Evaluator for ContainsEvaluator {
    fn name(&self) -> &str {
        "contains"
    }

    async fn evaluate(&self, example: &Example, actual: &Value) -> Result<EvalScore> {
        let actual_str = match actual {
            Value::String(s) => s.clone(),
            other => serde_json::to_string(other).unwrap_or_default(),
        };
        let expected_str = match &example.expected {
            Some(Value::String(s)) => s.clone(),
            Some(other) => serde_json::to_string(other).unwrap_or_default(),
            None => {
                return Ok(EvalScore {
                    value: 0.0,
                    metric: "contains".into(),
                    explanation: Some("No expected value".into()),
                })
            }
        };
        let score = if actual_str.contains(&expected_str) {
            1.0
        } else {
            0.0
        };
        Ok(EvalScore {
            value: score,
            metric: "contains".into(),
            explanation: Some(format!(
                "actual {} expected",
                if score == 1.0 {
                    "contains"
                } else {
                    "does not contain"
                }
            )),
        })
    }
}

/// JSON key presence evaluator — checks if actual output has expected keys.
pub struct JsonKeyEvaluator {
    pub required_keys: Vec<String>,
}

#[async_trait]
impl Evaluator for JsonKeyEvaluator {
    fn name(&self) -> &str {
        "json_keys"
    }

    async fn evaluate(&self, _example: &Example, actual: &Value) -> Result<EvalScore> {
        if let Value::Object(map) = actual {
            let found = self
                .required_keys
                .iter()
                .filter(|k| map.contains_key(*k))
                .count();
            let total = self.required_keys.len();
            let score = if total == 0 {
                1.0
            } else {
                found as f64 / total as f64
            };
            Ok(EvalScore {
                value: score,
                metric: "json_keys".into(),
                explanation: Some(format!("{found}/{total} keys present")),
            })
        } else {
            Ok(EvalScore {
                value: 0.0,
                metric: "json_keys".into(),
                explanation: Some("Not a JSON object".into()),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn example_with_expected(expected: Option<Value>) -> Example {
        Example {
            id: "test".into(),
            input: json!({"q": "test"}),
            expected,
            metadata: Default::default(),
        }
    }

    // --- ExactMatchEvaluator tests ---

    #[tokio::test]
    async fn exact_match_success() {
        let eval = ExactMatchEvaluator;
        let ex = example_with_expected(Some(json!("hello")));
        let score = eval.evaluate(&ex, &json!("hello")).await.unwrap();
        assert_eq!(score.value, 1.0);
        assert_eq!(score.metric, "exact_match");
    }

    #[tokio::test]
    async fn exact_match_failure() {
        let eval = ExactMatchEvaluator;
        let ex = example_with_expected(Some(json!("hello")));
        let score = eval.evaluate(&ex, &json!("world")).await.unwrap();
        assert_eq!(score.value, 0.0);
    }

    #[tokio::test]
    async fn exact_match_no_expected() {
        let eval = ExactMatchEvaluator;
        let ex = example_with_expected(None);
        let score = eval.evaluate(&ex, &json!("anything")).await.unwrap();
        assert_eq!(score.value, 0.0);
    }

    // --- ContainsEvaluator tests ---

    #[tokio::test]
    async fn contains_present() {
        let eval = ContainsEvaluator;
        let ex = example_with_expected(Some(json!("world")));
        let score = eval.evaluate(&ex, &json!("hello world")).await.unwrap();
        assert_eq!(score.value, 1.0);
        assert_eq!(score.metric, "contains");
    }

    #[tokio::test]
    async fn contains_absent() {
        let eval = ContainsEvaluator;
        let ex = example_with_expected(Some(json!("xyz")));
        let score = eval.evaluate(&ex, &json!("hello world")).await.unwrap();
        assert_eq!(score.value, 0.0);
    }

    #[tokio::test]
    async fn contains_no_expected() {
        let eval = ContainsEvaluator;
        let ex = example_with_expected(None);
        let score = eval.evaluate(&ex, &json!("anything")).await.unwrap();
        assert_eq!(score.value, 0.0);
        assert_eq!(
            score.explanation.as_deref(),
            Some("No expected value")
        );
    }

    // --- JsonKeyEvaluator tests ---

    #[tokio::test]
    async fn json_keys_all_present() {
        let eval = JsonKeyEvaluator {
            required_keys: vec!["name".into(), "age".into()],
        };
        let ex = example_with_expected(None);
        let actual = json!({"name": "Alice", "age": 30, "extra": true});
        let score = eval.evaluate(&ex, &actual).await.unwrap();
        assert_eq!(score.value, 1.0);
        assert_eq!(score.metric, "json_keys");
    }

    #[tokio::test]
    async fn json_keys_partial() {
        let eval = JsonKeyEvaluator {
            required_keys: vec!["name".into(), "age".into(), "email".into()],
        };
        let ex = example_with_expected(None);
        let actual = json!({"name": "Alice"});
        let score = eval.evaluate(&ex, &actual).await.unwrap();
        assert!((score.value - 1.0 / 3.0).abs() < 1e-10);
    }

    #[tokio::test]
    async fn json_keys_non_object() {
        let eval = JsonKeyEvaluator {
            required_keys: vec!["name".into()],
        };
        let ex = example_with_expected(None);
        let score = eval.evaluate(&ex, &json!("not an object")).await.unwrap();
        assert_eq!(score.value, 0.0);
        assert_eq!(
            score.explanation.as_deref(),
            Some("Not a JSON object")
        );
    }

    #[tokio::test]
    async fn json_keys_empty_required() {
        let eval = JsonKeyEvaluator {
            required_keys: vec![],
        };
        let ex = example_with_expected(None);
        let score = eval.evaluate(&ex, &json!({})).await.unwrap();
        assert_eq!(score.value, 1.0);
    }
}
