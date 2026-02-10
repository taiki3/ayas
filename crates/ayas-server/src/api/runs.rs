use axum::extract::{Path, Query, State};
use axum::{Json, Router, routing::{get, post}};
use uuid::Uuid;

use ayas_smith::client::flush_runs;
use ayas_smith::query::SmithQuery;
use ayas_smith::types::RunFilter;

use crate::error::AppError;
use crate::run_types::{
    BatchIngestRequest, BatchIngestResponse, ProjectQuery, RunDto, RunFilterRequest, RunSummary,
    StatsResponse,
};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/runs/batch", post(batch_ingest))
        .route("/runs/query", post(query_runs))
        .route("/runs/stats", get(get_stats))
        .route("/runs/{id}", get(get_run))
        .route("/runs/trace/{trace_id}", get(get_trace))
}

async fn batch_ingest(
    State(state): State<AppState>,
    Json(req): Json<BatchIngestRequest>,
) -> Result<Json<BatchIngestResponse>, AppError> {
    let count = req.runs.len();
    if count == 0 {
        return Ok(Json(BatchIngestResponse { ingested: 0 }));
    }

    let base_dir = state.smith_base_dir.clone();
    let runs: Vec<ayas_smith::types::Run> = req.runs.into_iter().map(|dto| dto.into()).collect();

    // Group runs by project and flush synchronously
    let mut by_project: std::collections::HashMap<String, Vec<ayas_smith::types::Run>> =
        std::collections::HashMap::new();
    for run in runs {
        by_project
            .entry(run.project.clone())
            .or_default()
            .push(run);
    }

    for (project, runs) in &by_project {
        flush_runs(runs, &base_dir, project).map_err(|e| AppError::Internal(e.to_string()))?;
    }

    Ok(Json(BatchIngestResponse { ingested: count }))
}

async fn query_runs(
    State(state): State<AppState>,
    Json(req): Json<RunFilterRequest>,
) -> Result<Json<Vec<RunSummary>>, AppError> {
    let base_dir = state.smith_base_dir.clone();
    let query = SmithQuery::new(&base_dir).map_err(|e| AppError::Internal(e.to_string()))?;
    let filter: RunFilter = req.into();
    let runs = query
        .list_runs(&filter)
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let summaries: Vec<RunSummary> = runs.iter().map(RunSummary::from).collect();
    Ok(Json(summaries))
}

async fn get_run(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<ProjectQuery>,
) -> Result<Json<RunDto>, AppError> {
    let base_dir = state.smith_base_dir.clone();
    let query = SmithQuery::new(&base_dir).map_err(|e| AppError::Internal(e.to_string()))?;
    let run = query
        .get_run(id, &q.project)
        .map_err(|e| AppError::Internal(e.to_string()))?;
    match run {
        Some(r) => Ok(Json(RunDto::from(&r))),
        None => Err(AppError::NotFound(format!("Run {id} not found"))),
    }
}

async fn get_trace(
    State(state): State<AppState>,
    Path(trace_id): Path<Uuid>,
    Query(q): Query<ProjectQuery>,
) -> Result<Json<Vec<RunDto>>, AppError> {
    let base_dir = state.smith_base_dir.clone();
    let query = SmithQuery::new(&base_dir).map_err(|e| AppError::Internal(e.to_string()))?;
    let runs = query
        .get_trace(trace_id, &q.project)
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let dtos: Vec<RunDto> = runs.iter().map(RunDto::from).collect();
    Ok(Json(dtos))
}

