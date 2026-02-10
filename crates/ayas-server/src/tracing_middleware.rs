use ayas_smith::client::{SmithClient, SmithConfig};
use ayas_smith::types::{Run, RunType};
use serde_json::Value;

/// Context for tracing graph executions via ayas-smith.
///
/// Builds [`Run`] records and submits them to the [`SmithClient`] background
/// writer.  Submission is non-blocking (mpsc channel send).
pub struct TracingContext {
    client: SmithClient,
    project: String,
}

impl TracingContext {
    /// Create a TracingContext with an explicit client and project name.
    pub fn new(client: SmithClient, project: impl Into<String>) -> Self {
        Self {
            client,
            project: project.into(),
        }
    }

    /// Create from environment variables.  Returns `None` when tracing is not
    /// globally enabled.
    ///
    /// | Variable                | Purpose                            |
    /// |-------------------------|------------------------------------|
    /// | `AYAS_TRACING_ENABLED`  | Must be `"true"` or `"1"`          |
    /// | `AYAS_SMITH_PROJECT`    | Project name (default: `"default"`) |
    /// | `AYAS_SMITH_BASE_DIR`   | Override base directory             |
    pub fn from_env() -> Option<Self> {
        let enabled = std::env::var("AYAS_TRACING_ENABLED")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);

        if !enabled {
            return None;
        }

        Some(Self::from_env_config())
    }

    /// Create a TracingContext from env-based config *without* checking
    /// `AYAS_TRACING_ENABLED`.  Used for per-request tracing triggered by the
    /// `X-Trace-Enabled` header.
    pub(crate) fn from_env_config() -> Self {
        let project = std::env::var("AYAS_SMITH_PROJECT")
            .unwrap_or_else(|_| "default".into());

        let mut config = SmithConfig::default().with_project(&project);

        if let Ok(base_dir) = std::env::var("AYAS_SMITH_BASE_DIR") {
            config = config.with_base_dir(base_dir);
        }

        let client = SmithClient::new(config);
        Self { client, project }
    }

    /// Record a graph execution run.
    ///
    /// Non-blocking: the [`Run`] is submitted to the [`SmithClient`]'s
    /// background writer via an mpsc channel.
    pub fn record_graph_run(
        &self,
        name: &str,
        input: &Value,
        output: &Value,
        error: Option<&str>,
    ) {
        let input_json = serde_json::to_string(input).unwrap_or_else(|_| "{}".into());

        let builder = Run::builder(name, RunType::Graph)
            .project(&self.project)
            .input(&input_json);

        let run = if let Some(err) = error {
            builder.finish_err(err)
        } else {
            let output_json = serde_json::to_string(output).unwrap_or_else(|_| "null".into());
            builder.finish_ok(output_json)
        };

        self.client.submit_run(run);
    }
}

/// Check `X-Trace-Enabled` header to enable per-request tracing.
pub fn is_tracing_requested(headers: &axum::http::HeaderMap) -> bool {
    headers
        .get("X-Trace-Enabled")
        .and_then(|v| v.to_str().ok())
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_tracing_context_from_env_disabled() {
        // Without AYAS_TRACING_ENABLED, from_env() returns None
        // SAFETY: test-only; env var mutation is not thread-safe but acceptable
        // in serial test execution.
        unsafe { std::env::remove_var("AYAS_TRACING_ENABLED") };
        assert!(TracingContext::from_env().is_none());
    }

    #[test]
    fn test_is_tracing_requested_header() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("X-Trace-Enabled", "true".parse().unwrap());
        assert!(is_tracing_requested(&headers));
    }

    #[test]
    fn test_is_tracing_requested_header_numeric() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("X-Trace-Enabled", "1".parse().unwrap());
        assert!(is_tracing_requested(&headers));
    }

    #[test]
    fn test_is_tracing_requested_no_header() {
        let headers = axum::http::HeaderMap::new();
        assert!(!is_tracing_requested(&headers));
    }

    #[test]
    fn test_is_tracing_requested_header_false() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("X-Trace-Enabled", "false".parse().unwrap());
        assert!(!is_tracing_requested(&headers));
    }

    #[tokio::test]
    async fn test_tracing_context_records_run() {
        let dir = tempfile::tempdir().unwrap();
        let config = SmithConfig::default()
            .with_base_dir(dir.path())
            .with_project("test-tracing")
            .with_batch_size(1)
            .with_flush_interval(Duration::from_millis(50));
        let client = SmithClient::new(config);
        let ctx = TracingContext::new(client, "test-tracing");

        let input = serde_json::json!({"query": "hello"});
        let output = serde_json::json!({"result": "world"});

        ctx.record_graph_run("test-graph", &input, &output, None);

        // Wait for background writer to flush
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(dir.path().join("test-tracing").exists());
    }

    #[tokio::test]
    async fn test_tracing_context_records_error_run() {
        let dir = tempfile::tempdir().unwrap();
        let config = SmithConfig::default()
            .with_base_dir(dir.path())
            .with_project("test-errors")
            .with_batch_size(1)
            .with_flush_interval(Duration::from_millis(50));
        let client = SmithClient::new(config);
        let ctx = TracingContext::new(client, "test-errors");

        let input = serde_json::json!({"query": "fail"});
        let output = serde_json::Value::Null;

        ctx.record_graph_run("test-graph", &input, &output, Some("something broke"));

        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(dir.path().join("test-errors").exists());
    }
}
