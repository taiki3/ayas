use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Type of a traced run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunType {
    Chain,
    Llm,
    Tool,
    Retriever,
    Graph,
}

impl RunType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Chain => "chain",
            Self::Llm => "llm",
            Self::Tool => "tool",
            Self::Retriever => "retriever",
            Self::Graph => "graph",
        }
    }
}

impl std::fmt::Display for RunType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for RunType {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "chain" => Ok(Self::Chain),
            "llm" => Ok(Self::Llm),
            "tool" => Ok(Self::Tool),
            "retriever" => Ok(Self::Retriever),
            "graph" => Ok(Self::Graph),
            other => Err(format!("unknown run type: '{other}'")),
        }
    }
}

/// Status of a traced run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    Running,
    Success,
    Error,
}

impl RunStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Success => "success",
            Self::Error => "error",
        }
    }
}

impl std::fmt::Display for RunStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for RunStatus {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "running" => Ok(Self::Running),
            "success" => Ok(Self::Success),
            "error" => Ok(Self::Error),
            other => Err(format!("unknown run status: '{other}'")),
        }
    }
}

/// A single traced run (span) recording one invocation in a pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Run {
    pub run_id: Uuid,
    pub parent_run_id: Option<Uuid>,
    pub trace_id: Uuid,
    pub name: String,
    pub run_type: RunType,
    pub project: String,
    pub start_time: DateTime<Utc>,
    pub end_time: Option<DateTime<Utc>>,
    pub status: RunStatus,
    pub input: String,
    pub output: Option<String>,
    pub error: Option<String>,
    pub tags: Vec<String>,
    pub metadata: String,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub latency_ms: Option<i64>,
    /// Hierarchical ordering key: `{timestamp}.{run_id_prefix}` segments
    /// joined by `.` to represent the execution tree.
    pub dotted_order: Option<String>,
}

impl Run {
    /// Start building a new run.
    pub fn builder(name: impl Into<String>, run_type: RunType) -> RunBuilder {
        let run_id = Uuid::new_v4();
        RunBuilder {
            run_id,
            parent_run_id: None,
            trace_id: run_id,
            name: name.into(),
            run_type,
            project: "default".into(),
            start_time: Utc::now(),
            input: "{}".into(),
            tags: Vec::new(),
            metadata: "{}".into(),
            dotted_order: None,
        }
    }
}

/// Builder for constructing Run instances.
pub struct RunBuilder {
    run_id: Uuid,
    parent_run_id: Option<Uuid>,
    trace_id: Uuid,
    name: String,
    run_type: RunType,
    project: String,
    start_time: DateTime<Utc>,
    input: String,
    tags: Vec<String>,
    metadata: String,
    dotted_order: Option<String>,
}

impl RunBuilder {
    pub fn run_id(mut self, id: Uuid) -> Self {
        self.run_id = id;
        self
    }

    pub fn parent_run_id(mut self, id: Uuid) -> Self {
        self.parent_run_id = Some(id);
        self
    }

    pub fn trace_id(mut self, id: Uuid) -> Self {
        self.trace_id = id;
        self
    }

    pub fn project(mut self, project: impl Into<String>) -> Self {
        self.project = project.into();
        self
    }

    pub fn start_time(mut self, t: DateTime<Utc>) -> Self {
        self.start_time = t;
        self
    }

    pub fn input(mut self, input: impl Into<String>) -> Self {
        self.input = input.into();
        self
    }

