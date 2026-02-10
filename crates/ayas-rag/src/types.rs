use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A document that can be stored in a vector store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    /// Unique identifier.
    pub id: String,
    /// Text content of the document.
    pub content: String,
    /// Arbitrary metadata.
    #[serde(default)]
    pub metadata: HashMap<String, Value>,
}

/// A vector embedding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingVector(pub Vec<f32>);

impl EmbeddingVector {
    pub fn new(data: Vec<f32>) -> Self {
        Self(data)
    }

    pub fn dimension(&self) -> usize {
        self.0.len()
    }

    pub fn as_slice(&self) -> &[f32] {
        &self.0
    }

    /// Cosine similarity with another vector.
    pub fn cosine_similarity(&self, other: &EmbeddingVector) -> f32 {
        let dot: f32 = self.0.iter().zip(other.0.iter()).map(|(a, b)| a * b).sum();
        let norm_a: f32 = self.0.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = other.0.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm_a == 0.0 || norm_b == 0.0 {
            return 0.0;
        }
        dot / (norm_a * norm_b)
    }
}

/// A search result with similarity score.
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub document: Document,
    pub score: f32,
}

/// Options for similarity search.
#[derive(Debug, Clone)]
pub struct SearchOptions {
    /// Number of results to return.
    pub k: usize,
    /// Minimum similarity score threshold.
    pub score_threshold: Option<f32>,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            k: 4,
            score_threshold: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_similarity_identical_vectors() {
        let v = EmbeddingVector::new(vec![1.0, 2.0, 3.0]);
        let sim = v.cosine_similarity(&v);
        assert!((sim - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_orthogonal_vectors() {
        let a = EmbeddingVector::new(vec![1.0, 0.0]);
        let b = EmbeddingVector::new(vec![0.0, 1.0]);
        let sim = a.cosine_similarity(&b);
        assert!(sim.abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_opposite_vectors() {
        let a = EmbeddingVector::new(vec![1.0, 0.0]);
        let b = EmbeddingVector::new(vec![-1.0, 0.0]);
        let sim = a.cosine_similarity(&b);
        assert!((sim - (-1.0)).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_zero_vector() {
        let a = EmbeddingVector::new(vec![1.0, 2.0]);
        let zero = EmbeddingVector::new(vec![0.0, 0.0]);
        assert_eq!(a.cosine_similarity(&zero), 0.0);
        assert_eq!(zero.cosine_similarity(&a), 0.0);
    }

    #[test]
    fn cosine_similarity_symmetric() {
        let a = EmbeddingVector::new(vec![1.0, 2.0, 3.0]);
        let b = EmbeddingVector::new(vec![4.0, 5.0, 6.0]);
        let ab = a.cosine_similarity(&b);
        let ba = b.cosine_similarity(&a);
        assert!((ab - ba).abs() < 1e-6);
    }

    #[test]
    fn embedding_vector_dimension() {
        let v = EmbeddingVector::new(vec![1.0, 2.0, 3.0]);
        assert_eq!(v.dimension(), 3);
    }

    #[test]
    fn embedding_vector_as_slice() {
        let v = EmbeddingVector::new(vec![1.0, 2.0]);
        assert_eq!(v.as_slice(), &[1.0, 2.0]);
    }

    #[test]
    fn document_serde_roundtrip() {
        let doc = Document {
            id: "doc1".into(),
            content: "hello world".into(),
            metadata: HashMap::from([("key".into(), serde_json::json!("value"))]),
        };
        let json = serde_json::to_string(&doc).unwrap();
        let deserialized: Document = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, doc.id);
        assert_eq!(deserialized.content, doc.content);
        assert_eq!(deserialized.metadata, doc.metadata);
    }

    #[test]
    fn document_default_metadata() {
        let json = r#"{"id":"d1","content":"text"}"#;
        let doc: Document = serde_json::from_str(json).unwrap();
        assert!(doc.metadata.is_empty());
    }

    #[test]
    fn search_options_default() {
        let opts = SearchOptions::default();
        assert_eq!(opts.k, 4);
        assert!(opts.score_threshold.is_none());
    }
}
