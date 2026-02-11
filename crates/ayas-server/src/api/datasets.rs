use axum::extract::{Path, Query, State};
use axum::{Json, Router, routing::post};
use chrono::Utc;
use uuid::Uuid;

use ayas_smith::duckdb_store::DuckDbStore;
use ayas_smith::store::SmithStore;
use ayas_smith::types::{Dataset, Example};

use crate::error::AppError;
use crate::run_types::{AddExamplesRequest, CreateDatasetRequest, ListDatasetsQuery};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/datasets", post(create_dataset).get(list_datasets))
        .route(
            "/datasets/{id}/examples",
            post(add_examples).get(list_examples),
        )
}

async fn create_dataset(
    State(state): State<AppState>,
    Json(req): Json<CreateDatasetRequest>,
) -> Result<Json<Dataset>, AppError> {
    let store = DuckDbStore::new(&state.smith_base_dir);
    let dataset = Dataset {
        id: Uuid::new_v4(),
        name: req.name,
        description: req.description,
        project_id: req.project_id,
        created_at: Utc::now(),
    };
    store
        .create_dataset(&dataset)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Json(dataset))
}

async fn list_datasets(
    State(state): State<AppState>,
    Query(q): Query<ListDatasetsQuery>,
) -> Result<Json<Vec<Dataset>>, AppError> {
    let store = DuckDbStore::new(&state.smith_base_dir);
    let datasets = store
        .list_datasets(q.project_id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Json(datasets))
}

async fn add_examples(
    State(state): State<AppState>,
    Path(dataset_id): Path<Uuid>,
    Json(req): Json<AddExamplesRequest>,
) -> Result<Json<Vec<Example>>, AppError> {
    let store = DuckDbStore::new(&state.smith_base_dir);
    let examples: Vec<Example> = req
        .examples
        .into_iter()
        .map(|e| Example {
            id: Uuid::new_v4(),
            dataset_id,
            input: e.input,
            output: e.output,
            metadata: e.metadata,
            created_at: Utc::now(),
        })
        .collect();

    store
        .add_examples(&examples)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Json(examples))
}

async fn list_examples(
    State(state): State<AppState>,
    Path(dataset_id): Path<Uuid>,
) -> Result<Json<Vec<Example>>, AppError> {
    let store = DuckDbStore::new(&state.smith_base_dir);
    let examples = store
        .list_examples(dataset_id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Json(examples))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn test_app(base_dir: &std::path::Path) -> Router {
        let state = AppState::with_smith_dir(base_dir.to_path_buf());
        Router::new().nest("/api", routes().with_state(state))
    }

    #[tokio::test]
    async fn create_and_list_datasets() {
        let dir = tempfile::tempdir().unwrap();
        let app = test_app(dir.path());

        let body = serde_json::json!({
            "name": "qa-dataset",
            "description": "QA evaluation set"
        });
        let req = Request::builder()
            .method("POST")
            .uri("/api/datasets")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let dataset: Dataset = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(dataset.name, "qa-dataset");

        // List
        let app = test_app(dir.path());
        let req = Request::builder()
            .method("GET")
            .uri("/api/datasets")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let datasets: Vec<Dataset> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(datasets.len(), 1);
    }

    #[tokio::test]
    async fn add_and_list_examples() {
        let dir = tempfile::tempdir().unwrap();

        // Create dataset first
        let app = test_app(dir.path());
        let body = serde_json::json!({ "name": "test-ds" });
        let req = Request::builder()
            .method("POST")
            .uri("/api/datasets")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let dataset: Dataset = serde_json::from_slice(&bytes).unwrap();

        // Add examples
        let app = test_app(dir.path());
        let body = serde_json::json!({
            "examples": [
                { "input": "{\"q\": \"What is 2+2?\"}", "output": "4" },
                { "input": "{\"q\": \"Capital of France?\"}", "output": "Paris" }
            ]
        });
        let req = Request::builder()
            .method("POST")
            .uri(format!("/api/datasets/{}/examples", dataset.id))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let examples: Vec<Example> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(examples.len(), 2);
        assert_eq!(examples[0].dataset_id, dataset.id);

        // List examples
        let app = test_app(dir.path());
        let req = Request::builder()
            .method("GET")
            .uri(format!("/api/datasets/{}/examples", dataset.id))
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let examples: Vec<Example> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(examples.len(), 2);
    }

    #[tokio::test]
    async fn list_datasets_by_project() {
        let dir = tempfile::tempdir().unwrap();
        let project_id = Uuid::new_v4();

        // Create dataset with project_id
        let app = test_app(dir.path());
        let body = serde_json::json!({
            "name": "proj-ds",
            "project_id": project_id
        });
        let req = Request::builder()
            .method("POST")
            .uri("/api/datasets")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Create dataset without project_id
        let app = test_app(dir.path());
        let body = serde_json::json!({ "name": "no-proj-ds" });
        let req = Request::builder()
            .method("POST")
            .uri("/api/datasets")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // List all
        let app = test_app(dir.path());
        let req = Request::builder()
            .method("GET")
            .uri("/api/datasets")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let all: Vec<Dataset> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(all.len(), 2);

        // List by project_id
        let app = test_app(dir.path());
        let req = Request::builder()
            .method("GET")
            .uri(format!("/api/datasets?project_id={project_id}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let filtered: Vec<Dataset> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "proj-ds");
    }

    #[tokio::test]
    async fn list_examples_empty() {
        let dir = tempfile::tempdir().unwrap();
        let app = test_app(dir.path());
        let fake_id = Uuid::new_v4();

        let req = Request::builder()
            .method("GET")
            .uri(format!("/api/datasets/{fake_id}/examples"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let examples: Vec<Example> = serde_json::from_slice(&bytes).unwrap();
        assert!(examples.is_empty());
    }
}