    pub fn tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }

    pub fn metadata(mut self, metadata: impl Into<String>) -> Self {
        self.metadata = metadata.into();
        self
    }

    pub fn dotted_order(mut self, order: impl Into<String>) -> Self {
        self.dotted_order = Some(order.into());
        self
    }

    /// Start the run with status Running (no end_time, no output).
    /// Used for the 2-phase lifecycle: start() then patch with end_time/status.
    pub fn start(self) -> Run {
        Run {
            run_id: self.run_id,
            parent_run_id: self.parent_run_id,
            trace_id: self.trace_id,
            name: self.name,
            run_type: self.run_type,
            project: self.project,
            start_time: self.start_time,
            end_time: None,
            status: RunStatus::Running,
            input: self.input,
            output: None,
            error: None,
            tags: self.tags,
            metadata: self.metadata,
            input_tokens: None,
            output_tokens: None,
            total_tokens: None,
            latency_ms: None,
            dotted_order: self.dotted_order,
        }
    }

    /// Finish the run with a successful result.
    pub fn finish_ok(self, output: impl Into<String>) -> Run {
        let end_time = Utc::now();
        let latency_ms = (end_time - self.start_time).num_milliseconds();
        Run {
            run_id: self.run_id,
            parent_run_id: self.parent_run_id,
            trace_id: self.trace_id,
            name: self.name,
            run_type: self.run_type,
            project: self.project,
            start_time: self.start_time,
            end_time: Some(end_time),
            status: RunStatus::Success,
            input: self.input,
            output: Some(output.into()),
            error: None,
            tags: self.tags,
            metadata: self.metadata,
            input_tokens: None,
            output_tokens: None,
            total_tokens: None,
            latency_ms: Some(latency_ms),
            dotted_order: self.dotted_order,
        }
    }

    /// Finish the run with an error.
    pub fn finish_err(self, error: impl Into<String>) -> Run {
        let end_time = Utc::now();
        let latency_ms = (end_time - self.start_time).num_milliseconds();
        Run {
            run_id: self.run_id,
            parent_run_id: self.parent_run_id,
            trace_id: self.trace_id,
            name: self.name,
            run_type: self.run_type,
            project: self.project,
            start_time: self.start_time,
            end_time: Some(end_time),
            status: RunStatus::Error,
            input: self.input,
            output: None,
            error: Some(error.into()),
            tags: self.tags,
            metadata: self.metadata,
            input_tokens: None,
            output_tokens: None,
            total_tokens: None,
            latency_ms: Some(latency_ms),
            dotted_order: self.dotted_order,
        }
    }

    /// Finish the run with a successful LLM result including token usage.
    pub fn finish_llm(
        self,
        output: impl Into<String>,
        input_tokens: i64,
        output_tokens: i64,
        total_tokens: i64,
    ) -> Run {
        let mut run = self.finish_ok(output);
        run.input_tokens = Some(input_tokens);
        run.output_tokens = Some(output_tokens);
        run.total_tokens = Some(total_tokens);
        run
    }
}

/// Summary of token usage from query results.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsageSummary {
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_tokens: i64,
    pub run_count: i64,
}

/// Latency statistics from query results.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LatencyStats {
    pub p50: f64,
    pub p90: f64,
    pub p95: f64,
    pub p99: f64,
}

/// Filter criteria for querying runs.
#[derive(Debug, Clone, Default)]
pub struct RunFilter {
    pub project: Option<String>,
    pub run_type: Option<RunType>,
    pub status: Option<RunStatus>,
    pub name: Option<String>,
    pub tags: Vec<String>,
    pub start_after: Option<DateTime<Utc>>,
    pub start_before: Option<DateTime<Utc>>,
    pub trace_id: Option<Uuid>,
    pub parent_run_id: Option<Uuid>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

/// A feedback entry associated with a run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Feedback {
    pub id: Uuid,
    pub run_id: Uuid,
    pub key: String,
    pub score: f64,
    #[serde(default)]
    pub comment: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Filter criteria for querying feedback.
#[derive(Debug, Clone, Default)]
pub struct FeedbackFilter {
    pub run_id: Option<Uuid>,
    pub key: Option<String>,
}

/// Partial update for an existing run (2-phase lifecycle).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RunPatch {
    #[serde(default)]
    pub end_time: Option<DateTime<Utc>>,
    #[serde(default)]
    pub output: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub status: Option<RunStatus>,
    #[serde(default)]
    pub input_tokens: Option<i64>,
    #[serde(default)]
    pub output_tokens: Option<i64>,
    #[serde(default)]
    pub total_tokens: Option<i64>,
    #[serde(default)]
    pub latency_ms: Option<i64>,
}

