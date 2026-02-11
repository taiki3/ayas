use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use ayas_core::error::{AyasError, GraphError, ModelError};

/// Application error type that maps to HTTP responses.
#[derive(Debug)]
pub enum AppError {
    MissingApiKey(String),
    BadRequest(String),
    Ayas(AyasError),
    Internal(String),
    NotFound(String),
}

impl From<AyasError> for AppError {
    fn from(err: AyasError) -> Self {
        AppError::Ayas(err)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            AppError::MissingApiKey(provider) => (
                StatusCode::BAD_REQUEST,
                format!("Missing API key for {provider}"),
            ),
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            AppError::Ayas(AyasError::Model(ModelError::Auth(msg))) => {
                (StatusCode::UNAUTHORIZED, msg.clone())
            }
            AppError::Ayas(AyasError::Model(ModelError::RateLimited { .. })) => {
                (StatusCode::TOO_MANY_REQUESTS, "Rate limited".into())
            }
            AppError::Ayas(AyasError::Graph(GraphError::RecursionLimit { limit })) => (
                StatusCode::BAD_REQUEST,
                format!("Recursion limit ({limit}) exceeded"),
            ),
            AppError::Ayas(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()),
            AppError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg.clone()),
            AppError::NotFound(msg) => (StatusCode::NOT_FOUND, msg.clone()),
        };

        let body = json!({ "error": message });
        (status, axum::Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_api_key_returns_400() {
        let err = AppError::MissingApiKey("gemini".into());
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn auth_error_returns_401() {
        let err = AppError::Ayas(AyasError::Model(ModelError::Auth("bad key".into())));
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn rate_limited_returns_429() {
        let err = AppError::Ayas(AyasError::Model(ModelError::RateLimited {
            retry_after_secs: None,
        }));
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[test]
    fn recursion_limit_returns_400() {
        let err = AppError::Ayas(AyasError::Graph(GraphError::RecursionLimit { limit: 25 }));
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn generic_error_returns_500() {
        let err = AppError::Ayas(AyasError::Other("something broke".into()));
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
