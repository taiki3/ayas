use std::path::{Path, PathBuf};

use axum::extract::State;
use axum::{Json, Router, routing::post};

use crate::error::AppError;
use crate::run_types::{
    Feedback, FeedbackQueryRequest, FeedbackRequest, FeedbackResponse,
};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/feedback", post(submit_feedback))
        .route("/feedback/query", post(query_feedback))
}

fn feedback_dir(base: &Path) -> PathBuf {
    base.join("_feedback")
}

fn feedback_file(base: &Path) -> PathBuf {
    feedback_dir(base).join("feedback.json")
}

fn load_feedback(base: &Path) -> Result<Vec<Feedback>, AppError> {
    let path = feedback_file(base);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let data = std::fs::read_to_string(&path).map_err(|e| AppError::Internal(e.to_string()))?;
    let items: Vec<Feedback> =
        serde_json::from_str(&data).map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(items)
}

fn save_feedback(base: &Path, items: &[Feedback]) -> Result<(), AppError> {
    let dir = feedback_dir(base);
    std::fs::create_dir_all(&dir).map_err(|e| AppError::Internal(e.to_string()))?;
    let data =
        serde_json::to_string_pretty(items).map_err(|e| AppError::Internal(e.to_string()))?;
    std::fs::write(feedback_file(base), data).map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(())
}

async fn submit_feedback(
    State(state): State<AppState>,
    Json(req): Json<FeedbackRequest>,
) -> Result<Json<FeedbackResponse>, AppError> {
    let base = &state.smith_base_dir;
    let feedback = Feedback {
        id: uuid::Uuid::new_v4(),
        run_id: req.run_id,
        key: req.key.clone(),
        score: req.score,
        comment: req.comment,
        created_at: chrono::Utc::now(),
    };

    let mut items = load_feedback(base)?;
    items.push(feedback.clone());
    save_feedback(base, &items)?;

    Ok(Json(FeedbackResponse {
        id: feedback.id,
        run_id: feedback.run_id,
        key: feedback.key,
        score: feedback.score,
    }))
}

async fn query_feedback(
    State(state): State<AppState>,
    Json(req): Json<FeedbackQueryRequest>,
) -> Result<Json<Vec<Feedback>>, AppError> {
    let items = load_feedback(&state.smith_base_dir)?;
    let filtered: Vec<Feedback> = items
        .into_iter()
        .filter(|f| {
            if let Some(ref run_id) = req.run_id {
                if f.run_id != *run_id {
                    return false;
                }
            }
            if let Some(ref key) = req.key {
                if f.key != *key {
                    return false;
                }
            }
            true
        })
        .collect();
    Ok(Json(filtered))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    use uuid::Uuid;

    fn test_app(base_dir: &std::path::Path) -> Router {
        let state = AppState::with_smith_dir(base_dir.to_path_buf());
        Router::new().nest("/api", routes().with_state(state))
    }

    #[tokio::test]
    async fn submit_feedback_success() {
        let dir = tempfile::tempdir().unwrap();
        let app = test_app(dir.path());
        let run_id = Uuid::new_v4();

        let body = serde_json::json!({
            "run_id": run_id,
            "key": "correctness",
            "score": 0.9,
            "comment": "Good answer"
        });

        let req = Request::builder()
            .method("POST")
            .uri("/api/feedback")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let result: FeedbackResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(result.run_id, run_id);
        assert_eq!(result.key, "correctness");
        assert!((result.score - 0.9).abs() < f64::EPSILON);

        // Verify file was written
        assert!(dir.path().join("_feedback").join("feedback.json").exists());
    }

    #[tokio::test]
    async fn query_feedback_empty() {
        let dir = tempfile::tempdir().unwrap();
        let app = test_app(dir.path());

        let body = serde_json::json!({});
        let req = Request::builder()
            .method("POST")
            .uri("/api/feedback/query")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let result: Vec<Feedback> = serde_json::from_slice(&bytes).unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn submit_and_query_feedback() {
        let dir = tempfile::tempdir().unwrap();
        let run_id = Uuid::new_v4();

        // Submit two feedbacks
        {
            let app = test_app(dir.path());
            let body = serde_json::json!({
                "run_id": run_id,
                "key": "correctness",
                "score": 0.9
            });
            let req = Request::builder()
                .method("POST")
                .uri("/api/feedback")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap();
            let resp = app.oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
        }
        {
            let app = test_app(dir.path());
            let body = serde_json::json!({
                "run_id": run_id,
                "key": "helpfulness",
                "score": 0.8
            });
            let req = Request::builder()
                .method("POST")
                .uri("/api/feedback")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap();
            let resp = app.oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
        }

        // Query all for this run_id
        {
            let app = test_app(dir.path());
            let body = serde_json::json!({ "run_id": run_id });
            let req = Request::builder()
                .method("POST")
                .uri("/api/feedback/query")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap();
            let resp = app.oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);

            let bytes = resp.into_body().collect().await.unwrap().to_bytes();
            let result: Vec<Feedback> = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(result.len(), 2);
        }

        // Query by key
        {
            let app = test_app(dir.path());
            let body = serde_json::json!({ "key": "correctness" });
            let req = Request::builder()
                .method("POST")
                .uri("/api/feedback/query")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap();
            let resp = app.oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);

            let bytes = resp.into_body().collect().await.unwrap().to_bytes();
            let result: Vec<Feedback> = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(result.len(), 1);
            assert_eq!(result[0].key, "correctness");
        }
    }

    #[tokio::test]
    async fn query_feedback_no_match() {
        let dir = tempfile::tempdir().unwrap();
        // Submit one feedback
        {
            let app = test_app(dir.path());
            let body = serde_json::json!({
                "run_id": Uuid::new_v4(),
                "key": "correctness",
                "score": 1.0
            });
            let req = Request::builder()
                .method("POST")
                .uri("/api/feedback")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap();
            let resp = app.oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
        }

        // Query with different run_id
        {
            let app = test_app(dir.path());
            let body = serde_json::json!({ "run_id": Uuid::new_v4() });
            let req = Request::builder()
                .method("POST")
                .uri("/api/feedback/query")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap();
            let resp = app.oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);

            let bytes = resp.into_body().collect().await.unwrap().to_bytes();
            let result: Vec<Feedback> = serde_json::from_slice(&bytes).unwrap();
            assert!(result.is_empty());
        }
    }
}