impl Run {
    /// Apply a patch to this run, updating only the fields present in the patch.
    pub fn apply_patch(&mut self, patch: &RunPatch) {
        if let Some(end_time) = patch.end_time {
            self.end_time = Some(end_time);
            self.latency_ms = Some((end_time - self.start_time).num_milliseconds());
        }
        if let Some(ref output) = patch.output {
            self.output = Some(output.clone());
        }
        if let Some(ref error) = patch.error {
            self.error = Some(error.clone());
        }
        if let Some(status) = patch.status {
            self.status = status;
        }
        if let Some(tokens) = patch.input_tokens {
            self.input_tokens = Some(tokens);
        }
        if let Some(tokens) = patch.output_tokens {
            self.output_tokens = Some(tokens);
        }
        if let Some(tokens) = patch.total_tokens {
            self.total_tokens = Some(tokens);
        }
        if let Some(ms) = patch.latency_ms {
            self.latency_ms = Some(ms);
        }
    }
}

// ---------------------------------------------------------------------------
// Project / Dataset / Example
// ---------------------------------------------------------------------------

/// A project groups runs and datasets together.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// A dataset contains a collection of examples for evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dataset {
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub project_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

/// A single example (input/output pair) in a dataset.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Example {
    pub id: Uuid,
    pub dataset_id: Uuid,
    pub input: String,
    #[serde(default)]
    pub output: Option<String>,
    #[serde(default)]
    pub metadata: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_type_as_str() {
        assert_eq!(RunType::Chain.as_str(), "chain");
        assert_eq!(RunType::Llm.as_str(), "llm");
        assert_eq!(RunType::Tool.as_str(), "tool");
        assert_eq!(RunType::Retriever.as_str(), "retriever");
        assert_eq!(RunType::Graph.as_str(), "graph");
    }

    #[test]
    fn run_type_display() {
        assert_eq!(RunType::Chain.to_string(), "chain");
        assert_eq!(RunType::Llm.to_string(), "llm");
    }

    #[test]
    fn run_type_from_str() {
        assert_eq!("chain".parse::<RunType>().unwrap(), RunType::Chain);
        assert_eq!("llm".parse::<RunType>().unwrap(), RunType::Llm);
        assert_eq!("tool".parse::<RunType>().unwrap(), RunType::Tool);
        assert!("unknown".parse::<RunType>().is_err());
    }

    #[test]
    fn run_type_serde_roundtrip() {
        let rt = RunType::Llm;
        let json = serde_json::to_string(&rt).unwrap();
        assert_eq!(json, "\"llm\"");
        let parsed: RunType = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, RunType::Llm);
    }

    #[test]
    fn run_status_as_str() {
        assert_eq!(RunStatus::Running.as_str(), "running");
        assert_eq!(RunStatus::Success.as_str(), "success");
        assert_eq!(RunStatus::Error.as_str(), "error");
    }

    #[test]
    fn run_status_from_str() {
        assert_eq!("running".parse::<RunStatus>().unwrap(), RunStatus::Running);
        assert_eq!("success".parse::<RunStatus>().unwrap(), RunStatus::Success);
        assert_eq!("error".parse::<RunStatus>().unwrap(), RunStatus::Error);
        assert!("pending".parse::<RunStatus>().is_err());
    }

    #[test]
    fn run_builder_finish_ok() {
        let run = Run::builder("test-chain", RunType::Chain)
            .project("my-project")
            .input(r#"{"query": "hello"}"#)
            .finish_ok(r#"{"result": "world"}"#);

        assert_eq!(run.name, "test-chain");
        assert_eq!(run.run_type, RunType::Chain);
        assert_eq!(run.project, "my-project");
        assert_eq!(run.status, RunStatus::Success);
        assert!(run.output.is_some());
        assert!(run.error.is_none());
        assert!(run.end_time.is_some());
        assert!(run.latency_ms.is_some());
        assert_eq!(run.trace_id, run.run_id);
        assert!(run.parent_run_id.is_none());
    }

    #[test]
    fn run_builder_finish_err() {
        let run = Run::builder("fail-tool", RunType::Tool)
            .finish_err("connection timeout");

        assert_eq!(run.status, RunStatus::Error);
        assert!(run.output.is_none());
        assert_eq!(run.error.as_deref(), Some("connection timeout"));
    }

    #[test]
    fn run_builder_with_parent() {
        let parent_id = Uuid::new_v4();
        let trace_id = Uuid::new_v4();
        let run = Run::builder("child", RunType::Llm)
            .parent_run_id(parent_id)
            .trace_id(trace_id)
            .finish_ok("done");

        assert_eq!(run.parent_run_id, Some(parent_id));
        assert_eq!(run.trace_id, trace_id);
    }

    #[test]
    fn run_builder_finish_llm_with_tokens() {
        let run = Run::builder("gpt-4o", RunType::Llm)
            .finish_llm("response text", 100, 50, 150);

        assert_eq!(run.status, RunStatus::Success);
        assert_eq!(run.input_tokens, Some(100));
        assert_eq!(run.output_tokens, Some(50));
        assert_eq!(run.total_tokens, Some(150));
    }

    #[test]
    fn run_builder_with_tags() {
        let run = Run::builder("tagged", RunType::Chain)
            .tags(vec!["production".into(), "v2".into()])
            .finish_ok("ok");

        assert_eq!(run.tags, vec!["production", "v2"]);
    }

    #[test]
    fn run_serde_roundtrip() {
        let run = Run::builder("test", RunType::Chain)
            .project("demo")
            .finish_ok("result");

        let json = serde_json::to_string(&run).unwrap();
        let parsed: Run = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.run_id, run.run_id);
        assert_eq!(parsed.name, run.name);
        assert_eq!(parsed.run_type, run.run_type);
        assert_eq!(parsed.project, run.project);
    }

    #[test]
    fn run_filter_default() {
        let filter = RunFilter::default();
        assert!(filter.project.is_none());
        assert!(filter.run_type.is_none());
        assert!(filter.tags.is_empty());
        assert!(filter.limit.is_none());
    }

    #[test]
    fn token_usage_summary_default() {
        let summary = TokenUsageSummary::default();
        assert_eq!(summary.total_input_tokens, 0);
        assert_eq!(summary.run_count, 0);
    }

    #[test]
    fn latency_stats_default() {
        let stats = LatencyStats::default();
        assert_eq!(stats.p50, 0.0);
        assert_eq!(stats.p99, 0.0);
    }

    #[test]
    fn feedback_serde_roundtrip() {
        let fb = Feedback {
            id: Uuid::new_v4(),
            run_id: Uuid::new_v4(),
            key: "correctness".into(),
            score: 0.95,
            comment: Some("Great answer".into()),
            created_at: Utc::now(),
        };
        let json = serde_json::to_string(&fb).unwrap();
        let parsed: Feedback = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, fb.id);
        assert_eq!(parsed.key, "correctness");
        assert!((parsed.score - 0.95).abs() < f64::EPSILON);
    }

    #[test]
    fn feedback_without_comment() {
        let fb = Feedback {
            id: Uuid::new_v4(),
            run_id: Uuid::new_v4(),
            key: "helpfulness".into(),
            score: 0.8,
            comment: None,
            created_at: Utc::now(),
        };
        let json = serde_json::to_string(&fb).unwrap();
        let parsed: Feedback = serde_json::from_str(&json).unwrap();
        assert!(parsed.comment.is_none());
    }

    #[test]
    fn feedback_filter_default() {
        let filter = FeedbackFilter::default();
        assert!(filter.run_id.is_none());
        assert!(filter.key.is_none());
    }

    #[test]
    fn run_builder_start_creates_running_run() {
        let run = Run::builder("test", RunType::Chain)
            .project("proj")
            .input(r#"{"q": "hello"}"#)
            .start();

        assert_eq!(run.status, RunStatus::Running);
        assert!(run.end_time.is_none());
        assert!(run.output.is_none());
        assert!(run.error.is_none());
        assert!(run.latency_ms.is_none());
        assert_eq!(run.name, "test");
        assert_eq!(run.input, r#"{"q": "hello"}"#);
    }

    #[test]
    fn run_patch_apply() {
        let mut run = Run::builder("test", RunType::Chain)
            .project("proj")
            .start();

        assert_eq!(run.status, RunStatus::Running);
        assert!(run.end_time.is_none());

        let end_time = Utc::now();
        let patch = RunPatch {
            end_time: Some(end_time),
            output: Some("result".into()),
            status: Some(RunStatus::Success),
            ..Default::default()
        };

        run.apply_patch(&patch);

        assert_eq!(run.status, RunStatus::Success);
        assert_eq!(run.end_time, Some(end_time));
        assert_eq!(run.output.as_deref(), Some("result"));
        assert!(run.latency_ms.is_some());
    }

    #[test]
    fn run_patch_partial_apply() {
        let mut run = Run::builder("test", RunType::Llm)
            .finish_ok("original output");

        let patch = RunPatch {
            output_tokens: Some(42),
            ..Default::default()
        };

        let original_output = run.output.clone();
        run.apply_patch(&patch);

        // Only output_tokens changed
        assert_eq!(run.output_tokens, Some(42));
        // Other fields unchanged
        assert_eq!(run.output, original_output);
        assert_eq!(run.status, RunStatus::Success);
    }

    #[test]
    fn run_patch_serde_roundtrip() {
        let patch = RunPatch {
            status: Some(RunStatus::Success),
            output: Some("done".into()),
            ..Default::default()
        };
        let json = serde_json::to_string(&patch).unwrap();
        let parsed: RunPatch = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.status, Some(RunStatus::Success));
        assert_eq!(parsed.output.as_deref(), Some("done"));
        assert!(parsed.end_time.is_none());
    }

    #[test]
    fn run_status_running_serde() {
        let status = RunStatus::Running;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"running\"");
        let parsed: RunStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, RunStatus::Running);
    }

    #[test]
    fn project_serde_roundtrip() {
        let p = Project {
            id: Uuid::new_v4(),
            name: "my-project".into(),
            description: Some("A test project".into()),
            created_at: Utc::now(),
        };
        let json = serde_json::to_string(&p).unwrap();
        let parsed: Project = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, p.id);
        assert_eq!(parsed.name, "my-project");
        assert_eq!(parsed.description.as_deref(), Some("A test project"));
    }

    #[test]
    fn dataset_serde_roundtrip() {
        let d = Dataset {
            id: Uuid::new_v4(),
            name: "qa-set".into(),
            description: None,
            project_id: Some(Uuid::new_v4()),
            created_at: Utc::now(),
        };
        let json = serde_json::to_string(&d).unwrap();
        let parsed: Dataset = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, d.id);
        assert_eq!(parsed.name, "qa-set");
        assert_eq!(parsed.project_id, d.project_id);
    }

    #[test]
    fn example_serde_roundtrip() {
        let e = Example {
            id: Uuid::new_v4(),
            dataset_id: Uuid::new_v4(),
            input: r#"{"question": "What is 2+2?"}"#.into(),
            output: Some("4".into()),
            metadata: None,
            created_at: Utc::now(),
        };
        let json = serde_json::to_string(&e).unwrap();
        let parsed: Example = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, e.id);
        assert_eq!(parsed.dataset_id, e.dataset_id);
        assert_eq!(parsed.output.as_deref(), Some("4"));
    }
}
