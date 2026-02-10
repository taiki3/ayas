use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use axum::body::Body;
use axum::http::{Request, Response};
use tower::{Layer, Service};

use ayas_smith::client::SmithClient;
use ayas_smith::types::{Run, RunType};

/// Tower layer that auto-traces HTTP requests to ayas-smith.
///
/// Tracing is activated when either:
/// - The `X-Trace-Enabled: true` (or `1`) request header is present, or
/// - The `AYAS_TRACING_ENABLED` environment variable is `true` or `1`.
#[derive(Clone)]
pub struct TracingLayer {
    client: SmithClient,
}

impl TracingLayer {
    pub fn new(client: SmithClient) -> Self {
        Self { client }
    }
}

impl<S> Layer<S> for TracingLayer {
    type Service = TracingService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        TracingService {
            inner,
            client: self.client.clone(),
        }
    }
}

#[derive(Clone)]
pub struct TracingService<S> {
    inner: S,
    client: SmithClient,
}

impl<S, ResBody> Service<Request<Body>> for TracingService<S>
where
    S: Service<Request<Body>, Response = Response<ResBody>> + Clone + Send + 'static,
    S::Error: Send,
    S::Future: Send,
    ResBody: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        let client = self.client.clone();
        let mut inner = self.inner.clone();
        // swap to ensure inner is ready (standard tower pattern)
        std::mem::swap(&mut self.inner, &mut inner);

        let trace_header = req
            .headers()
            .get("x-trace-enabled")
            .and_then(|v| v.to_str().ok())
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);

        let trace_env = std::env::var("AYAS_TRACING_ENABLED")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);

        let trace_enabled = (trace_header || trace_env) && client.is_enabled();

        let method = req.method().to_string();
        let path = req.uri().path().to_string();

        Box::pin(async move {
            if !trace_enabled {
                return inner.call(req).await;
            }

            let builder = Run::builder(
                format!("{method} {path}"),
                RunType::Chain,
            )
            .project(client.project().to_string())
            .input(
                serde_json::json!({"method": &method, "path": &path}).to_string(),
            )
            .metadata(
                serde_json::json!({"source": "auto-tracing"}).to_string(),
            );

            let result = inner.call(req).await;

            match &result {
                Ok(_resp) => {
                    let run = builder.finish_ok(
                        serde_json::json!({"status": "ok"}).to_string(),
                    );
                    client.submit_run(run);
                }
                Err(_) => {
                    let run = builder.finish_err("internal_error");
                    client.submit_run(run);
                }
            }

            result
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use axum::Router;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn test_app(client: SmithClient) -> Router {
        let app = Router::new().route("/health", get(|| async { "ok" }));
        app.layer(TracingLayer::new(client))
    }

    #[tokio::test]
    async fn tracing_disabled_passes_through() {
        let client = SmithClient::noop();
        let app = test_app(client);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"ok");
    }

    #[tokio::test]
    async fn tracing_header_activates() {
        let dir = tempfile::tempdir().unwrap();
        let config = ayas_smith::client::SmithConfig::default()
            .with_base_dir(dir.path())
            .with_project("tracing-test");
        let client = SmithClient::new(config);
        let app = test_app(client);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .header("x-trace-enabled", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);

        // Give background writer time to flush
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let project_dir = dir.path().join("tracing-test");
        assert!(project_dir.exists(), "Expected trace data directory");
    }

    #[tokio::test]
    async fn tracing_noop_client_header_ignored() {
        let client = SmithClient::noop();
        let app = test_app(client);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .header("x-trace-enabled", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn tracing_header_value_one() {
        let dir = tempfile::tempdir().unwrap();
        let config = ayas_smith::client::SmithConfig::default()
            .with_base_dir(dir.path())
            .with_project("val-one-test");
        let client = SmithClient::new(config);
        let app = test_app(client);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .header("x-trace-enabled", "1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
    }
}
