use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use ayas_core::config::RunnableConfig;
use ayas_core::error::{AyasError, Result};
use ayas_core::runnable::Runnable;

use crate::embedding::Embedding;
use crate::store::VectorStore;
use crate::types::{EmbeddingVector, SearchOptions, SearchResult};

// ---------------------------------------------------------------------------
// SimilarityRetriever (original Retriever, renamed for clarity)
// ---------------------------------------------------------------------------

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

        Ok(results_to_json(&results))
    }
}

/// Alias for the basic similarity retriever.
pub type SimilarityRetriever = Retriever;

// ---------------------------------------------------------------------------
// ThresholdRetriever
// ---------------------------------------------------------------------------

/// A retriever that filters results by a minimum similarity score threshold.
pub struct ThresholdRetriever {
    embedder: Arc<dyn Embedding>,
    store: Arc<dyn VectorStore>,
    threshold: f32,
    k: usize,
}

impl ThresholdRetriever {
    pub fn new(
        embedder: Arc<dyn Embedding>,
        store: Arc<dyn VectorStore>,
        threshold: f32,
        k: usize,
    ) -> Self {
        Self {
            embedder,
            store,
            threshold,
            k,
        }
    }
}

#[async_trait]
impl Runnable for ThresholdRetriever {
    type Input = Value;
    type Output = Value;

    async fn invoke(&self, input: Value, _config: &RunnableConfig) -> Result<Value> {
        let query = input
            .as_str()
            .ok_or_else(|| AyasError::Other("Retriever input must be a JSON string".into()))?;

        let embedding = self.embedder.embed(query).await?;
        let options = SearchOptions {
            k: self.k,
            score_threshold: Some(self.threshold),
        };
        let results = self.store.similarity_search(&embedding, options).await?;

        Ok(results_to_json(&results))
    }
}

// ---------------------------------------------------------------------------
// MaxMarginalRelevanceRetriever (MMR)
// ---------------------------------------------------------------------------

/// A retriever that uses Maximal Marginal Relevance to balance relevance and diversity.
///
/// MMR selects documents iteratively:
/// - Each step picks the doc that maximizes: `lambda * sim(query, doc) - (1 - lambda) * max(sim(doc, selected))`
/// - `lambda = 1.0` → pure relevance (same as similarity search)
/// - `lambda = 0.0` → pure diversity
pub struct MaxMarginalRelevanceRetriever {
    embedder: Arc<dyn Embedding>,
    store: Arc<dyn VectorStore>,
    /// Number of final results to return.
    k: usize,
    /// Number of candidates to fetch from the store before applying MMR.
    fetch_k: usize,
    /// Trade-off between relevance (1.0) and diversity (0.0). Default: 0.5.
    lambda: f32,
}

impl MaxMarginalRelevanceRetriever {
    pub fn new(
        embedder: Arc<dyn Embedding>,
        store: Arc<dyn VectorStore>,
        k: usize,
        fetch_k: usize,
        lambda: f32,
    ) -> Self {
        Self {
            embedder,
            store,
            k,
            fetch_k,
            lambda,
        }
    }
}

#[async_trait]
impl Runnable for MaxMarginalRelevanceRetriever {
    type Input = Value;
    type Output = Value;

    async fn invoke(&self, input: Value, _config: &RunnableConfig) -> Result<Value> {
        let query = input
            .as_str()
            .ok_or_else(|| AyasError::Other("Retriever input must be a JSON string".into()))?;

        let query_embedding = self.embedder.embed(query).await?;

        // Fetch more candidates than needed
        let options = SearchOptions {
            k: self.fetch_k,
            score_threshold: None,
        };
        let candidates = self.store.similarity_search(&query_embedding, options).await?;

        if candidates.is_empty() {
            return Ok(Value::Array(vec![]));
        }

        // Re-embed candidates for inter-document similarity
        let candidate_texts: Vec<&str> = candidates.iter().map(|c| c.document.content.as_str()).collect();
        let candidate_embeddings = self.embedder.embed_batch(&candidate_texts).await?;

        let selected = mmr_select(
            &query_embedding,
            &candidates,
            &candidate_embeddings,
            self.k,
            self.lambda,
        );

        Ok(results_to_json(&selected))
    }
}

