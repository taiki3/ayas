use std::pin::Pin;

use async_trait::async_trait;
use futures::stream::StreamExt;
use futures::Stream;
use reqwest::StatusCode;
use tracing::{info, warn};

use ayas_core::error::{AyasError, ModelError, Result};

use crate::client::InteractionsClient;
use crate::types::{
    CreateInteractionRequest, Interaction, StreamEvent,
};

const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";

/// HTTP client for the Gemini Interactions API.
pub struct GeminiInteractionsClient {
    api_key: String,
    base_url: String,
    client: reqwest::Client,
}

impl GeminiInteractionsClient {
    /// Create a client with the default Gemini API base URL.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: DEFAULT_BASE_URL.into(),
            client: reqwest::Client::new(),
        }
    }

    /// Create a client with a custom base URL (for testing).
    pub fn with_base_url(api_key: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: base_url.into(),
            client: reqwest::Client::new(),
        }
    }

    fn interactions_url(&self) -> String {
        format!("{}/interactions?key={}", self.base_url, self.api_key)
    }

    fn interaction_url(&self, id: &str) -> String {
        format!("{}/interactions/{}?key={}", self.base_url, id, self.api_key)
    }

    fn map_status_error(status: StatusCode, body: String) -> AyasError {
        match status.as_u16() {
            401 | 403 => AyasError::Model(ModelError::Auth(body)),
            429 => AyasError::Model(ModelError::RateLimited {
                retry_after_secs: None,
            }),
            _ => AyasError::Model(ModelError::ApiRequest(format!(
                "HTTP {}: {}",
                status, body
            ))),
        }
    }
}

#[async_trait]
impl InteractionsClient for GeminiInteractionsClient {
    async fn create(&self, request: &CreateInteractionRequest) -> Result<Interaction> {
        info!(agent = %request.agent, background = request.background, "Sending create interaction POST");
        let response = self
            .client
            .post(self.interactions_url())
            .json(request)
            .send()
            .await
            .map_err(|e| {
                warn!(error = %e, "Create interaction POST failed");
                AyasError::Model(ModelError::ApiRequest(e.to_string()))
            })?;

        let status = response.status();
        info!(%status, "Create interaction response received");
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "failed to read response body".into());
            warn!(%status, body = %body, "Create interaction API error");
            return Err(Self::map_status_error(status, body));
        }

        response
            .json::<Interaction>()
            .await
            .map_err(|e| AyasError::Model(ModelError::InvalidResponse(e.to_string())))
    }

    async fn get(&self, interaction_id: &str) -> Result<Interaction> {
        let response = self
            .client
            .get(self.interaction_url(interaction_id))
            .send()
            .await
            .map_err(|e| AyasError::Model(ModelError::ApiRequest(e.to_string())))?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "failed to read response body".into());
            return Err(Self::map_status_error(status, body));
        }

        response
            .json::<Interaction>()
            .await
            .map_err(|e| AyasError::Model(ModelError::InvalidResponse(e.to_string())))
    }

    async fn create_stream(
        &self,
        request: &CreateInteractionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let mut stream_request = request.clone();
        stream_request.stream = Some(true);

        let response = self
            .client
            .post(self.interactions_url())
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&stream_request)
            .send()
            .await
            .map_err(|e| AyasError::Model(ModelError::ApiRequest(e.to_string())))?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "failed to read response body".into());
            return Err(Self::map_status_error(status, body));
        }

        let byte_stream = response.bytes_stream();
        let event_stream = parse_sse_stream(byte_stream);
        Ok(Box::pin(event_stream))
    }
}

type ByteStream = Pin<Box<dyn Stream<Item = std::result::Result<bytes::Bytes, reqwest::Error>> + Send>>;

