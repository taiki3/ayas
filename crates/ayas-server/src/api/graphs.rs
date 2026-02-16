use std::path::Path;

use axum::extract::{Path as AxumPath, State};
use axum::{Json, Router, routing::{delete, get, post, put}};
use duckdb::Connection;
use uuid::Uuid;

use crate::error::AppError;
use crate::state::AppState;
use crate::types::{
    GraphData, GraphListItem, SaveGraphRequest, SaveGraphResponse, SavedGraph, UpdateGraphRequest,
};

fn open_db(base: &Path) -> Result<Connection, AppError> {
    let meta_dir = base.join("_meta");
    std::fs::create_dir_all(&meta_dir)
        .map_err(|e| AppError::Internal(format!("Failed to create _meta dir: {e}")))?;
    let db_path = meta_dir.join("graphs.duckdb");
    let conn = Connection::open(db_path)
        .map_err(|e| AppError::Internal(format!("Failed to open DuckDB: {e}")))?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS saved_graphs (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            description TEXT,
            graph_data TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )",
    )
    .map_err(|e| AppError::Internal(format!("Failed to create table: {e}")))?;
    Ok(conn)
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/graphs", post(save_graph))
        .route("/graphs", get(list_graphs))
        .route("/graphs/{id}", get(get_graph))
        .route("/graphs/{id}", put(update_graph))
        .route("/graphs/{id}", delete(delete_graph))
}

async fn save_graph(
    State(state): State<AppState>,
    Json(req): Json<SaveGraphRequest>,
) -> Result<Json<SaveGraphResponse>, AppError> {
    let conn = open_db(&state.smith_base_dir)?;
    let id = Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    let graph_data_json = serde_json::to_string(&req.graph_data)
        .map_err(|e| AppError::Internal(format!("Failed to serialize graph_data: {e}")))?;

    conn.execute(
        "INSERT INTO saved_graphs (id, name, description, graph_data, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?)",
        duckdb::params![id, req.name, req.description, graph_data_json, now, now],
    )
    .map_err(|e| AppError::Internal(format!("Failed to insert graph: {e}")))?;

    Ok(Json(SaveGraphResponse {
        id,
        name: req.name,
        created_at: now,
    }))
}

async fn list_graphs(
    State(state): State<AppState>,
) -> Result<Json<Vec<GraphListItem>>, AppError> {
    let conn = open_db(&state.smith_base_dir)?;
    let mut stmt = conn
        .prepare("SELECT id, name, description, created_at, updated_at FROM saved_graphs ORDER BY updated_at DESC")
        .map_err(|e| AppError::Internal(format!("Failed to prepare query: {e}")))?;

    let rows = stmt
        .query_map([], |row| {
            Ok(GraphListItem {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
            })
        })
        .map_err(|e| AppError::Internal(format!("Failed to query graphs: {e}")))?;

    let mut items = Vec::new();
    for row in rows {
        items.push(row.map_err(|e| AppError::Internal(format!("Row error: {e}")))?);
    }
    Ok(Json(items))
}

