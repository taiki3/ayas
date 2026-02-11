use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::sync::Mutex;

use ayas_core::error::Result;

use crate::evaluator::{EvalScore, Evaluator};

/// Trait abstracting the SmithStore operations needed by the online evaluator.
/// This avoids a direct dependency on ayas-smith crate.
#[async_trait::async_trait]
pub trait OnlineSmithStore: Send + Sync {
    /// List runs that started after the given timestamp.
    async fn list_runs_after(
        &self,
        project: &str,
        start_after: DateTime<Utc>,
    ) -> Result<Vec<OnlineRun>>;

    /// Store feedback for a run.
    async fn put_feedback(
        &self,
        run_id: uuid::Uuid,
        key: &str,
        score: f64,
        comment: Option<&str>,
    ) -> Result<()>;
}

/// Minimal run representation for online evaluation.
#[derive(Debug, Clone)]
pub struct OnlineRun {
    pub run_id: uuid::Uuid,
    pub output: Option<serde_json::Value>,
    pub start_time: DateTime<Utc>,
}

/// Online evaluator that polls a SmithStore for new runs and applies evaluators.
pub struct OnlineEvaluator {
    store: Arc<dyn OnlineSmithStore>,
    evaluators: Vec<Box<dyn Evaluator>>,
    project: String,
    poll_interval: Duration,
    last_seen: Mutex<DateTime<Utc>>,
}

impl OnlineEvaluator {
    pub fn new(
        store: Arc<dyn OnlineSmithStore>,
        project: impl Into<String>,
        poll_interval: Duration,
    ) -> Self {
        Self {
            store,
            evaluators: Vec::new(),
            project: project.into(),
            poll_interval,
            last_seen: Mutex::new(Utc::now()),
        }
    }

    pub fn add_evaluator(mut self, eval: impl Evaluator + 'static) -> Self {
        self.evaluators.push(Box::new(eval));
        self
    }

    /// Poll once for new runs, evaluate them, and store feedback.
    /// Returns the number of runs evaluated.
    pub async fn poll_once(&self) -> Result<usize> {
        let start_after = {
            let guard = self.last_seen.lock().await;
            *guard
        };

        let runs = self
            .store
            .list_runs_after(&self.project, start_after)
            .await?;

        if runs.is_empty() {
            return Ok(0);
        }

        let mut latest_time = start_after;
        let mut count = 0;

        for run in &runs {
            if run.start_time > latest_time {
                latest_time = run.start_time;
            }

            let output = match &run.output {
                Some(v) => v.clone(),
                None => continue,
            };

            // Create a dummy example for evaluation (no expected value for online eval)
            let example = crate::dataset::Example {
                id: run.run_id.to_string(),
                input: serde_json::Value::Null,
                expected: None,
                metadata: Default::default(),
            };

            for evaluator in &self.evaluators {
                let score: EvalScore = evaluator.evaluate(&example, &output).await?;
                self.store
                    .put_feedback(
                        run.run_id,
                        &score.metric,
                        score.value,
                        score.explanation.as_deref(),
                    )
                    .await?;
            }

            count += 1;
        }

        // Update watermark
        {
            let mut guard = self.last_seen.lock().await;
            *guard = latest_time;
        }

        Ok(count)
    }
}

