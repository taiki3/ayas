use axum::extract::{Path, State};
use axum::{Json, Router, routing::{get, post}};
use chrono::Utc;
use uuid::Uuid;

use ayas_smith::duckdb_store::DuckDbStore;
use ayas_smith::store::SmithStore;
use ayas_smith::types::Project;

use crate::error::AppError;
use crate::run_types::CreateProjectRequest;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/projects", post(create_project).get(list_projects))
        .route("/projects/{id}", get(get_project).delete(delete_project))
}

async fn create_project(
    State(state): State<AppState>,
    Json(req): Json<CreateProjectRequest>,
) -> Result<Json<Project>, AppError> {
    let store = DuckDbStore::new(&state.smith_base_dir);
    let project = Project {
        id: Uuid::new_v4(),
        name: req.name,
        description: req.description,
        created_at: Utc::now(),
    };
    store
        .create_project(&project)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Json(project))
}

async fn list_projects(
    State(state): State<AppState>,
) -> Result<Json<Vec<Project>>, AppError> {
    let store = DuckDbStore::new(&state.smith_base_dir);
    let projects = store
        .list_projects()
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Json(projects))
}

async fn get_project(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Project>, AppError> {
    let store = DuckDbStore::new(&state.smith_base_dir);
    let project = store
        .get_project(id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    match project {
        Some(p) => Ok(Json(p)),
        None => Err(AppError::NotFound(format!("Project {id} not found"))),
    }
}

async fn delete_project(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, AppError> {
    let store = DuckDbStore::new(&state.smith_base_dir);
    store
        .delete_project(id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Json(serde_json::json!({ "deleted": true })))
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
    async fn create_and_list_projects() {
        let dir = tempfile::tempdir().unwrap();
        let app = test_app(dir.path());

        // Create a project
        let body = serde_json::json!({
            "name": "my-project",
            "description": "Test project"
        });
        let req = Request::builder()
            .method("POST")
            .uri("/api/projects")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let project: Project = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(project.name, "my-project");
        assert_eq!(project.description.as_deref(), Some("Test project"));

        // List projects
        let app = test_app(dir.path());
        let req = Request::builder()
            .method("GET")
            .uri("/api/projects")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let projects: Vec<Project> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].name, "my-project");
    }

    #[tokio::test]
    async fn get_project_success() {
        let dir = tempfile::tempdir().unwrap();

        // Create first
        let app = test_app(dir.path());
        let body = serde_json::json!({ "name": "proj-1" });
        let req = Request::builder()
            .method("POST")
            .uri("/api/projects")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let created: Project = serde_json::from_slice(&bytes).unwrap();

        // Get by ID
        let app = test_app(dir.path());
        let req = Request::builder()
            .method("GET")
            .uri(format!("/api/projects/{}", created.id))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let fetched: Project = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(fetched.id, created.id);
        assert_eq!(fetched.name, "proj-1");
    }

    #[tokio::test]
    async fn get_project_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let app = test_app(dir.path());
        let fake_id = Uuid::new_v4();

        let req = Request::builder()
            .method("GET")
            .uri(format!("/api/projects/{fake_id}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn delete_project_success() {
        let dir = tempfile::tempdir().unwrap();

        // Create
        let app = test_app(dir.path());
        let body = serde_json::json!({ "name": "to-delete" });
        let req = Request::builder()
            .method("POST")
            .uri("/api/projects")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let created: Project = serde_json::from_slice(&bytes).unwrap();

        // Delete
        let app = test_app(dir.path());
        let req = Request::builder()
            .method("DELETE")
            .uri(format!("/api/projects/{}", created.id))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Verify deleted
        let app = test_app(dir.path());
        let req = Request::builder()
            .method("GET")
            .uri(format!("/api/projects/{}", created.id))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