async fn get_stats(
    State(state): State<AppState>,
    Query(req): Query<RunFilterRequest>,
) -> Result<Json<StatsResponse>, AppError> {
    let base_dir = state.smith_base_dir.clone();
    let query = SmithQuery::new(&base_dir).map_err(|e| AppError::Internal(e.to_string()))?;
    let filter: RunFilter = req.into();
    let tokens = query
        .token_usage_summary(&filter)
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let latency = query
        .latency_percentiles(&filter)
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Json(StatsResponse { tokens, latency }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use ayas_smith::client::flush_runs;
    use ayas_smith::types::{Run, RunType};

    fn test_app(base_dir: &std::path::Path) -> Router {
        let state = AppState::with_smith_dir(base_dir.to_path_buf());
        Router::new().nest("/api", routes().with_state(state))
    }

    fn create_test_runs(dir: &std::path::Path) -> Vec<Run> {
        let trace_id = Uuid::new_v4();
        let root_id = Uuid::new_v4();

        let mut root = Run::builder("my-chain", RunType::Chain)
            .run_id(root_id)
            .trace_id(trace_id)
            .project("test-proj")
            .input(r#"{"query": "hello"}"#)
            .finish_ok(r#"{"answer": "world"}"#);
        root.trace_id = trace_id;
        root.run_id = root_id;

        let mut child_llm = Run::builder("gpt-4o", RunType::Llm)
            .parent_run_id(root_id)
            .trace_id(trace_id)
            .project("test-proj")
            .finish_llm(r#""Hello!""#, 50, 10, 60);
        child_llm.trace_id = trace_id;

        let mut child_tool = Run::builder("calculator", RunType::Tool)
            .parent_run_id(root_id)
            .trace_id(trace_id)
            .project("test-proj")
            .input(r#"{"expression": "2+3"}"#)
            .finish_ok("5");
        child_tool.trace_id = trace_id;

        let runs = vec![root, child_llm, child_tool];
        flush_runs(&runs, dir, "test-proj").unwrap();
        runs
    }

    #[tokio::test]
    async fn batch_ingest_success() {
        let dir = tempfile::tempdir().unwrap();
        let app = test_app(dir.path());

        let body = serde_json::json!({
            "runs": [
                {
                    "name": "test-chain",
                    "run_type": "chain",
                    "project": "batch-proj",
                    "input": "{}",
                    "output": "{\"result\": \"ok\"}"
                },
                {
                    "name": "test-llm",
                    "run_type": "llm",
                    "project": "batch-proj",
                    "input_tokens": 10,
                    "output_tokens": 5,
                    "total_tokens": 15
                }
            ]
        });

        let req = Request::builder()
            .method("POST")
            .uri("/api/runs/batch")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let result: BatchIngestResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(result.ingested, 2);

        // Verify files were written
        assert!(dir.path().join("batch-proj").exists());
    }

    #[tokio::test]
    async fn batch_ingest_empty() {
        let dir = tempfile::tempdir().unwrap();
        let app = test_app(dir.path());

        let body = serde_json::json!({ "runs": [] });
        let req = Request::builder()
            .method("POST")
            .uri("/api/runs/batch")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let result: BatchIngestResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(result.ingested, 0);
    }

    #[tokio::test]
    async fn query_runs_success() {
        let dir = tempfile::tempdir().unwrap();
        create_test_runs(dir.path());
        let app = test_app(dir.path());

        let body = serde_json::json!({
            "project": "test-proj"
        });
        let req = Request::builder()
            .method("POST")
            .uri("/api/runs/query")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let result: Vec<RunSummary> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(result.len(), 3);
    }

    #[tokio::test]
    async fn query_runs_with_filter() {
        let dir = tempfile::tempdir().unwrap();
        create_test_runs(dir.path());
        let app = test_app(dir.path());

        let body = serde_json::json!({
            "project": "test-proj",
            "run_type": "llm"
        });
        let req = Request::builder()
            .method("POST")
            .uri("/api/runs/query")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let result: Vec<RunSummary> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "gpt-4o");
    }

    #[tokio::test]
    async fn get_run_success() {
        let dir = tempfile::tempdir().unwrap();
        let runs = create_test_runs(dir.path());
        let app = test_app(dir.path());
        let target_id = runs[0].run_id;

        let req = Request::builder()
            .method("GET")
            .uri(format!("/api/runs/{target_id}?project=test-proj"))
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let result: RunDto = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(result.run_id, target_id);
        assert_eq!(result.name, "my-chain");
    }

    #[tokio::test]
    async fn get_run_not_found() {
        let dir = tempfile::tempdir().unwrap();
        create_test_runs(dir.path());
        let app = test_app(dir.path());
        let fake_id = Uuid::new_v4();

        let req = Request::builder()
            .method("GET")
            .uri(format!("/api/runs/{fake_id}?project=test-proj"))
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn get_trace_success() {
        let dir = tempfile::tempdir().unwrap();
        let runs = create_test_runs(dir.path());
        let app = test_app(dir.path());
        let trace_id = runs[0].trace_id;

        let req = Request::builder()
            .method("GET")
            .uri(format!("/api/runs/trace/{trace_id}?project=test-proj"))
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let result: Vec<RunDto> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(result.len(), 3);
        for dto in &result {
            assert_eq!(dto.trace_id, Some(trace_id));
        }
    }

    #[tokio::test]
    async fn get_stats_success() {
        let dir = tempfile::tempdir().unwrap();
        create_test_runs(dir.path());
        let app = test_app(dir.path());

        let req = Request::builder()
            .method("GET")
            .uri("/api/runs/stats?project=test-proj")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let result: StatsResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(result.tokens.total_input_tokens, 50);
        assert_eq!(result.tokens.total_output_tokens, 10);
        assert!(result.latency.p50 >= 0.0);
    }
}