/// Spawn a background task that continuously polls for new runs and evaluates them.
/// Returns a JoinHandle that can be used to cancel the loop.
pub fn run_online_eval(evaluator: Arc<OnlineEvaluator>) -> tokio::task::JoinHandle<()> {
    let interval = evaluator.poll_interval;
    tokio::spawn(async move {
        loop {
            match evaluator.poll_once().await {
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!("Online eval poll error: {e}");
                }
            }
            tokio::time::sleep(interval).await;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evaluator::ContainsEvaluator;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct MockSmithStore {
        runs: Vec<OnlineRun>,
        feedback_count: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl OnlineSmithStore for MockSmithStore {
        async fn list_runs_after(
            &self,
            _project: &str,
            start_after: DateTime<Utc>,
        ) -> Result<Vec<OnlineRun>> {
            Ok(self
                .runs
                .iter()
                .filter(|r| r.start_time > start_after)
                .cloned()
                .collect())
        }

        async fn put_feedback(
            &self,
            _run_id: uuid::Uuid,
            _key: &str,
            _score: f64,
            _comment: Option<&str>,
        ) -> Result<()> {
            self.feedback_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn poll_once_evaluates_new_runs() {
        let now = Utc::now();
        let store = Arc::new(MockSmithStore {
            runs: vec![
                OnlineRun {
                    run_id: uuid::Uuid::new_v4(),
                    output: Some(json!("hello world")),
                    start_time: now + chrono::Duration::seconds(1),
                },
                OnlineRun {
                    run_id: uuid::Uuid::new_v4(),
                    output: Some(json!("foo bar")),
                    start_time: now + chrono::Duration::seconds(2),
                },
            ],
            feedback_count: AtomicUsize::new(0),
        });

        let evaluator = OnlineEvaluator::new(
            store.clone(),
            "test-project",
            Duration::from_secs(1),
        )
        .add_evaluator(ContainsEvaluator);

        let count = evaluator.poll_once().await.unwrap();
        assert_eq!(count, 2);
        assert_eq!(store.feedback_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn poll_once_skips_runs_without_output() {
        let now = Utc::now();
        let store = Arc::new(MockSmithStore {
            runs: vec![OnlineRun {
                run_id: uuid::Uuid::new_v4(),
                output: None,
                start_time: now + chrono::Duration::seconds(1),
            }],
            feedback_count: AtomicUsize::new(0),
        });

        let evaluator = OnlineEvaluator::new(store.clone(), "proj", Duration::from_secs(1))
            .add_evaluator(ContainsEvaluator);

        let count = evaluator.poll_once().await.unwrap();
        assert_eq!(count, 0);
        assert_eq!(store.feedback_count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn poll_once_empty_runs() {
        let store = Arc::new(MockSmithStore {
            runs: vec![],
            feedback_count: AtomicUsize::new(0),
        });

        let evaluator = OnlineEvaluator::new(store.clone(), "proj", Duration::from_secs(1))
            .add_evaluator(ContainsEvaluator);

        let count = evaluator.poll_once().await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn poll_once_advances_watermark() {
        let now = Utc::now();
        let run_time = now + chrono::Duration::seconds(5);
        let store = Arc::new(MockSmithStore {
            runs: vec![OnlineRun {
                run_id: uuid::Uuid::new_v4(),
                output: Some(json!("test")),
                start_time: run_time,
            }],
            feedback_count: AtomicUsize::new(0),
        });

        let evaluator = OnlineEvaluator::new(store.clone(), "proj", Duration::from_secs(1))
            .add_evaluator(ContainsEvaluator);

        evaluator.poll_once().await.unwrap();

        // Second poll should find no new runs (watermark advanced past run_time)
        let count = evaluator.poll_once().await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn multiple_evaluators() {
        let now = Utc::now();
        let store = Arc::new(MockSmithStore {
            runs: vec![OnlineRun {
                run_id: uuid::Uuid::new_v4(),
                output: Some(json!("test output")),
                start_time: now + chrono::Duration::seconds(1),
            }],
            feedback_count: AtomicUsize::new(0),
        });

        let evaluator = OnlineEvaluator::new(store.clone(), "proj", Duration::from_secs(1))
            .add_evaluator(ContainsEvaluator)
            .add_evaluator(crate::evaluator::ExactMatchEvaluator);

        evaluator.poll_once().await.unwrap();
        // 1 run * 2 evaluators = 2 feedback entries
        assert_eq!(store.feedback_count.load(Ordering::SeqCst), 2);
    }
}