/// MMR selection algorithm.
///
/// Iteratively selects documents that maximize:
/// `lambda * sim(query, doc) - (1 - lambda) * max_j(sim(doc, selected_j))`
pub fn mmr_select(
    query_embedding: &EmbeddingVector,
    candidates: &[SearchResult],
    candidate_embeddings: &[EmbeddingVector],
    k: usize,
    lambda: f32,
) -> Vec<SearchResult> {
    let n = candidates.len();
    if n == 0 || k == 0 {
        return vec![];
    }

    let k = k.min(n);

    // Pre-compute query similarities
    let query_sims: Vec<f32> = candidate_embeddings
        .iter()
        .map(|emb| query_embedding.cosine_similarity(emb))
        .collect();

    let mut selected_indices: Vec<usize> = Vec::with_capacity(k);
    let mut remaining: Vec<usize> = (0..n).collect();

    for _ in 0..k {
        let mut best_idx = 0;
        let mut best_score = f32::NEG_INFINITY;

        for (pos, &cand_idx) in remaining.iter().enumerate() {
            let relevance = query_sims[cand_idx];

            let max_similarity = if selected_indices.is_empty() {
                0.0
            } else {
                selected_indices
                    .iter()
                    .map(|&sel_idx| {
                        candidate_embeddings[cand_idx]
                            .cosine_similarity(&candidate_embeddings[sel_idx])
                    })
                    .fold(f32::NEG_INFINITY, f32::max)
            };

            let mmr_score = lambda * relevance - (1.0 - lambda) * max_similarity;

            if mmr_score > best_score {
                best_score = mmr_score;
                best_idx = pos;
            }
        }

        let chosen = remaining.remove(best_idx);
        selected_indices.push(chosen);
    }

    selected_indices
        .into_iter()
        .map(|i| candidates[i].clone())
        .collect()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn results_to_json(results: &[SearchResult]) -> Value {
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
    Value::Array(output)
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

    // ---- ThresholdRetriever tests ----

    #[tokio::test]
    async fn threshold_retriever_filters_low_scores() {
        let store = Arc::new(InMemoryVectorStore::new());
        let embedder = Arc::new(MockEmbedder::new(3));

        store
            .add_documents(vec![
                (
                    Document {
                        id: "d1".into(),
                        content: "hello".into(),
                        metadata: HashMap::new(),
                    },
                    EmbeddingVector::new(vec![104.0, 1.0, 0.0]), // high sim to "h" query
                ),
                (
                    Document {
                        id: "d2".into(),
                        content: "zzz".into(),
                        metadata: HashMap::new(),
                    },
                    EmbeddingVector::new(vec![0.0, 0.0, 1.0]), // low sim
                ),
            ])
            .await
            .unwrap();

        let retriever = ThresholdRetriever::new(embedder, store, 0.5, 10);
        let config = RunnableConfig::default();
        let result = retriever
            .invoke(Value::String("hello".into()), &config)
            .await
            .unwrap();

        let arr = result.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["id"], "d1");
    }

    // ---- MMR tests ----

    #[test]
    fn mmr_select_empty_candidates() {
        let query = EmbeddingVector::new(vec![1.0, 0.0]);
        let result = mmr_select(&query, &[], &[], 5, 0.5);
        assert!(result.is_empty());
    }

    #[test]
    fn mmr_select_k_zero() {
        let query = EmbeddingVector::new(vec![1.0, 0.0]);
        let candidates = vec![SearchResult {
            document: Document {
                id: "d1".into(),
                content: "test".into(),
                metadata: HashMap::new(),
            },
            score: 1.0,
        }];
        let embeddings = vec![EmbeddingVector::new(vec![1.0, 0.0])];
        let result = mmr_select(&query, &candidates, &embeddings, 0, 0.5);
        assert!(result.is_empty());
    }

    #[test]
    fn mmr_select_lambda_one_is_pure_relevance() {
        let query = EmbeddingVector::new(vec![1.0, 0.0]);
        let candidates = vec![
            SearchResult {
                document: Document {
                    id: "high".into(),
                    content: "high".into(),
                    metadata: HashMap::new(),
                },
                score: 0.99,
            },
            SearchResult {
                document: Document {
                    id: "low".into(),
                    content: "low".into(),
                    metadata: HashMap::new(),
                },
                score: 0.1,
            },
        ];
        let embeddings = vec![
            EmbeddingVector::new(vec![1.0, 0.0]),  // identical to query
            EmbeddingVector::new(vec![0.0, 1.0]),  // orthogonal to query
        ];

        let results = mmr_select(&query, &candidates, &embeddings, 2, 1.0);
        // With lambda=1.0, purely relevance-based, most similar first
        assert_eq!(results[0].document.id, "high");
        assert_eq!(results[1].document.id, "low");
    }

    #[test]
    fn mmr_select_promotes_diversity() {
        // Three docs: A, B very similar to each other and query; C diverse
        let query = EmbeddingVector::new(vec![1.0, 0.0, 0.0]);

        let candidates = vec![
            SearchResult {
                document: Document { id: "a".into(), content: "a".into(), metadata: HashMap::new() },
                score: 0.99,
            },
            SearchResult {
                document: Document { id: "b".into(), content: "b".into(), metadata: HashMap::new() },
                score: 0.98,
            },
            SearchResult {
                document: Document { id: "c".into(), content: "c".into(), metadata: HashMap::new() },
                score: 0.5,
            },
        ];

        let embeddings = vec![
            EmbeddingVector::new(vec![1.0, 0.0, 0.0]),   // a: identical to query
            EmbeddingVector::new(vec![0.99, 0.01, 0.0]),  // b: very similar to a
            EmbeddingVector::new(vec![0.0, 0.0, 1.0]),    // c: diverse
        ];

        // Low lambda → diversity preferred
        let results = mmr_select(&query, &candidates, &embeddings, 2, 0.1);
        // First pick: "a" (most relevant)
        assert_eq!(results[0].document.id, "a");
        // Second pick: "c" (most diverse from "a"), not "b"
        assert_eq!(results[1].document.id, "c");
    }

    #[test]
    fn mmr_select_single_candidate() {
        let query = EmbeddingVector::new(vec![1.0, 0.0]);
        let candidates = vec![SearchResult {
            document: Document {
                id: "only".into(),
                content: "only".into(),
                metadata: HashMap::new(),
            },
            score: 0.9,
        }];
        let embeddings = vec![EmbeddingVector::new(vec![0.9, 0.1])];

        let results = mmr_select(&query, &candidates, &embeddings, 5, 0.5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].document.id, "only");
    }

    #[test]
    fn mmr_select_k_greater_than_candidates() {
        let query = EmbeddingVector::new(vec![1.0, 0.0]);
        let candidates = vec![
            SearchResult {
                document: Document { id: "a".into(), content: "a".into(), metadata: HashMap::new() },
                score: 0.9,
            },
            SearchResult {
                document: Document { id: "b".into(), content: "b".into(), metadata: HashMap::new() },
                score: 0.5,
            },
        ];
        let embeddings = vec![
            EmbeddingVector::new(vec![1.0, 0.0]),
            EmbeddingVector::new(vec![0.0, 1.0]),
        ];

        let results = mmr_select(&query, &candidates, &embeddings, 10, 0.5);
        assert_eq!(results.len(), 2);
    }
}
