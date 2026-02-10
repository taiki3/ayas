use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use ayas_core::config::RunnableConfig;
use ayas_core::error::Result;
use ayas_core::runnable::Runnable;

use crate::dataset::Dataset;
use crate::evaluator::{EvalResult, Evaluator};

/// Summary report of an evaluation run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalReport {
    pub dataset_name: String,
    pub total_examples: usize,
    pub results: Vec<EvalResult>,
    pub aggregate_scores: std::collections::HashMap<String, f64>,
    pub mean_latency_ms: f64,
}

/// Runs evaluation of a Runnable against a Dataset.
pub struct EvalRunner {
    evaluators: Vec<Box<dyn Evaluator>>,
}

impl EvalRunner {
    pub fn new() -> Self {
        Self {
            evaluators: Vec::new(),
        }
    }

    pub fn add_evaluator(mut self, eval: impl Evaluator + 'static) -> Self {
        self.evaluators.push(Box::new(eval));
        self
    }

    /// Run evaluation: invoke the runnable for each example, then evaluate.
    pub async fn run<R: Runnable<Input = Value, Output = Value>>(
        &self,
        runnable: &R,
        dataset: &Dataset,
        config: &RunnableConfig,
    ) -> Result<EvalReport> {
        let mut results = Vec::new();

        for example in &dataset.examples {
            let start = Instant::now();
            let actual = runnable.invoke(example.input.clone(), config).await?;
            let latency = start.elapsed().as_millis() as u64;

            let mut scores = Vec::new();
            for evaluator in &self.evaluators {
                let score = evaluator.evaluate(example, &actual).await?;
                scores.push(score);
            }

            results.push(EvalResult {
                example_id: example.id.clone(),
                actual_output: actual,
                scores,
                latency_ms: latency,
            });
        }

        // Compute aggregates
        let mut aggregate_scores = std::collections::HashMap::new();
        for evaluator in &self.evaluators {
            let name = evaluator.name();
            let sum: f64 = results
                .iter()
                .flat_map(|r| r.scores.iter())
                .filter(|s| s.metric == name)
                .map(|s| s.value)
                .sum();
            let count = results
                .iter()
                .flat_map(|r| r.scores.iter())
                .filter(|s| s.metric == name)
                .count();
            if count > 0 {
                aggregate_scores.insert(name.to_string(), sum / count as f64);
            }
        }

        let mean_latency = if results.is_empty() {
            0.0
        } else {
            results.iter().map(|r| r.latency_ms as f64).sum::<f64>() / results.len() as f64
        };

        Ok(EvalReport {
            dataset_name: dataset.name.clone(),
            total_examples: dataset.examples.len(),
            results,
            aggregate_scores,
            mean_latency_ms: mean_latency,
        })
    }
}

impl Default for EvalRunner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dataset::Example;
    use crate::evaluator::ExactMatchEvaluator;
    use async_trait::async_trait;
    use serde_json::json;

    /// A mock Runnable that echoes the input value.
    struct EchoRunnable;

    #[async_trait]
    impl Runnable for EchoRunnable {
        type Input = Value;
        type Output = Value;

        async fn invoke(&self, input: Self::Input, _config: &RunnableConfig) -> Result<Value> {
            Ok(input)
        }
    }

    fn make_dataset() -> Dataset {
        let mut ds = Dataset::new("test-ds");
        ds.add_example(Example {
            id: "ex1".into(),
            input: json!("hello"),
            expected: Some(json!("hello")),
            metadata: Default::default(),
        });
        ds.add_example(Example {
            id: "ex2".into(),
            input: json!("world"),
            expected: Some(json!("hello")),
            metadata: Default::default(),
        });
        ds
    }

    #[tokio::test]
    async fn run_with_exact_match() {
        let runner = EvalRunner::new().add_evaluator(ExactMatchEvaluator);
        let dataset = make_dataset();
        let config = RunnableConfig::default();

        let report = runner.run(&EchoRunnable, &dataset, &config).await.unwrap();

        assert_eq!(report.dataset_name, "test-ds");
        assert_eq!(report.total_examples, 2);
        assert_eq!(report.results.len(), 2);

        // ex1: input="hello", expected="hello" → match (1.0)
        assert_eq!(report.results[0].example_id, "ex1");
        assert_eq!(report.results[0].scores[0].value, 1.0);

        // ex2: input="world", expected="hello" → no match (0.0)
        assert_eq!(report.results[1].example_id, "ex2");
        assert_eq!(report.results[1].scores[0].value, 0.0);

        // Aggregate: (1.0 + 0.0) / 2 = 0.5
        let agg = report.aggregate_scores.get("exact_match").unwrap();
        assert!((agg - 0.5).abs() < 1e-10);
    }

    #[tokio::test]
    async fn report_structure() {
        let runner = EvalRunner::new().add_evaluator(ExactMatchEvaluator);
        let dataset = make_dataset();
        let config = RunnableConfig::default();

        let report = runner.run(&EchoRunnable, &dataset, &config).await.unwrap();

        // Verify serialization works
        let json_str = serde_json::to_string(&report).unwrap();
        assert!(json_str.contains("test-ds"));
        assert!(json_str.contains("exact_match"));

        // Mean latency should be non-negative
        assert!(report.mean_latency_ms >= 0.0);
    }

    #[tokio::test]
    async fn empty_dataset_run() {
        let runner = EvalRunner::new().add_evaluator(ExactMatchEvaluator);
        let dataset = Dataset::new("empty");
        let config = RunnableConfig::default();

        let report = runner.run(&EchoRunnable, &dataset, &config).await.unwrap();

        assert_eq!(report.total_examples, 0);
        assert!(report.results.is_empty());
        assert!(report.aggregate_scores.is_empty());
        assert_eq!(report.mean_latency_ms, 0.0);
    }

    #[tokio::test]
    async fn run_no_evaluators() {
        let runner = EvalRunner::new();
        let mut dataset = Dataset::new("no-eval");
        dataset.add_example(Example {
            id: "ex1".into(),
            input: json!("test"),
            expected: None,
            metadata: Default::default(),
        });
        let config = RunnableConfig::default();

        let report = runner.run(&EchoRunnable, &dataset, &config).await.unwrap();
        assert_eq!(report.total_examples, 1);
        assert!(report.results[0].scores.is_empty());
        assert!(report.aggregate_scores.is_empty());
    }
}
