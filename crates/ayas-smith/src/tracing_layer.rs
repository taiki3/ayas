//! A `tracing::Layer` implementation that automatically creates Smith runs from spans.
//!
//! # Usage
//!
//! ```rust,ignore
//! use tracing_subscriber::prelude::*;
//! use ayas_smith::tracing_layer::SmithLayer;
//! use ayas_smith::client::{SmithClient, SmithConfig};
//!
//! let client = SmithClient::new(SmithConfig::default());
//! tracing_subscriber::registry()
//!     .with(SmithLayer::new(client))
//!     .init();
//!
//! // Now any span with `smith.*` fields is automatically traced:
//! #[tracing::instrument(fields(smith.run_type = "chain", smith.input = %input))]
//! async fn my_chain(input: &str) -> String {
//!     "result".into()
//! }
//! ```

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::Utc;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id};
use tracing::Subscriber;
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::registry::LookupSpan;
use uuid::Uuid;

use crate::client::SmithClient;
use crate::context::build_dotted_order;
use crate::types::{Run, RunType};

/// Metadata attached to each span tracked by `SmithLayer`.
#[derive(Debug, Clone)]
struct SpanData {
    run_id: Uuid,
    trace_id: Uuid,
    parent_run_id: Option<Uuid>,
    name: String,
    run_type: RunType,
    project: String,
    input: String,
    output: Option<String>,
    start_time: chrono::DateTime<Utc>,
    dotted_order: String,
}

/// Visitor that extracts `smith.*` fields from span attributes.
struct SmithFieldVisitor {
    run_type: Option<String>,
    input: Option<String>,
    output: Option<String>,
    project: Option<String>,
}

impl SmithFieldVisitor {
    fn new() -> Self {
        Self {
            run_type: None,
            input: None,
            output: None,
            project: None,
        }
    }

    fn has_smith_fields(&self) -> bool {
        self.run_type.is_some() || self.input.is_some()
    }
}

impl Visit for SmithFieldVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        match field.name() {
            "smith.run_type" => self.run_type = Some(format!("{value:?}").trim_matches('"').to_string()),
            "smith.input" => self.input = Some(format!("{value:?}").trim_matches('"').to_string()),
            "smith.output" => self.output = Some(format!("{value:?}").trim_matches('"').to_string()),
            "smith.project" => self.project = Some(format!("{value:?}").trim_matches('"').to_string()),
            _ => {}
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "smith.run_type" => self.run_type = Some(value.to_string()),
            "smith.input" => self.input = Some(value.to_string()),
            "smith.output" => self.output = Some(value.to_string()),
            "smith.project" => self.project = Some(value.to_string()),
            _ => {}
        }
    }
}

/// Visitor that extracts `smith.output` recorded after span creation.
struct OutputVisitor {
    output: Option<String>,
}

impl OutputVisitor {
    fn new() -> Self {
        Self { output: None }
    }
}

impl Visit for OutputVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "smith.output" {
            self.output = Some(format!("{value:?}").trim_matches('"').to_string());
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "smith.output" {
            self.output = Some(value.to_string());
        }
    }
}

/// A `tracing::Layer` that intercepts spans with `smith.*` fields
/// and automatically records them as Smith runs.
///
/// Only spans that contain at least one `smith.*` field are tracked.
/// Other spans are ignored.
pub struct SmithLayer {
    client: SmithClient,
    spans: Arc<Mutex<HashMap<Id, SpanData>>>,
}

