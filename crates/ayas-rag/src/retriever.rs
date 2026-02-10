use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use ayas_core::config::RunnableConfig;
use ayas_core::error::{AyasError, Result};
use ayas_core::runnable::Runnable;

use crate::embedding::Embedding;
use crate::store::VectorStore;
use crate::types::SearchOptions;

/// A retriever that embeds a query and searches a vector store.
pub struct Retriever {
    embedder: Arc<dyn Embedding>,
    store: Arc<dyn VectorStore>,
    options: SearchOptions,
}

impl Retriever {
    pub fn new(
        embedder: Arc<dyn Embedding>,
        store: Arc<dyn VectorStore>,
        options: SearchOptions,
    ) -> Self {
        Self {
            embedder,
            store,
            options,
        }
    }
}

#[async_trait]
impl Runnable for Retriever {
    type Input = Value;
    type Output = Value;

    async fn invoke(&self, input: Value, _config: &RunnableConfig) -> Result<Value> {
        let query = input
            .as_str()
            .ok_or_else(|| AyasError::Other("Retriever input must be a JSON string".into()))?;

        let embedding = self.embedder.embed(query).await?;
        let results = self
            .store
            .similarity_search(&embedding, self.options.clone())
            .await?;

        let output: Vec<Value> = results
            .iter()
            .map(|r| {
                serde_json::json!({
                    "id": r.document.id,
                    "content": r.document.content,
                    "metadata": r.document.metadata,
                    "score": r.score,
                })
            })
            .collect();

        Ok(Value::Array(output))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::InMemoryVectorStore;
    use crate::types::{Document, EmbeddingVector};
    use std::collections::HashMap;

    /// A mock embedder that returns a fixed vector based on input hash.
    struct MockEmbedder {
        dim: usize,
    }

    impl MockEmbedder {
        fn new(dim: usize) -> Self {
            Self { dim }
        }
    }

    #[async_trait]
    impl Embedding for MockEmbedder {
        async fn embed(&self, text: &str) -> Result<EmbeddingVector> {
            // Simple deterministic embedding: use first char's byte value
            let seed = text.as_bytes().first().copied().unwrap_or(0) as f32;
            let mut vec = vec![0.0f32; self.dim];
            vec[0] = seed;
            if self.dim > 1 {
                vec[1] = 1.0;
            }
            Ok(EmbeddingVector::new(vec))
        }

        fn dimension(&self) -> usize {
            self.dim
        }
    }

    #[tokio::test]
    async fn retriever_invoke_returns_results() {
        let store = Arc::new(InMemoryVectorStore::new());
        let embedder = Arc::new(MockEmbedder::new(3));

        // Add a document with embedding matching what "h" would produce
        store
            .add_documents(vec![(
                Document {
                    id: "d1".into(),
                    content: "hello world".into(),
                    metadata: HashMap::new(),
                },
                EmbeddingVector::new(vec![104.0, 1.0, 0.0]), // 'h' = 104
            )])
            .await
            .unwrap();

        let retriever = Retriever::new(
            embedder,
            store,
            SearchOptions {
                k: 10,
                score_threshold: None,
            },
        );

        let config = RunnableConfig::default();
        let result = retriever
            .invoke(Value::String("hello".into()), &config)
            .await
            .unwrap();

        let arr = result.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["id"], "d1");
        assert_eq!(arr[0]["content"], "hello world");
    }

    #[tokio::test]
    async fn retriever_invoke_non_string_input_errors() {
        let store = Arc::new(InMemoryVectorStore::new());
        let embedder = Arc::new(MockEmbedder::new(3));

        let retriever = Retriever::new(embedder, store, SearchOptions::default());
        let config = RunnableConfig::default();
        let result = retriever.invoke(serde_json::json!(42), &config).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn retriever_empty_store() {
        let store = Arc::new(InMemoryVectorStore::new());
        let embedder = Arc::new(MockEmbedder::new(3));

        let retriever = Retriever::new(embedder, store, SearchOptions::default());
        let config = RunnableConfig::default();
        let result = retriever
            .invoke(Value::String("query".into()), &config)
            .await
            .unwrap();

        assert_eq!(result.as_array().unwrap().len(), 0);
    }
}
