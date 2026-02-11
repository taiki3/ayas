use std::sync::Arc;

use async_trait::async_trait;

use ayas_core::error::Result;
use ayas_core::tool::{Tool, ToolDefinition};

use crate::client::SmithClient;
use crate::context::{build_dotted_order, SMITH_TRACE_CTX};
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
        let run_id = uuid::Uuid::new_v4();
        let start_time = chrono::Utc::now();

        // Try to get trace context from task-local (set by TracedRunnable)
        let ctx = SMITH_TRACE_CTX.try_with(|c| c.clone()).ok();

        let mut builder = Run::builder(&tool_name, RunType::Tool)
            .run_id(run_id)
            .project(self.client.project())
            .input(&input_json)
            .start_time(start_time);

        if let Some(ref ctx) = ctx {
            builder = builder.trace_id(ctx.trace_id);
            if let Some(pid) = ctx.parent_run_id {
                builder = builder.parent_run_id(pid);
            }
            let dotted_order =
                build_dotted_order(start_time, run_id, Some(&ctx.dotted_order));
            builder = builder.dotted_order(dotted_order);
        }

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
    use crate::context::SmithTraceCtx;
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

    #[tokio::test]
    async fn traced_tool_inherits_trace_context() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::client::SmithConfig::default()
            .with_base_dir(dir.path())
            .with_project("test-proj")
            .with_batch_size(1)
            .with_flush_interval(std::time::Duration::from_millis(50));
        let client = SmithClient::new(config);

        let trace_id = uuid::Uuid::new_v4();
        let parent_run_id = uuid::Uuid::new_v4();
        let ctx = SmithTraceCtx {
            trace_id,
            parent_run_id: Some(parent_run_id),
            dotted_order: "20250210T120000000000Z.abc12345".into(),
        };

        let tool = TracedTool::new(Arc::new(MockTool), client);

        SMITH_TRACE_CTX
            .scope(ctx, async {
                tool.call(serde_json::json!({"query": "test"})).await.unwrap();
            })
            .await;

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let query = crate::query::SmithQuery::new(dir.path()).unwrap();
        let filter = crate::types::RunFilter {
            project: Some("test-proj".into()),
            run_type: Some(RunType::Tool),
            ..Default::default()
        };
        let runs = query.list_runs(&filter).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].trace_id, trace_id);
        assert_eq!(runs[0].parent_run_id, Some(parent_run_id));
        assert!(runs[0].dotted_order.is_some());
    }
}
