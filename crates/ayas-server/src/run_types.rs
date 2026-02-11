use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use ayas_smith::prelude::{
    LatencyStats, Run, RunFilter, RunPatch, RunStatus, RunType, TokenUsageSummary,
};

// --- Batch Ingest ---

/// Legacy batch ingest request (backward compatible).
#[derive(Debug, Deserialize)]
pub struct BatchIngestRequest {
    pub runs: Vec<RunDto>,
}

/// New batch request supporting both POST (new runs) and PATCH (updates).
#[derive(Debug, Deserialize)]
pub struct BatchRunRequest {
    #[serde(default)]
    pub post: Vec<RunDto>,
    #[serde(default)]
    pub patch: Vec<RunPatchRequest>,
}

/// Request to patch an existing run.
#[derive(Debug, Clone, Deserialize)]
pub struct RunPatchRequest {
    pub run_id: Uuid,
    #[serde(default = "default_project")]
    pub project: String,
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

impl From<&RunPatchRequest> for RunPatch {
    fn from(req: &RunPatchRequest) -> Self {
        RunPatch {
            end_time: req.end_time,
            output: req.output.clone(),
            error: req.error.clone(),
            status: req.status,
            input_tokens: req.input_tokens,
            output_tokens: req.output_tokens,
            total_tokens: req.total_tokens,
            latency_ms: req.latency_ms,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BatchIngestResponse {
    pub ingested: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BatchRunResponse {
    pub posted: usize,
    pub patched: usize,
}

/// A JSON-friendly representation for ingesting runs via the API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunDto {
    #[serde(default = "Uuid::new_v4")]
    pub run_id: Uuid,
    #[serde(default)]
    pub parent_run_id: Option<Uuid>,
    #[serde(default)]
    pub trace_id: Option<Uuid>,
    pub name: String,
    pub run_type: RunType,
    #[serde(default = "default_project")]
    pub project: String,
    #[serde(default = "Utc::now")]
    pub start_time: DateTime<Utc>,
    #[serde(default)]
    pub end_time: Option<DateTime<Utc>>,
    #[serde(default = "default_status")]
    pub status: RunStatus,
    #[serde(default = "default_empty_json")]
    pub input: String,
    #[serde(default)]
    pub output: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default = "default_empty_json")]
    pub metadata: String,
    #[serde(default)]
    pub input_tokens: Option<i64>,
    #[serde(default)]
    pub output_tokens: Option<i64>,
    #[serde(default)]
    pub total_tokens: Option<i64>,
    #[serde(default)]
    pub latency_ms: Option<i64>,
}

fn default_project() -> String {
    "default".into()
}

fn default_status() -> RunStatus {
    RunStatus::Success
}

fn default_empty_json() -> String {
    "{}".into()
}

impl From<RunDto> for Run {
    fn from(dto: RunDto) -> Self {
        let trace_id = dto.trace_id.unwrap_or(dto.run_id);
        Run {
            run_id: dto.run_id,
            parent_run_id: dto.parent_run_id,
            trace_id,
            name: dto.name,
            run_type: dto.run_type,
            project: dto.project,
            start_time: dto.start_time,
            end_time: dto.end_time,
            status: dto.status,
            input: dto.input,
            output: dto.output,
            error: dto.error,
            tags: dto.tags,
            metadata: dto.metadata,
            input_tokens: dto.input_tokens,
            output_tokens: dto.output_tokens,
            total_tokens: dto.total_tokens,
            latency_ms: dto.latency_ms,
            dotted_order: None,
        }
    }
}

impl From<&Run> for RunDto {
    fn from(run: &Run) -> Self {
        RunDto {
            run_id: run.run_id,
            parent_run_id: run.parent_run_id,
            trace_id: Some(run.trace_id),
            name: run.name.clone(),
            run_type: run.run_type,
            project: run.project.clone(),
            start_time: run.start_time,
            end_time: run.end_time,
            status: run.status,
            input: run.input.clone(),
            output: run.output.clone(),
            error: run.error.clone(),
            tags: run.tags.clone(),
            metadata: run.metadata.clone(),
            input_tokens: run.input_tokens,
            output_tokens: run.output_tokens,
            total_tokens: run.total_tokens,
            latency_ms: run.latency_ms,
        }
    }
}

// --- Query ---

/// Serde-friendly filter for querying runs.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RunFilterRequest {
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub run_type: Option<RunType>,
    #[serde(default)]
    pub status: Option<RunStatus>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub start_after: Option<DateTime<Utc>>,
    #[serde(default)]
    pub start_before: Option<DateTime<Utc>>,
    #[serde(default)]
    pub trace_id: Option<Uuid>,
    #[serde(default)]
    pub parent_run_id: Option<Uuid>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub offset: Option<usize>,
}

impl From<RunFilterRequest> for RunFilter {
    fn from(req: RunFilterRequest) -> Self {
        RunFilter {
            project: req.project,
            run_type: req.run_type,
            status: req.status,
            name: req.name,
            tags: req.tags,
            start_after: req.start_after,
            start_before: req.start_before,
            trace_id: req.trace_id,
            parent_run_id: req.parent_run_id,
            limit: req.limit,
            offset: req.offset,
        }
    }
}

/// Compact summary returned when listing runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSummary {
    pub run_id: Uuid,
    pub name: String,
    pub run_type: RunType,
    pub status: RunStatus,
    pub project: String,
    pub start_time: DateTime<Utc>,
    pub latency_ms: Option<i64>,
    pub total_tokens: Option<i64>,
}

