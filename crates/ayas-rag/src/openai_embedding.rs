use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use ayas_core::error::{AyasError, ModelError, Result};

use crate::embedding::Embedding;
use crate::types::EmbeddingVector;

/// OpenAI embedding model variants.
#[derive(Debug, Clone)]
pub enum OpenAiEmbeddingModel {
    TextEmbedding3Small,
    TextEmbedding3Large,
    Custom(String),
}

impl OpenAiEmbeddingModel {
    fn as_str(&self) -> &str {
        match self {
            Self::TextEmbedding3Small => "text-embedding-3-small",
            Self::TextEmbedding3Large => "text-embedding-3-large",
            Self::Custom(s) => s,
        }
    }

    fn dimension(&self) -> usize {
        match self {
            Self::TextEmbedding3Small => 1536,
            Self::TextEmbedding3Large => 3072,
            Self::Custom(_) => 1536,
        }
    }
}

/// OpenAI Embeddings provider.
pub struct OpenAiEmbedding {
    client: Client,
    api_key: String,
    model: OpenAiEmbeddingModel,
    base_url: String,
}

impl OpenAiEmbedding {
    /// Create a new OpenAI embedding provider.
    /// API key is read from `OPENAI_API_KEY` environment variable if not provided.
    pub fn new(model: OpenAiEmbeddingModel) -> Result<Self> {
        let api_key = std::env::var("OPENAI_API_KEY").map_err(|_| {
            AyasError::Model(ModelError::Auth(
                "OPENAI_API_KEY environment variable not set".into(),
            ))
        })?;
        Self::with_api_key(api_key, model)
    }

    pub fn with_api_key(api_key: String, model: OpenAiEmbeddingModel) -> Result<Self> {
        let client = Client::new();
        Ok(Self {
            client,
            api_key,
            model,
            base_url: "https://api.openai.com".into(),
        })
    }

    /// Set a custom base URL (for testing or proxied endpoints).
    pub fn with_base_url(mut self, base_url: String) -> Self {
        self.base_url = base_url;
        self
    }

    async fn call_api(&self, input: Vec<&str>) -> Result<Vec<EmbeddingVector>> {
        let request = EmbeddingRequest {
            input: input.into_iter().map(String::from).collect(),
            model: self.model.as_str().to_string(),
        };

        let response = self
            .client
            .post(format!("{}/v1/embeddings", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&request)
            .send()
            .await
            .map_err(|e| AyasError::Model(ModelError::ApiRequest(e.to_string())))?;

        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(AyasError::Model(ModelError::Auth(
                "Invalid OpenAI API key".into(),
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
                "OpenAI API error {status}: {body}"
            ))));
        }

        let body: EmbeddingResponse = response
            .json()
            .await
            .map_err(|e| AyasError::Model(ModelError::InvalidResponse(e.to_string())))?;

        let mut embeddings: Vec<(usize, EmbeddingVector)> = body
            .data
            .into_iter()
            .map(|d| (d.index, EmbeddingVector::new(d.embedding)))
            .collect();

        // Sort by index to maintain input order
        embeddings.sort_by_key(|(i, _)| *i);

        Ok(embeddings.into_iter().map(|(_, v)| v).collect())
    }
}

#[async_trait]
impl Embedding for OpenAiEmbedding {
    async fn embed(&self, text: &str) -> Result<EmbeddingVector> {
        let results = self.call_api(vec![text]).await?;
        results
            .into_iter()
            .next()
            .ok_or_else(|| AyasError::Model(ModelError::InvalidResponse("Empty response".into())))
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<EmbeddingVector>> {
        if texts.is_empty() {
            return Ok(vec![]);
        }
        self.call_api(texts.to_vec()).await
    }

    fn dimension(&self) -> usize {
        self.model.dimension()
    }
}

#[derive(Serialize)]
struct EmbeddingRequest {
    input: Vec<String>,
    model: String,
}

#[derive(Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingData>,
}

#[derive(Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
    index: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_embedding_response() {
        let json = r#"{
            "object": "list",
            "data": [
                {"object": "embedding", "embedding": [0.1, 0.2, 0.3], "index": 0},
                {"object": "embedding", "embedding": [0.4, 0.5, 0.6], "index": 1}
            ],
            "model": "text-embedding-3-small",
            "usage": {"prompt_tokens": 10, "total_tokens": 10}
        }"#;

        let response: EmbeddingResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.data.len(), 2);
        assert_eq!(response.data[0].embedding, vec![0.1, 0.2, 0.3]);
        assert_eq!(response.data[0].index, 0);
        assert_eq!(response.data[1].embedding, vec![0.4, 0.5, 0.6]);
        assert_eq!(response.data[1].index, 1);
    }

    #[test]
    fn parse_single_embedding_response() {
        let json = r#"{
            "data": [
                {"embedding": [1.0, 2.0], "index": 0}
            ]
        }"#;

        let response: EmbeddingResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.data.len(), 1);
        assert_eq!(response.data[0].embedding, vec![1.0, 2.0]);
    }

    #[test]
    fn model_names() {
        assert_eq!(
            OpenAiEmbeddingModel::TextEmbedding3Small.as_str(),
            "text-embedding-3-small"
        );
        assert_eq!(
            OpenAiEmbeddingModel::TextEmbedding3Large.as_str(),
            "text-embedding-3-large"
        );
        assert_eq!(
            OpenAiEmbeddingModel::Custom("my-model".into()).as_str(),
            "my-model"
        );
    }

    #[test]
    fn model_dimensions() {
        assert_eq!(OpenAiEmbeddingModel::TextEmbedding3Small.dimension(), 1536);
        assert_eq!(OpenAiEmbeddingModel::TextEmbedding3Large.dimension(), 3072);
    }

    #[test]
    fn missing_api_key_errors() {
        // Temporarily remove the env var if it exists
        let original = std::env::var("OPENAI_API_KEY").ok();
        unsafe { std::env::remove_var("OPENAI_API_KEY") };

        let result = OpenAiEmbedding::new(OpenAiEmbeddingModel::TextEmbedding3Small);
        assert!(result.is_err());

        // Restore if it was set
        if let Some(key) = original {
            unsafe { std::env::set_var("OPENAI_API_KEY", key) };
        }
    }

    #[test]
    fn with_api_key_succeeds() {
        let result =
            OpenAiEmbedding::with_api_key("test-key".into(), OpenAiEmbeddingModel::TextEmbedding3Small);
        assert!(result.is_ok());
    }

    #[test]
    fn custom_base_url() {
        let embedding =
            OpenAiEmbedding::with_api_key("key".into(), OpenAiEmbeddingModel::TextEmbedding3Small)
                .unwrap()
                .with_base_url("http://localhost:8080".into());
        assert_eq!(embedding.base_url, "http://localhost:8080");
    }

    #[test]
    fn response_out_of_order_indices() {
        let json = r#"{
            "data": [
                {"embedding": [0.4, 0.5], "index": 1},
                {"embedding": [0.1, 0.2], "index": 0}
            ]
        }"#;

        let response: EmbeddingResponse = serde_json::from_str(json).unwrap();
        let mut embeddings: Vec<(usize, Vec<f32>)> = response
            .data
            .into_iter()
            .map(|d| (d.index, d.embedding))
            .collect();
        embeddings.sort_by_key(|(i, _)| *i);

        assert_eq!(embeddings[0].1, vec![0.1, 0.2]);
        assert_eq!(embeddings[1].1, vec![0.4, 0.5]);
    }
}