async fn get_graph(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<SavedGraph>, AppError> {
    let conn = open_db(&state.smith_base_dir)?;
    let mut stmt = conn
        .prepare("SELECT id, name, description, graph_data, created_at, updated_at FROM saved_graphs WHERE id = ?")
        .map_err(|e| AppError::Internal(format!("Failed to prepare query: {e}")))?;

    let mut rows = stmt
        .query_map(duckdb::params![id], |row| {
            let graph_data_str: String = row.get(3)?;
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                graph_data_str,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })
        .map_err(|e| AppError::Internal(format!("Failed to query graph: {e}")))?;

    match rows.next() {
        Some(Ok((id, name, description, graph_data_str, created_at, updated_at))) => {
            let graph_data: GraphData = serde_json::from_str(&graph_data_str)
                .map_err(|e| AppError::Internal(format!("Failed to parse graph_data: {e}")))?;
            Ok(Json(SavedGraph {
                id,
                name,
                description,
                graph_data,
                created_at,
                updated_at,
            }))
        }
        Some(Err(e)) => Err(AppError::Internal(format!("Row error: {e}"))),
        None => Err(AppError::NotFound("Graph not found".into())),
    }
}

async fn update_graph(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Json(req): Json<UpdateGraphRequest>,
) -> Result<Json<SavedGraph>, AppError> {
    let conn = open_db(&state.smith_base_dir)?;
    let now = chrono::Utc::now().to_rfc3339();

    // Fetch existing
    let mut stmt = conn
        .prepare("SELECT name, description, graph_data FROM saved_graphs WHERE id = ?")
        .map_err(|e| AppError::Internal(format!("Failed to prepare query: {e}")))?;

    let mut rows = stmt
        .query_map(duckdb::params![id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|e| AppError::Internal(format!("Failed to query graph: {e}")))?;

    let (existing_name, existing_desc, existing_data_str) = match rows.next() {
        Some(Ok(v)) => v,
        Some(Err(e)) => return Err(AppError::Internal(format!("Row error: {e}"))),
        None => return Err(AppError::NotFound("Graph not found".into())),
    };
    drop(rows);
    drop(stmt);

    let name = req.name.unwrap_or(existing_name);
    let description = if req.description.is_some() {
        req.description
    } else {
        existing_desc
    };
    let graph_data_json = if let Some(data) = &req.graph_data {
        serde_json::to_string(data)
            .map_err(|e| AppError::Internal(format!("Failed to serialize graph_data: {e}")))?
    } else {
        existing_data_str
    };

    conn.execute(
        "UPDATE saved_graphs SET name = ?, description = ?, graph_data = ?, updated_at = ? WHERE id = ?",
        duckdb::params![name, description, graph_data_json, now, id],
    )
    .map_err(|e| AppError::Internal(format!("Failed to update graph: {e}")))?;

    let graph_data: GraphData = serde_json::from_str(&graph_data_json)
        .map_err(|e| AppError::Internal(format!("Failed to parse graph_data: {e}")))?;

    Ok(Json(SavedGraph {
        id,
        name,
        description,
        graph_data,
        created_at: now.clone(),
        updated_at: now,
    }))
}

async fn delete_graph(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let conn = open_db(&state.smith_base_dir)?;

    // Check existence first
    let mut stmt = conn
        .prepare("SELECT COUNT(*) FROM saved_graphs WHERE id = ?")
        .map_err(|e| AppError::Internal(format!("Failed to prepare query: {e}")))?;

    let count: i64 = stmt
        .query_row(duckdb::params![id], |row| row.get(0))
        .map_err(|e| AppError::Internal(format!("Failed to query: {e}")))?;

    if count == 0 {
        return Err(AppError::NotFound("Graph not found".into()));
    }

    conn.execute(
        "DELETE FROM saved_graphs WHERE id = ?",
        duckdb::params![id],
    )
    .map_err(|e| AppError::Internal(format!("Failed to delete graph: {e}")))?;

    Ok(Json(serde_json::json!({"deleted": true})))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use crate::api;
    use crate::state::AppState;
    use crate::types::{SaveGraphResponse, SavedGraph, GraphListItem};

    fn test_app() -> (axum::Router, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let state = AppState::with_smith_dir(tmp.path().to_path_buf());
        let app = api::api_routes(state);
        (app, tmp)
    }

    fn sample_graph_data() -> serde_json::Value {
        serde_json::json!({
            "nodes": [
                {"id": "start", "type": "start", "position": {"x": 250.0, "y": 0.0}},
                {"id": "llm_1", "type": "llm", "label": "My LLM", "config": {"provider": "gemini"}, "position": {"x": 200.0, "y": 150.0}},
                {"id": "end", "type": "end", "position": {"x": 250.0, "y": 300.0}}
            ],
            "edges": [
                {"from": "start", "to": "llm_1"},
                {"from": "llm_1", "to": "end"}
            ],
            "channels": [
                {"key": "value", "type": "LastValue"}
            ],
            "node_counter": 2
        })
    }

    async fn body_json<T: serde::de::DeserializeOwned>(body: Body) -> T {
        let bytes = body.collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn test_save_graph() {
        let (app, _tmp) = test_app();
        let req = Request::builder()
            .method("POST")
            .uri("/api/graphs")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&serde_json::json!({
                "name": "Test Graph",
                "description": "A test graph",
                "graph_data": sample_graph_data()
            })).unwrap()))
            .unwrap();

        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body: SaveGraphResponse = body_json(res.into_body()).await;
        assert_eq!(body.name, "Test Graph");
        assert!(!body.id.is_empty());
    }

    #[tokio::test]
    async fn test_list_empty() {
        let (app, _tmp) = test_app();
        let req = Request::builder()
            .uri("/api/graphs")
            .body(Body::empty())
            .unwrap();

        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body: Vec<GraphListItem> = body_json(res.into_body()).await;
        assert!(body.is_empty());
    }

    #[tokio::test]
    async fn test_save_and_list() {
        let (app, _tmp) = test_app();

        // Save
        let save_req = Request::builder()
            .method("POST")
            .uri("/api/graphs")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&serde_json::json!({
                "name": "Graph 1",
                "graph_data": sample_graph_data()
            })).unwrap()))
            .unwrap();
        let res = app.clone().oneshot(save_req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        // List
        let list_req = Request::builder()
            .uri("/api/graphs")
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(list_req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body: Vec<GraphListItem> = body_json(res.into_body()).await;
        assert_eq!(body.len(), 1);
        assert_eq!(body[0].name, "Graph 1");
    }

    #[tokio::test]
    async fn test_get_graph() {
        let (app, _tmp) = test_app();

        // Save
        let save_req = Request::builder()
            .method("POST")
            .uri("/api/graphs")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&serde_json::json!({
                "name": "Get Test",
                "description": "desc",
                "graph_data": sample_graph_data()
            })).unwrap()))
            .unwrap();
        let res = app.clone().oneshot(save_req).await.unwrap();
        let saved: SaveGraphResponse = body_json(res.into_body()).await;

        // Get
        let get_req = Request::builder()
            .uri(format!("/api/graphs/{}", saved.id))
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(get_req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body: SavedGraph = body_json(res.into_body()).await;
        assert_eq!(body.name, "Get Test");
        assert_eq!(body.description.as_deref(), Some("desc"));
        assert_eq!(body.graph_data.nodes.len(), 3);
    }

    #[tokio::test]
    async fn test_get_not_found() {
        let (app, _tmp) = test_app();
        let req = Request::builder()
            .uri("/api/graphs/nonexistent-id")
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_update_graph() {
        let (app, _tmp) = test_app();

        // Save
        let save_req = Request::builder()
            .method("POST")
            .uri("/api/graphs")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&serde_json::json!({
                "name": "Original",
                "graph_data": sample_graph_data()
            })).unwrap()))
            .unwrap();
        let res = app.clone().oneshot(save_req).await.unwrap();
        let saved: SaveGraphResponse = body_json(res.into_body()).await;

        // Update
        let update_req = Request::builder()
            .method("PUT")
            .uri(format!("/api/graphs/{}", saved.id))
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&serde_json::json!({
                "name": "Updated",
                "description": "new desc"
            })).unwrap()))
            .unwrap();
        let res = app.clone().oneshot(update_req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body: SavedGraph = body_json(res.into_body()).await;
        assert_eq!(body.name, "Updated");
        assert_eq!(body.description.as_deref(), Some("new desc"));

        // Verify via get
        let get_req = Request::builder()
            .uri(format!("/api/graphs/{}", saved.id))
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(get_req).await.unwrap();
        let body: SavedGraph = body_json(res.into_body()).await;
        assert_eq!(body.name, "Updated");
    }

    #[tokio::test]
    async fn test_delete_graph() {
        let (app, _tmp) = test_app();

        // Save
        let save_req = Request::builder()
            .method("POST")
            .uri("/api/graphs")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&serde_json::json!({
                "name": "To Delete",
                "graph_data": sample_graph_data()
            })).unwrap()))
            .unwrap();
        let res = app.clone().oneshot(save_req).await.unwrap();
        let saved: SaveGraphResponse = body_json(res.into_body()).await;

        // Delete
        let del_req = Request::builder()
            .method("DELETE")
            .uri(format!("/api/graphs/{}", saved.id))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(del_req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        // Verify gone
        let get_req = Request::builder()
            .uri(format!("/api/graphs/{}", saved.id))
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(get_req).await.unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_delete_not_found() {
        let (app, _tmp) = test_app();
        let req = Request::builder()
            .method("DELETE")
            .uri("/api/graphs/nonexistent-id")
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }
}