impl From<&Run> for RunSummary {
    fn from(run: &Run) -> Self {
        RunSummary {
            run_id: run.run_id,
            name: run.name.clone(),
            run_type: run.run_type,
            status: run.status,
            project: run.project.clone(),
            start_time: run.start_time,
            latency_ms: run.latency_ms,
            total_tokens: run.total_tokens,
        }
    }
}

// --- Stats ---

#[derive(Debug, Serialize, Deserialize)]
pub struct StatsResponse {
    pub tokens: TokenUsageSummary,
    pub latency: LatencyStats,
}

// --- Get endpoints query params ---

#[derive(Debug, Deserialize)]
pub struct ProjectQuery {
    #[serde(default = "default_project")]
    pub project: String,
}

// --- Feedback ---

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

#[derive(Debug, Deserialize)]
pub struct FeedbackRequest {
    pub run_id: Uuid,
    pub key: String,
    pub score: f64,
    #[serde(default)]
    pub comment: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FeedbackResponse {
    pub id: Uuid,
    pub run_id: Uuid,
    pub key: String,
    pub score: f64,
}

#[derive(Debug, Default, Deserialize)]
pub struct FeedbackQueryRequest {
    #[serde(default)]
    pub run_id: Option<Uuid>,
    #[serde(default)]
    pub key: Option<String>,
}

// --- Projects ---

#[derive(Debug, Deserialize)]
pub struct CreateProjectRequest {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

// --- Datasets ---

#[derive(Debug, Deserialize)]
pub struct CreateDatasetRequest {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub project_id: Option<Uuid>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ListDatasetsQuery {
    #[serde(default)]
    pub project_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct AddExamplesRequest {
    pub examples: Vec<ExampleInput>,
}

#[derive(Debug, Deserialize)]
pub struct ExampleInput {
    pub input: String,
    #[serde(default)]
    pub output: Option<String>,
    #[serde(default)]
    pub metadata: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_dto_deserialize_minimal() {
        let json = r#"{
            "name": "my-chain",
            "run_type": "chain"
        }"#;
        let dto: RunDto = serde_json::from_str(json).unwrap();
        assert_eq!(dto.name, "my-chain");
        assert_eq!(dto.run_type, RunType::Chain);
        assert_eq!(dto.project, "default");
        assert_eq!(dto.status, RunStatus::Success);
    }

    #[test]
    fn run_dto_deserialize_full() {
        let run_id = Uuid::new_v4();
        let json = serde_json::json!({
            "run_id": run_id,
            "name": "gpt-4o",
            "run_type": "llm",
            "project": "my-proj",
            "status": "success",
            "input": "{\"query\": \"hello\"}",
            "output": "{\"result\": \"world\"}",
            "input_tokens": 50,
            "output_tokens": 10,
            "total_tokens": 60,
            "latency_ms": 123,
            "tags": ["prod"]
        });
        let dto: RunDto = serde_json::from_value(json).unwrap();
        assert_eq!(dto.run_id, run_id);
        assert_eq!(dto.name, "gpt-4o");
        assert_eq!(dto.input_tokens, Some(50));
        assert_eq!(dto.tags, vec!["prod"]);
    }

    #[test]
    fn run_dto_to_run_conversion() {
        let dto = RunDto {
            run_id: Uuid::new_v4(),
            parent_run_id: None,
            trace_id: None,
            name: "test".into(),
            run_type: RunType::Chain,
            project: "default".into(),
            start_time: Utc::now(),
            end_time: None,
            status: RunStatus::Success,
            input: "{}".into(),
            output: None,
            error: None,
            tags: vec![],
            metadata: "{}".into(),
            input_tokens: None,
            output_tokens: None,
            total_tokens: None,
            latency_ms: None,
        };
        let run: Run = dto.clone().into();
        assert_eq!(run.run_id, dto.run_id);
        // trace_id defaults to run_id when None
        assert_eq!(run.trace_id, dto.run_id);
    }

