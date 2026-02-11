use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use ayas_core::error::{AyasError, Result};

use crate::store::VectorStore;
use crate::types::{Document, EmbeddingVector, SearchOptions, SearchResult};

/// Qdrant vector store using the REST API.
pub struct QdrantStore {
    client: Client,
    base_url: String,
    collection_name: String,
}

impl QdrantStore {
    /// Create a new Qdrant store.
    /// URL defaults to `QDRANT_URL` env var, falling back to `http://localhost:6333`.
    pub fn new(collection_name: &str) -> Self {
        let base_url = std::env::var("QDRANT_URL")
            .unwrap_or_else(|_| "http://localhost:6333".into());
        Self {
            client: Client::new(),
            base_url,
            collection_name: collection_name.to_string(),
        }
    }

    pub fn with_url(mut self, url: String) -> Self {
        self.base_url = url;
        self
    }

    /// Ensure the collection exists with the given vector dimension.
    pub async fn ensure_collection(&self, dimension: usize) -> Result<()> {
        let url = format!(
            "{}/collections/{}",
            self.base_url, self.collection_name
        );

        // Check if collection exists
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| AyasError::Other(format!("Qdrant connection error: {e}")))?;

        if resp.status().is_success() {
            return Ok(());
        }

        // Create collection
        let body = serde_json::json!({
            "vectors": {
                "size": dimension,
                "distance": "Cosine"
            }
        });

        let resp = self
            .client
            .put(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| AyasError::Other(format!("Qdrant create collection error: {e}")))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(AyasError::Other(format!(
                "Qdrant create collection failed: {body}"
            )));
        }

        Ok(())
    }
}

#[async_trait]
impl VectorStore for QdrantStore {
    async fn add_documents(&self, docs: Vec<(Document, EmbeddingVector)>) -> Result<Vec<String>> {
        let points: Vec<QdrantPoint> = docs
            .iter()
            .map(|(doc, emb)| QdrantPoint {
                id: doc.id.clone(),
                vector: emb.0.clone(),
                payload: serde_json::json!({
                    "content": doc.content,
                    "metadata": doc.metadata,
                }),
            })
            .collect();

        let ids: Vec<String> = docs.iter().map(|(doc, _)| doc.id.clone()).collect();

        let body = QdrantUpsertRequest { points };

        let url = format!(
            "{}/collections/{}/points",
            self.base_url, self.collection_name
        );

        let resp = self
            .client
            .put(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| AyasError::Other(format!("Qdrant upsert error: {e}")))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(AyasError::Other(format!("Qdrant upsert failed: {body}")));
        }

        Ok(ids)
    }

    async fn similarity_search(
        &self,
        query: &EmbeddingVector,
        options: SearchOptions,
    ) -> Result<Vec<SearchResult>> {
        let body = QdrantSearchRequest {
            vector: query.0.clone(),
            limit: options.k,
            score_threshold: options.score_threshold,
            with_payload: true,
        };

        let url = format!(
            "{}/collections/{}/points/search",
            self.base_url, self.collection_name
        );

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| AyasError::Other(format!("Qdrant search error: {e}")))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(AyasError::Other(format!("Qdrant search failed: {body}")));
        }

        let response: QdrantSearchResponse = resp
            .json()
            .await
            .map_err(|e| AyasError::Other(format!("Qdrant response parse error: {e}")))?;

        let results = response
            .result
            .into_iter()
            .map(|hit| {
                let payload = hit.payload.unwrap_or_default();
                let content = payload
                    .get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let metadata = payload
                    .get("metadata")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default();

                SearchResult {
                    document: Document {
                        id: hit.id,
                        content,
                        metadata,
                    },
                    score: hit.score,
                }
            })
            .collect();

        Ok(results)
    }

    async fn delete(&self, ids: &[String]) -> Result<()> {
        let body = serde_json::json!({
            "points": ids,
        });

        let url = format!(
            "{}/collections/{}/points/delete",
            self.base_url, self.collection_name
        );

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| AyasError::Other(format!("Qdrant delete error: {e}")))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(AyasError::Other(format!("Qdrant delete failed: {body}")));
        }

        Ok(())
    }

    async fn get(&self, id: &str) -> Result<Option<Document>> {
        let url = format!(
            "{}/collections/{}/points/{}",
            self.base_url, self.collection_name, id
        );

        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| AyasError::Other(format!("Qdrant get error: {e}")))?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(AyasError::Other(format!("Qdrant get failed: {body}")));
        }

        let response: QdrantGetResponse = resp
            .json()
            .await
            .map_err(|e| AyasError::Other(format!("Qdrant get parse error: {e}")))?;

        let payload = response.result.payload.unwrap_or_default();
        let content = payload
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let metadata = payload
            .get("metadata")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        Ok(Some(Document {
            id: response.result.id,
            content,
            metadata,
        }))
    }
}