impl SmithLayer {
    /// Create a new `SmithLayer` that sends runs to the given `SmithClient`.
    pub fn new(client: SmithClient) -> Self {
        Self {
            client,
            spans: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn find_parent_span_data(&self, ctx: &Context<'_, impl Subscriber + for<'a> LookupSpan<'a>>, attrs: &Attributes<'_>) -> Option<SpanData> {
        // Check explicit parent first, then current span
        let parent_id = if let Some(parent) = attrs.parent() {
            Some(parent.clone())
        } else if attrs.is_contextual() {
            ctx.current_span().id().cloned()
        } else {
            None
        };

        if let Some(pid) = parent_id {
            let spans = self.spans.lock().unwrap();
            spans.get(&pid).cloned()
        } else {
            None
        }
    }
}

impl<S> Layer<S> for SmithLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let mut visitor = SmithFieldVisitor::new();
        attrs.record(&mut visitor);

        if !visitor.has_smith_fields() {
            return;
        }

        let run_type = visitor
            .run_type
            .as_deref()
            .and_then(|s| s.parse::<RunType>().ok())
            .unwrap_or(RunType::Chain);

        let run_id = Uuid::new_v4();
        let start_time = Utc::now();
        let name = attrs.metadata().name().to_string();

        let parent_data = self.find_parent_span_data(&ctx, attrs);

        let (trace_id, parent_run_id, dotted_order) = match &parent_data {
            Some(parent) => {
                let dotted = build_dotted_order(start_time, run_id, Some(&parent.dotted_order));
                (parent.trace_id, Some(parent.run_id), dotted)
            }
            None => {
                let dotted = build_dotted_order(start_time, run_id, None);
                (run_id, None, dotted)
            }
        };

        let project = visitor
            .project
            .unwrap_or_else(|| self.client.project().to_string());
        let input = visitor.input.unwrap_or_else(|| "{}".to_string());

        let span_data = SpanData {
            run_id,
            trace_id,
            parent_run_id,
            name,
            run_type,
            project,
            input,
            output: visitor.output,
            start_time,
            dotted_order,
        };

        self.spans.lock().unwrap().insert(id.clone(), span_data);
    }

    fn on_record(&self, id: &Id, values: &tracing::span::Record<'_>, _ctx: Context<'_, S>) {
        let mut visitor = OutputVisitor::new();
        values.record(&mut visitor);

        if let Some(output) = visitor.output {
            let mut spans = self.spans.lock().unwrap();
            if let Some(data) = spans.get_mut(id) {
                data.output = Some(output);
            }
        }
    }

