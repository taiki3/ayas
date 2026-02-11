use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use ayas_core::error::{AyasError, ModelError, Result};

use crate::embedding::Embedding;
use crate::types::EmbeddingVector;

/// Gemini embedding model.
pub struct GeminiEmbedding {
    client: Client,
    api_key: String,
    model: String,
    base_url: String,
}

impl GeminiEmbedding {
    /// Create a new Gemini embedding provider with default model (`text-embedding-004`).
    /// API key is read from `GEMINI_API_KEY` environment variable.
    pub fn new() -> Result<Self> {
        let api_key = std::env::var("GEMINI_API_KEY").map_err(|_| {
            AyasError::Model(ModelError::Auth(
                "GEMINI_API_KEY environment variable not set".into(),
            ))
        })?;
        Self::with_api_key(api_key)
    }

    pub fn with_api_key(api_key: String) -> Result<Self> {
        let client = Client::new();
        Ok(Self {
            client,
            api_key,
            model: "text-embedding-004".into(),
            base_url: "https://generativelanguage.googleapis.com".into(),
        })
    }

    pub fn with_model(mut self, model: String) -> Self {
        self.model = model;
        self
    }

    pub fn with_base_url(mut self, base_url: String) -> Self {
        self.base_url = base_url;
        self
    }
}

#[async_trait]
impl Embedding for GeminiEmbedding {
    async fn embed(&self, text: &str) -> Result<EmbeddingVector> {
        let url = format!(
            "{}/v1beta/models/{}:embedContent?key={}",
            self.base_url, self.model, self.api_key
        );

        let request = GeminiEmbedRequest {
            model: format!("models/{}", self.model),
            content: GeminiContent {
                parts: vec![GeminiPart {
                    text: text.to_string(),
                }],
            },
        };

        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| AyasError::Model(ModelError::ApiRequest(e.to_string())))?;

        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED
            || status == reqwest::StatusCode::FORBIDDEN
        {
            return Err(AyasError::Model(ModelError::Auth(
                "Invalid Gemini API key".into(),
            )));
        }
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(AyasError::Model(ModelError::RateLimited {
                retry_after_secs: None,
            }));
        }
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(AyasError::Model(ModelError::ApiRequest(format!(
                "Gemini API error {status}: {body}"
            ))));
        }

        let body: GeminiEmbedResponse = response
            .json()
            .await
            .map_err(|e| AyasError::Model(ModelError::InvalidResponse(e.to_string())))?;

        Ok(EmbeddingVector::new(body.embedding.values))
    }

    fn dimension(&self) -> usize {
        768 // text-embedding-004 dimension
    }
}

#[derive(Serialize)]
struct GeminiEmbedRequest {
    model: String,
    content: GeminiContent,
}

#[derive(Serialize)]
struct GeminiContent {
    parts: Vec<GeminiPart>,
}

#[derive(Serialize)]
struct GeminiPart {
    text: String,
}

#[derive(Deserialize)]
struct GeminiEmbedResponse {
    embedding: GeminiEmbeddingValues,
}

#[derive(Deserialize)]
struct GeminiEmbeddingValues {
    values: Vec<f32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_gemini_response() {
        let json = r#"{
            "embedding": {
                "values": [0.1, 0.2, 0.3, 0.4]
            }
        }"#;

        let response: GeminiEmbedResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.embedding.values, vec![0.1, 0.2, 0.3, 0.4]);
    }

    #[test]
    fn serialize_gemini_request() {
        let request = GeminiEmbedRequest {
            model: "models/text-embedding-004".into(),
            content: GeminiContent {
                parts: vec![GeminiPart {
                    text: "hello world".into(),
                }],
            },
        };

        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["model"], "models/text-embedding-004");
        assert_eq!(json["content"]["parts"][0]["text"], "hello world");
    }

    #[test]
    fn missing_api_key_errors() {
        let original = std::env::var("GEMINI_API_KEY").ok();
        unsafe { std::env::remove_var("GEMINI_API_KEY") };

        let result = GeminiEmbedding::new();
        assert!(result.is_err());

        if let Some(key) = original {
            unsafe { std::env::set_var("GEMINI_API_KEY", key) };
        }
    }

    #[test]
    fn with_api_key_succeeds() {
        let result = GeminiEmbedding::with_api_key("test-key".into());
        assert!(result.is_ok());
    }

    #[test]
    fn custom_model_and_base_url() {
        let embedding = GeminiEmbedding::with_api_key("key".into())
            .unwrap()
            .with_model("custom-model".into())
            .with_base_url("http://localhost:8080".into());
        assert_eq!(embedding.model, "custom-model");
        assert_eq!(embedding.base_url, "http://localhost:8080");
    }

    #[test]
    fn default_dimension() {
        let embedding = GeminiEmbedding::with_api_key("key".into()).unwrap();
        assert_eq!(embedding.dimension(), 768);
    }
}