/// Parse an SSE byte stream into StreamEvent items.
fn parse_sse_stream<S>(byte_stream: S) -> impl Stream<Item = Result<StreamEvent>> + Send
where
    S: Stream<Item = std::result::Result<bytes::Bytes, reqwest::Error>> + Send + 'static,
{
    let buffer = String::new();
    let pinned: ByteStream = Box::pin(byte_stream);
    futures::stream::unfold(
        (pinned, buffer),
        |(mut stream, mut buf): (ByteStream, String)| async move {
            loop {
                // Check if we have a complete SSE event in the buffer
                if let Some(event) = extract_sse_event(&mut buf) {
                    return Some((event, (stream, buf)));
                }

                // Read more data
                match stream.next().await {
                    Some(Ok(bytes)) => {
                        let text = String::from_utf8_lossy(&bytes);
                        buf.push_str(&text);
                    }
                    Some(Err(e)) => {
                        let err: Result<StreamEvent> =
                            Err(AyasError::Model(ModelError::ApiRequest(e.to_string())));
                        return Some((err, (stream, buf)));
                    }
                    None => return None,
                }
            }
        },
    )
}

/// Extract a single SSE event from the buffer, if one is complete.
fn extract_sse_event(buf: &mut String) -> Option<Result<StreamEvent>> {
    // SSE events are separated by double newlines
    let delimiter = "\n\n";
    let pos = buf.find(delimiter)?;

    let event_text = buf[..pos].to_string();
    *buf = buf[pos + delimiter.len()..].to_string();

    // Parse data lines
    let mut data = String::new();
    for line in event_text.lines() {
        if let Some(value) = line.strip_prefix("data: ") {
            if value == "[DONE]" {
                return None;
            }
            data.push_str(value);
        }
    }

    if data.is_empty() {
        return None;
    }

    Some(
        serde_json::from_str::<StreamEvent>(&data)
            .map_err(|e| AyasError::Model(ModelError::InvalidResponse(e.to_string()))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_status_error_auth() {
        let err = GeminiInteractionsClient::map_status_error(
            StatusCode::UNAUTHORIZED,
            "bad key".into(),
        );
        assert!(matches!(err, AyasError::Model(ModelError::Auth(msg)) if msg == "bad key"));
    }

    #[test]
    fn map_status_error_forbidden() {
        let err = GeminiInteractionsClient::map_status_error(
            StatusCode::FORBIDDEN,
            "no access".into(),
        );
        assert!(matches!(err, AyasError::Model(ModelError::Auth(_))));
    }

    #[test]
    fn map_status_error_rate_limited() {
        let err = GeminiInteractionsClient::map_status_error(
            StatusCode::TOO_MANY_REQUESTS,
            "slow down".into(),
        );
        assert!(matches!(
            err,
            AyasError::Model(ModelError::RateLimited { .. })
        ));
    }

    #[test]
    fn map_status_error_server_error() {
        let err = GeminiInteractionsClient::map_status_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "oops".into(),
        );
        assert!(
            matches!(err, AyasError::Model(ModelError::ApiRequest(msg)) if msg.contains("500"))
        );
    }

    #[test]
    fn extract_sse_event_parses_data_line() {
        let event_json = serde_json::json!({
            "event_type": "content.delta",
            "delta": { "type": "text", "text": "Hello" }
        });
        let mut buf = format!("data: {}\n\n", event_json);
        let result = extract_sse_event(&mut buf);
        assert!(result.is_some());
        let event = result.unwrap().unwrap();
        assert_eq!(event.event_type, crate::types::StreamEventType::ContentDelta);
        assert_eq!(
            event.delta.as_ref().unwrap().text.as_deref(),
            Some("Hello")
        );
        assert!(buf.is_empty());
    }

    #[test]
    fn extract_sse_event_returns_none_for_done() {
        let mut buf = "data: [DONE]\n\n".to_string();
        let result = extract_sse_event(&mut buf);
        assert!(result.is_none());
    }

    #[test]
    fn extract_sse_event_returns_none_for_incomplete() {
        let mut buf = "data: {\"event_type\":\"error\"}".to_string();
        let result = extract_sse_event(&mut buf);
        assert!(result.is_none());
        // Buffer should be unchanged
        assert!(buf.contains("data:"));
    }

    #[test]
    fn with_base_url_sets_custom_url() {
        let client =
            GeminiInteractionsClient::with_base_url("key", "http://localhost:8080/v1");
        assert_eq!(
            client.interactions_url(),
            "http://localhost:8080/v1/interactions?key=key"
        );
        assert_eq!(
            client.interaction_url("abc"),
            "http://localhost:8080/v1/interactions/abc?key=key"
        );
    }
}