    fn on_close(&self, id: Id, _ctx: Context<'_, S>) {
        let data = {
            let mut spans = self.spans.lock().unwrap();
            spans.remove(&id)
        };

        let Some(data) = data else {
            return;
        };

        let end_time = Utc::now();
        let latency_ms = (end_time - data.start_time).num_milliseconds();

        let mut builder = Run::builder(&data.name, data.run_type)
            .run_id(data.run_id)
            .trace_id(data.trace_id)
            .project(&data.project)
            .input(&data.input)
            .start_time(data.start_time)
            .dotted_order(&data.dotted_order);

        if let Some(pid) = data.parent_run_id {
            builder = builder.parent_run_id(pid);
        }

        let mut run = match &data.output {
            Some(output) => builder.finish_ok(output),
            None => builder.finish_ok(""),
        };
        run.latency_ms = Some(latency_ms);

        self.client.submit_run(run);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::SmithConfig;
    use crate::query::SmithQuery;
    use crate::types::{RunFilter, RunStatus};
    use std::time::Duration;
    use tracing_subscriber::prelude::*;

    fn setup_layer(dir: &std::path::Path) -> (SmithClient, tracing::subscriber::DefaultGuard) {
        let config = SmithConfig::default()
            .with_base_dir(dir)
            .with_project("layer-test")
            .with_batch_size(1)
            .with_flush_interval(Duration::from_millis(50));

        let client = SmithClient::new(config);
        let layer = SmithLayer::new(client.clone());

        let subscriber = tracing_subscriber::registry().with(layer);
        let guard = tracing::subscriber::set_default(subscriber);

        (client, guard)
    }

    #[tokio::test]
    async fn basic_span_creates_run() {
        let dir = tempfile::tempdir().unwrap();
        let (_client, _guard) = setup_layer(dir.path());

        {
            let span = tracing::info_span!("test_fn", smith.run_type = "chain", smith.input = "hello");
            let _enter = span.enter();
        }

        tokio::time::sleep(Duration::from_millis(300)).await;

        let query = SmithQuery::new(dir.path()).unwrap();
        let filter = RunFilter {
            project: Some("layer-test".into()),
            ..Default::default()
        };
        let runs = query.list_runs(&filter).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].name, "test_fn");
        assert_eq!(runs[0].run_type, RunType::Chain);
        assert_eq!(runs[0].input, "hello");
        assert_eq!(runs[0].status, RunStatus::Success);
    }

    #[tokio::test]
    async fn span_without_smith_fields_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let (_client, _guard) = setup_layer(dir.path());

        {
            let span = tracing::info_span!("normal_span", some_field = "value");
            let _enter = span.enter();
        }

        tokio::time::sleep(Duration::from_millis(300)).await;

        // No parquet files should be created since no smith spans were recorded.
        // SmithQuery would error if there are no parquet files, so just check
        // the project directory doesn't exist.
        let project_dir = dir.path().join("layer-test");
        assert!(!project_dir.exists());
    }

    #[tokio::test]
    async fn parent_child_hierarchy() {
        let dir = tempfile::tempdir().unwrap();
        let (_client, _guard) = setup_layer(dir.path());

        {
            let parent = tracing::info_span!("parent_chain", smith.run_type = "chain", smith.input = "p_in");
            let _p = parent.enter();
            {
                let child = tracing::info_span!("child_llm", smith.run_type = "llm", smith.input = "c_in");
                let _c = child.enter();
            }
        }

        tokio::time::sleep(Duration::from_millis(300)).await;

        let query = SmithQuery::new(dir.path()).unwrap();
        let filter = RunFilter {
            project: Some("layer-test".into()),
            ..Default::default()
        };
        let runs = query.list_runs(&filter).unwrap();
        assert_eq!(runs.len(), 2);

        let parent_run = runs.iter().find(|r| r.name == "parent_chain").unwrap();
        let child_run = runs.iter().find(|r| r.name == "child_llm").unwrap();

        // Child should reference parent
        assert_eq!(child_run.parent_run_id, Some(parent_run.run_id));
        assert_eq!(child_run.trace_id, parent_run.trace_id);

        // Parent is the root
        assert!(parent_run.parent_run_id.is_none());
        assert_eq!(parent_run.trace_id, parent_run.run_id);
    }

    #[tokio::test]
    async fn output_recorded_via_record() {
        let dir = tempfile::tempdir().unwrap();
        let (_client, _guard) = setup_layer(dir.path());

        {
            let span = tracing::info_span!(
                "with_output",
                smith.run_type = "chain",
                smith.input = "in",
                smith.output = tracing::field::Empty,
            );
            let _enter = span.enter();
            span.record("smith.output", "my_result");
        }

        tokio::time::sleep(Duration::from_millis(300)).await;

        let query = SmithQuery::new(dir.path()).unwrap();
        let filter = RunFilter {
            project: Some("layer-test".into()),
            ..Default::default()
        };
        let runs = query.list_runs(&filter).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].output.as_deref(), Some("my_result"));
    }

    #[tokio::test]
    async fn custom_project_field() {
        let dir = tempfile::tempdir().unwrap();
        let (_client, _guard) = setup_layer(dir.path());

        {
            let span = tracing::info_span!(
                "custom_proj",
                smith.run_type = "tool",
                smith.input = "x",
                smith.project = "custom-proj",
            );
            let _enter = span.enter();
        }

        tokio::time::sleep(Duration::from_millis(300)).await;

        let query = SmithQuery::new(dir.path()).unwrap();
        let filter = RunFilter {
            project: Some("custom-proj".into()),
            ..Default::default()
        };
        let runs = query.list_runs(&filter).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].project, "custom-proj");
    }

    #[tokio::test]
    async fn noop_client_layer_does_not_panic() {
        let client = SmithClient::noop();
        let layer = SmithLayer::new(client);
        let subscriber = tracing_subscriber::registry().with(layer);
        let _guard = tracing::subscriber::set_default(subscriber);

        {
            let span = tracing::info_span!("noop_test", smith.run_type = "chain", smith.input = "hi");
            let _enter = span.enter();
        }
        // No panic = success
    }
}