// --- Qdrant API types ---

#[derive(Serialize)]
struct QdrantPoint {
    id: String,
    vector: Vec<f32>,
    payload: Value,
}

#[derive(Serialize)]
struct QdrantUpsertRequest {
    points: Vec<QdrantPoint>,
}

#[derive(Serialize)]
struct QdrantSearchRequest {
    vector: Vec<f32>,
    limit: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    score_threshold: Option<f32>,
    with_payload: bool,
}

#[derive(Deserialize)]
struct QdrantSearchResponse {
    result: Vec<QdrantSearchHit>,
}

#[derive(Deserialize)]
struct QdrantSearchHit {
    id: String,
    score: f32,
    payload: Option<Value>,
}

#[derive(Deserialize)]
struct QdrantGetResponse {
    result: QdrantGetResult,
}

#[derive(Deserialize)]
struct QdrantGetResult {
    id: String,
    payload: Option<Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_url_from_env() {
        // Temporarily set env var
        let original = std::env::var("QDRANT_URL").ok();
        unsafe { std::env::set_var("QDRANT_URL", "http://custom:443") };

        let store = QdrantStore::new("test-collection");
        assert_eq!(store.base_url, "http://custom:443");
        assert_eq!(store.collection_name, "test-collection");

        // Restore
        match original {
            Some(v) => unsafe { std::env::set_var("QDRANT_URL", v) },
            None => unsafe { std::env::remove_var("QDRANT_URL") },
        }
    }

    #[test]
    fn with_url_override() {
        let store = QdrantStore::new("coll").with_url("http://override:8080".into());
        assert_eq!(store.base_url, "http://override:8080");
    }

    #[test]
    fn serialize_upsert_request() {
        let req = QdrantUpsertRequest {
            points: vec![QdrantPoint {
                id: "p1".into(),
                vector: vec![1.0, 2.0, 3.0],
                payload: serde_json::json!({"content": "hello"}),
            }],
        };

        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["points"][0]["id"], "p1");
        let vec = json["points"][0]["vector"].as_array().unwrap();
        assert_eq!(vec.len(), 3);
    }

    #[test]
    fn serialize_search_request_without_threshold() {
        let req = QdrantSearchRequest {
            vector: vec![1.0, 2.0],
            limit: 5,
            score_threshold: None,
            with_payload: true,
        };

        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["limit"], 5);
        assert!(json.get("score_threshold").is_none());
        assert_eq!(json["with_payload"], true);
    }

    #[test]
    fn serialize_search_request_with_threshold() {
        let req = QdrantSearchRequest {
            vector: vec![1.0],
            limit: 10,
            score_threshold: Some(0.5),
            with_payload: true,
        };

        let json = serde_json::to_value(&req).unwrap();
        assert!(json.get("score_threshold").is_some());
        let threshold = json["score_threshold"].as_f64().unwrap();
        assert!((threshold - 0.5).abs() < 1e-6);
    }

    #[test]
    fn parse_search_response() {
        let json = r#"{
            "result": [
                {
                    "id": "p1",
                    "version": 1,
                    "score": 0.95,
                    "payload": {"content": "hello world", "metadata": {"source": "test"}}
                },
                {
                    "id": "p2",
                    "version": 1,
                    "score": 0.80,
                    "payload": {"content": "foo bar"}
                }
            ],
            "status": "ok",
            "time": 0.001
        }"#;

        let response: QdrantSearchResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.result.len(), 2);
        assert_eq!(response.result[0].id, "p1");
        assert!((response.result[0].score - 0.95).abs() < 1e-6);
        assert_eq!(
            response.result[0]
                .payload
                .as_ref()
                .unwrap()
                .get("content")
                .unwrap(),
            "hello world"
        );
    }

    #[test]
    fn parse_get_response() {
        let json = r#"{
            "result": {
                "id": "p1",
                "payload": {"content": "hello", "metadata": {}},
                "vector": [0.1, 0.2]
            },
            "status": "ok",
            "time": 0.0005
        }"#;

        let response: QdrantGetResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.result.id, "p1");
    }

    #[test]
    fn search_hit_to_document() {
        let hit = QdrantSearchHit {
            id: "doc1".into(),
            score: 0.9,
            payload: Some(serde_json::json!({
                "content": "test content",
                "metadata": {"key": "value"}
            })),
        };

        let payload = hit.payload.unwrap();
        let content = payload
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let metadata: std::collections::HashMap<String, Value> = payload
            .get("metadata")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        let doc = Document {
            id: "doc1".into(),
            content,
            metadata,
        };

        assert_eq!(doc.id, "doc1");
        assert_eq!(doc.content, "test content");
        assert_eq!(doc.metadata.get("key").unwrap(), "value");
    }
}
