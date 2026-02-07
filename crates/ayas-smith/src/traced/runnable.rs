use async_trait::async_trait;
use serde::Serialize;

use ayas_core::config::RunnableConfig;
use ayas_core::error::Result;
use ayas_core::runnable::Runnable;

use crate::client::SmithClient;
use crate::context::{child_config, trace_context};
use crate::types::{Run, RunType};

/// A Runnable wrapper that records tracing information for each invocation.
pub struct TracedRunnable<R: Runnable> {
    inner: R,
    client: SmithClient,
    name: String,
    run_type: RunType,
}

impl<R: Runnable> TracedRunnable<R> {
    pub fn new(inner: R, client: SmithClient, name: impl Into<String>, run_type: RunType) -> Self {
        Self {
            inner,
            client,
            name: name.into(),
            run_type,
        }
    }
}

#[async_trait]
impl<R> Runnable for TracedRunnable<R>
where
    R: Runnable,
    R::Input: Serialize,
    R::Output: Serialize,
{
    type Input = R::Input;
    type Output = R::Output;

    async fn invoke(&self, input: Self::Input, config: &RunnableConfig) -> Result<Self::Output> {
        if !self.client.is_enabled() {
            return self.inner.invoke(input, config).await;
        }

        let (trace_id, parent_run_id) = trace_context(config);
        let run_id = uuid::Uuid::new_v4();

        let input_json = serde_json::to_string(&input).unwrap_or_else(|_| "{}".into());

        let mut builder = Run::builder(&self.name, self.run_type)
            .run_id(run_id)
            .trace_id(trace_id)
            .project(self.client.project())
            .input(&input_json)
            .tags(config.tags.clone());

        if let Some(pid) = parent_run_id {
            builder = builder.parent_run_id(pid);
        }

        let child_cfg = child_config(config, run_id, trace_id);

        match self.inner.invoke(input, &child_cfg).await {
            Ok(output) => {
                let output_json =
                    serde_json::to_string(&output).unwrap_or_else(|_| "null".into());
                let run = builder.finish_ok(output_json);
                self.client.submit_run(run);
                Ok(output)
            }
            Err(e) => {
                let run = builder.finish_err(e.to_string());
                self.client.submit_run(run);
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ayas_core::error::AyasError;

    struct AddOne;

    #[async_trait]
    impl Runnable for AddOne {
        type Input = i32;
        type Output = i32;

        async fn invoke(&self, input: i32, _config: &RunnableConfig) -> Result<i32> {
            Ok(input + 1)
        }
    }

    struct FailRunnable;

    #[async_trait]
    impl Runnable for FailRunnable {
        type Input = i32;
        type Output = i32;

        async fn invoke(&self, _input: i32, _config: &RunnableConfig) -> Result<i32> {
            Err(AyasError::Other("intentional failure".into()))
        }
    }

    #[tokio::test]
    async fn traced_runnable_success() {
        let traced = TracedRunnable::new(AddOne, SmithClient::noop(), "add-one", RunType::Chain);
        let config = RunnableConfig::default();
        let result = traced.invoke(5, &config).await.unwrap();
        assert_eq!(result, 6);
    }

    #[tokio::test]
    async fn traced_runnable_error_propagates() {
        let traced =
            TracedRunnable::new(FailRunnable, SmithClient::noop(), "fail", RunType::Chain);
        let config = RunnableConfig::default();
        let result = traced.invoke(5, &config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn traced_runnable_noop_still_works() {
        let traced = TracedRunnable::new(AddOne, SmithClient::noop(), "add-one", RunType::Chain);
        let config = RunnableConfig::default();
        let result = traced.invoke(10, &config).await.unwrap();
        assert_eq!(result, 11);
    }

    #[tokio::test]
    async fn traced_runnable_with_enabled_client() {
        let dir = tempfile::tempdir().unwrap();
        let smith_config = crate::client::SmithConfig::default()
            .with_base_dir(dir.path())
            .with_batch_size(1)
            .with_flush_interval(std::time::Duration::from_millis(50));
        let client = SmithClient::new(smith_config);

        let traced = TracedRunnable::new(AddOne, client, "add-one", RunType::Chain);
        let config = RunnableConfig::default();
        let result = traced.invoke(5, &config).await.unwrap();
        assert_eq!(result, 6);

        // Wait for flush
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert!(dir.path().join("default").exists());
    }
}
