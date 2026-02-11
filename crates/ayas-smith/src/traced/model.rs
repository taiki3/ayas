use std::sync::Arc;

use async_trait::async_trait;

use ayas_core::error::Result;
use ayas_core::message::Message;
use ayas_core::model::{CallOptions, ChatModel, ChatResult};

use crate::client::SmithClient;
use crate::context::{build_dotted_order, SMITH_TRACE_CTX};
use crate::types::{Run, RunType};

/// A ChatModel wrapper that records tracing information for each generation.
pub struct TracedChatModel {
    inner: Arc<dyn ChatModel>,
    client: SmithClient,
}

impl TracedChatModel {
    pub fn new(inner: Arc<dyn ChatModel>, client: SmithClient) -> Self {
        Self { inner, client }
    }
}

#[async_trait]
impl ChatModel for TracedChatModel {
    async fn generate(
        &self,
        messages: &[Message],
        options: &CallOptions,
    ) -> Result<ChatResult> {
        if !self.client.is_enabled() {
            return self.inner.generate(messages, options).await;
        }

        let input_json = serde_json::to_string(&messages).unwrap_or_else(|_| "[]".into());
        let run_id = uuid::Uuid::new_v4();
        let start_time = chrono::Utc::now();

        // Try to get trace context from task-local (set by TracedRunnable)
        let ctx = SMITH_TRACE_CTX.try_with(|c| c.clone()).ok();

        let mut builder = Run::builder(self.inner.model_name(), RunType::Llm)
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

        match self.inner.generate(messages, options).await {
            Ok(result) => {
                let output_json =
                    serde_json::to_string(&result.message).unwrap_or_else(|_| "null".into());

                let (input_tokens, output_tokens, total_tokens) =
                    if let Some(ref usage) = result.usage {
                        (
                            usage.input_tokens as i64,
                            usage.output_tokens as i64,
                            usage.total_tokens as i64,
                        )
                    } else {
                        (0, 0, 0)
                    };

                let run = builder.finish_llm(output_json, input_tokens, output_tokens, total_tokens);
                self.client.submit_run(run);
                Ok(result)
            }
            Err(e) => {
                let run = builder.finish_err(e.to_string());
                self.client.submit_run(run);
                Err(e)
            }
        }
    }

    fn model_name(&self) -> &str {
        self.inner.model_name()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::SmithTraceCtx;
    use ayas_core::message::{AIContent, UsageMetadata};

    struct MockModel;

    #[async_trait]
    impl ChatModel for MockModel {
        async fn generate(
            &self,
            _messages: &[Message],
            _options: &CallOptions,
        ) -> Result<ChatResult> {
            Ok(ChatResult {
                message: Message::AI(AIContent {
                    content: "Hello!".into(),
                    tool_calls: Vec::new(),
                    usage: Some(UsageMetadata {
                        input_tokens: 10,
                        output_tokens: 5,
                        total_tokens: 15,
                    }),
                }),
                usage: Some(UsageMetadata {
                    input_tokens: 10,
                    output_tokens: 5,
                    total_tokens: 15,
                }),
            })
        }

        fn model_name(&self) -> &str {
            "mock-model"
        }
    }

    #[tokio::test]
    async fn traced_model_generate() {
        let model = TracedChatModel::new(Arc::new(MockModel), SmithClient::noop());
        let messages = vec![Message::user("Hi")];
        let options = CallOptions::default();

        let result = model.generate(&messages, &options).await.unwrap();
        assert_eq!(result.message.content(), "Hello!");
    }

    #[tokio::test]
    async fn traced_model_name() {
        let model = TracedChatModel::new(Arc::new(MockModel), SmithClient::noop());
        assert_eq!(model.model_name(), "mock-model");
    }

    #[tokio::test]
    async fn traced_model_with_enabled_client() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::client::SmithConfig::default()
            .with_base_dir(dir.path())
            .with_batch_size(1)
            .with_flush_interval(std::time::Duration::from_millis(50));
        let client = SmithClient::new(config);

        let model = TracedChatModel::new(Arc::new(MockModel), client);
        let messages = vec![Message::user("Hi")];
        let result = model.generate(&messages, &CallOptions::default()).await.unwrap();
        assert_eq!(result.message.content(), "Hello!");

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert!(dir.path().join("default").exists());
    }

    #[tokio::test]
    async fn traced_model_inherits_trace_context() {
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

        let model = TracedChatModel::new(Arc::new(MockModel), client);

        SMITH_TRACE_CTX
            .scope(ctx, async {
                model
                    .generate(&[Message::user("Hi")], &CallOptions::default())
                    .await
                    .unwrap();
            })
            .await;

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let query = crate::query::SmithQuery::new(dir.path()).unwrap();
        let filter = crate::types::RunFilter {
            project: Some("test-proj".into()),
            run_type: Some(RunType::Llm),
            ..Default::default()
        };
        let runs = query.list_runs(&filter).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].trace_id, trace_id);
        assert_eq!(runs[0].parent_run_id, Some(parent_run_id));
        assert!(runs[0].dotted_order.is_some());
        assert!(runs[0]
            .dotted_order
            .as_ref()
            .unwrap()
            .starts_with("20250210T120000000000Z.abc12345."));
    }
}
