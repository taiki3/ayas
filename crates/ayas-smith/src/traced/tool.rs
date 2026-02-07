use std::sync::Arc;

use async_trait::async_trait;

use ayas_core::error::Result;
use ayas_core::tool::{Tool, ToolDefinition};

use crate::client::SmithClient;
use crate::types::{Run, RunType};

/// A Tool wrapper that records tracing information for each invocation.
pub struct TracedTool {
    inner: Arc<dyn Tool>,
    client: SmithClient,
}

impl TracedTool {
    pub fn new(inner: Arc<dyn Tool>, client: SmithClient) -> Self {
        Self { inner, client }
    }
}

#[async_trait]
impl Tool for TracedTool {
    fn definition(&self) -> ToolDefinition {
        self.inner.definition()
    }

    async fn call(&self, input: serde_json::Value) -> Result<String> {
        if !self.client.is_enabled() {
            return self.inner.call(input).await;
        }

        let tool_name = self.inner.definition().name;
        let input_json = serde_json::to_string(&input).unwrap_or_else(|_| "{}".into());

        let builder = Run::builder(&tool_name, RunType::Tool)
            .project(self.client.project())
            .input(&input_json);

        match self.inner.call(input).await {
            Ok(output) => {
                let run = builder.finish_ok(&output);
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

    struct MockTool;

    #[async_trait]
    impl Tool for MockTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                name: "mock-tool".into(),
                description: "A mock tool".into(),
                parameters: serde_json::json!({"type": "object"}),
            }
        }

        async fn call(&self, input: serde_json::Value) -> Result<String> {
            let q = input
                .get("query")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            Ok(format!("result for: {q}"))
        }
    }

    struct FailTool;

    #[async_trait]
    impl Tool for FailTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                name: "fail-tool".into(),
                description: "Always fails".into(),
                parameters: serde_json::json!({"type": "object"}),
            }
        }

        async fn call(&self, _input: serde_json::Value) -> Result<String> {
            Err(AyasError::Other("tool error".into()))
        }
    }

    #[tokio::test]
    async fn traced_tool_call_success() {
        let tool = TracedTool::new(Arc::new(MockTool), SmithClient::noop());
        let input = serde_json::json!({"query": "hello"});
        let result = tool.call(input).await.unwrap();
        assert_eq!(result, "result for: hello");
    }

    #[tokio::test]
    async fn traced_tool_definition() {
        let tool = TracedTool::new(Arc::new(MockTool), SmithClient::noop());
        let def = tool.definition();
        assert_eq!(def.name, "mock-tool");
    }

    #[tokio::test]
    async fn traced_tool_error_propagates() {
        let tool = TracedTool::new(Arc::new(FailTool), SmithClient::noop());
        let result = tool.call(serde_json::json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn traced_tool_with_enabled_client() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::client::SmithConfig::default()
            .with_base_dir(dir.path())
            .with_batch_size(1)
            .with_flush_interval(std::time::Duration::from_millis(50));
        let client = SmithClient::new(config);

        let tool = TracedTool::new(Arc::new(MockTool), client);
        let result = tool.call(serde_json::json!({"query": "test"})).await.unwrap();
        assert_eq!(result, "result for: test");

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert!(dir.path().join("default").exists());
    }
}