    #[test]
    fn run_dto_roundtrip() {
        let run = Run::builder("test", RunType::Llm)
            .project("proj")
            .finish_llm("output", 10, 5, 15);
        let dto = RunDto::from(&run);
        let json = serde_json::to_string(&dto).unwrap();
        let parsed: RunDto = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.run_id, run.run_id);
        assert_eq!(parsed.name, "test");
        assert_eq!(parsed.input_tokens, Some(10));
    }

    #[test]
    fn run_filter_request_deserialize() {
        let json = r#"{
            "project": "my-proj",
            "run_type": "llm",
            "limit": 10
        }"#;
        let req: RunFilterRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.project.as_deref(), Some("my-proj"));
        assert_eq!(req.run_type, Some(RunType::Llm));
        assert_eq!(req.limit, Some(10));
    }

    #[test]
    fn run_filter_request_to_filter() {
        let req = RunFilterRequest {
            project: Some("proj".into()),
            run_type: Some(RunType::Tool),
            limit: Some(5),
            ..Default::default()
        };
        let filter: RunFilter = req.into();
        assert_eq!(filter.project.as_deref(), Some("proj"));
        assert_eq!(filter.run_type, Some(RunType::Tool));
        assert_eq!(filter.limit, Some(5));
    }

    #[test]
    fn run_summary_from_run() {
        let run = Run::builder("test", RunType::Chain)
            .project("proj")
            .finish_ok("result");
        let summary = RunSummary::from(&run);
        assert_eq!(summary.run_id, run.run_id);
        assert_eq!(summary.name, "test");
        assert_eq!(summary.project, "proj");
    }

    #[test]
    fn feedback_request_deserialize() {
        let run_id = Uuid::new_v4();
        let json = serde_json::json!({
            "run_id": run_id,
            "key": "correctness",
            "score": 0.95,
            "comment": "Good answer"
        });
        let req: FeedbackRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.run_id, run_id);
        assert_eq!(req.key, "correctness");
        assert!((req.score - 0.95).abs() < f64::EPSILON);
        assert_eq!(req.comment.as_deref(), Some("Good answer"));
    }

    #[test]
    fn feedback_query_default() {
        let req = FeedbackQueryRequest::default();
        assert!(req.run_id.is_none());
        assert!(req.key.is_none());
    }

    #[test]
    fn stats_response_serialize() {
        let resp = StatsResponse {
            tokens: TokenUsageSummary::default(),
            latency: LatencyStats::default(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"tokens\""));
        assert!(json.contains("\"latency\""));
    }

    #[test]
    fn project_query_default() {
        let json = "{}";
        let q: ProjectQuery = serde_json::from_str(json).unwrap();
        assert_eq!(q.project, "default");
    }

    #[test]
    fn batch_run_request_deserialize() {
        let run_id = Uuid::new_v4();
        let json = serde_json::json!({
            "post": [{
                "name": "test-chain",
                "run_type": "chain",
                "status": "running"
            }],
            "patch": [{
                "run_id": run_id,
                "project": "test-proj",
                "status": "success",
                "output": "result"
            }]
        });
        let req: BatchRunRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.post.len(), 1);
        assert_eq!(req.patch.len(), 1);
        assert_eq!(req.patch[0].run_id, run_id);
        assert_eq!(req.patch[0].status, Some(RunStatus::Success));
    }

    #[test]
    fn run_patch_request_to_run_patch() {
        let req = RunPatchRequest {
            run_id: Uuid::new_v4(),
            project: "test-proj".into(),
            end_time: None,
            output: Some("result".into()),
            error: None,
            status: Some(RunStatus::Success),
            input_tokens: None,
            output_tokens: Some(42),
            total_tokens: None,
            latency_ms: None,
        };
        let patch = RunPatch::from(&req);
        assert_eq!(patch.output.as_deref(), Some("result"));
        assert_eq!(patch.status, Some(RunStatus::Success));
        assert_eq!(patch.output_tokens, Some(42));
        assert!(patch.end_time.is_none());
    }

    #[test]
    fn run_status_running_in_dto() {
        let json = r#"{
            "name": "my-chain",
            "run_type": "chain",
            "status": "running"
        }"#;
        let dto: RunDto = serde_json::from_str(json).unwrap();
        assert_eq!(dto.status, RunStatus::Running);
    }
}
