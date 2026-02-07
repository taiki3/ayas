pub mod model;
pub mod runnable;
pub mod tool;

use std::sync::Arc;

use ayas_core::model::ChatModel;
use ayas_core::runnable::Runnable;
use ayas_core::tool::Tool;

use crate::client::SmithClient;
use crate::types::RunType;

pub use self::model::TracedChatModel;
pub use self::runnable::TracedRunnable;
pub use self::tool::TracedTool;

/// Extension trait for adding tracing to any Runnable.
pub trait TraceExt: Runnable + Sized {
    /// Wrap this Runnable with tracing instrumentation.
    fn traced(self, client: SmithClient, name: &str) -> TracedRunnable<Self> {
        TracedRunnable::new(self, client, name, RunType::Chain)
    }

    /// Wrap this Runnable with tracing and a specific RunType.
    fn traced_as(self, client: SmithClient, name: &str, run_type: RunType) -> TracedRunnable<Self> {
        TracedRunnable::new(self, client, name, run_type)
    }
}

impl<T: Runnable + Sized> TraceExt for T {}

/// Create a traced ChatModel wrapper.
pub fn traced_model(model: Arc<dyn ChatModel>, client: SmithClient) -> Arc<TracedChatModel> {
    Arc::new(TracedChatModel::new(model, client))
}

/// Create a traced Tool wrapper.
pub fn traced_tool(tool: Arc<dyn Tool>, client: SmithClient) -> Arc<TracedTool> {
    Arc::new(TracedTool::new(tool, client))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ayas_core::config::RunnableConfig;
    use ayas_core::error::Result;
    use async_trait::async_trait;

    struct DoubleRunnable;

    #[async_trait]
    impl Runnable for DoubleRunnable {
        type Input = i32;
        type Output = i32;

        async fn invoke(&self, input: i32, _config: &RunnableConfig) -> Result<i32> {
            Ok(input * 2)
        }
    }

    #[tokio::test]
    async fn trace_ext_default_chain_type() {
        let traced = DoubleRunnable.traced(SmithClient::noop(), "double");
        let config = RunnableConfig::default();
        let result = traced.invoke(5, &config).await.unwrap();
        assert_eq!(result, 10);
    }

    #[tokio::test]
    async fn trace_ext_with_run_type() {
        let traced = DoubleRunnable.traced_as(SmithClient::noop(), "double", RunType::Graph);
        let config = RunnableConfig::default();
        let result = traced.invoke(3, &config).await.unwrap();
        assert_eq!(result, 6);
    }
}
