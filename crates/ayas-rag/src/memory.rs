use std::collections::HashMap;

use async_trait::async_trait;
use tokio::sync::RwLock;

use ayas_core::error::Result;

use crate::store::VectorStore;
use crate::types::{Document, EmbeddingVector, SearchOptions, SearchResult};

/// An in-memory vector store backed by a HashMap.
pub struct InMemoryVectorStore {
    data: RwLock<HashMap<String, (Document, EmbeddingVector)>>,
}

impl InMemoryVectorStore {
    pub fn new() -> Self {
        Self {
            data: RwLock::new(HashMap::new()),
        }
    }
}

impl Default for InMemoryVectorStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl VectorStore for InMemoryVectorStore {
    async fn add_documents(&self, docs: Vec<(Document, EmbeddingVector)>) -> Result<Vec<String>> {
        let mut data = self.data.write().await;
        let ids: Vec<String> = docs
            .into_iter()
            .map(|(doc, emb)| {
                let id = doc.id.clone();
                data.insert(id.clone(), (doc, emb));
                id
            })
            .collect();
        Ok(ids)
    }

    async fn similarity_search(
        &self,
        query: &EmbeddingVector,
        options: SearchOptions,
    ) -> Result<Vec<SearchResult>> {
        let data = self.data.read().await;

        let mut scored: Vec<SearchResult> = data
            .values()
            .map(|(doc, emb)| SearchResult {
                document: doc.clone(),
                score: query.cosine_similarity(emb),
            })
            .collect();

        // Apply score threshold filter
        if let Some(threshold) = options.score_threshold {
            scored.retain(|r| r.score >= threshold);
        }

        // Sort by score descending
        scored.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

        // Return top-k
        scored.truncate(options.k);

        Ok(scored)
    }

    async fn delete(&self, ids: &[String]) -> Result<()> {
        let mut data = self.data.write().await;
        for id in ids {
            data.remove(id);
        }
        Ok(())
    }

    async fn get(&self, id: &str) -> Result<Option<Document>> {
        let data = self.data.read().await;
        Ok(data.get(id).map(|(doc, _)| doc.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_doc(id: &str, content: &str) -> Document {
        Document {
            id: id.into(),
            content: content.into(),
            metadata: HashMap::new(),
        }
    }

    fn make_emb(values: Vec<f32>) -> EmbeddingVector {
        EmbeddingVector::new(values)
    }

    #[tokio::test]
    async fn add_and_get_document() {
        let store = InMemoryVectorStore::new();
        let doc = make_doc("d1", "hello");
        let emb = make_emb(vec![1.0, 0.0, 0.0]);

        let ids = store.add_documents(vec![(doc.clone(), emb)]).await.unwrap();
        assert_eq!(ids, vec!["d1"]);

        let retrieved = store.get("d1").await.unwrap().unwrap();
        assert_eq!(retrieved.id, "d1");
        assert_eq!(retrieved.content, "hello");
    }

    #[tokio::test]
    async fn get_nonexistent_returns_none() {
        let store = InMemoryVectorStore::new();
        let result = store.get("nonexistent").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn delete_document() {
        let store = InMemoryVectorStore::new();
        store
            .add_documents(vec![(make_doc("d1", "hello"), make_emb(vec![1.0, 0.0]))])
            .await
            .unwrap();

        store.delete(&["d1".into()]).await.unwrap();
        assert!(store.get("d1").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn delete_nonexistent_is_ok() {
        let store = InMemoryVectorStore::new();
        let result = store.delete(&["nonexistent".into()]).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn similarity_search_basic() {
        let store = InMemoryVectorStore::new();
        store
            .add_documents(vec![
                (make_doc("d1", "close"), make_emb(vec![1.0, 0.0, 0.0])),
                (make_doc("d2", "far"), make_emb(vec![0.0, 1.0, 0.0])),
                (make_doc("d3", "medium"), make_emb(vec![0.7, 0.7, 0.0])),
            ])
            .await
            .unwrap();

        let query = make_emb(vec![1.0, 0.0, 0.0]);
        let results = store
            .similarity_search(&query, SearchOptions { k: 2, score_threshold: None })
            .await
            .unwrap();

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].document.id, "d1");
        assert!((results[0].score - 1.0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn similarity_search_with_threshold() {
        let store = InMemoryVectorStore::new();
        store
            .add_documents(vec![
                (make_doc("d1", "close"), make_emb(vec![1.0, 0.0])),
                (make_doc("d2", "far"), make_emb(vec![0.0, 1.0])),
            ])
            .await
            .unwrap();

        let query = make_emb(vec![1.0, 0.0]);
        let results = store
            .similarity_search(
                &query,
                SearchOptions {
                    k: 10,
                    score_threshold: Some(0.5),
                },
            )
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].document.id, "d1");
    }

    #[tokio::test]
    async fn similarity_search_k_zero() {
        let store = InMemoryVectorStore::new();
        store
            .add_documents(vec![(make_doc("d1", "hello"), make_emb(vec![1.0]))])
            .await
            .unwrap();

        let results = store
            .similarity_search(
                &make_emb(vec![1.0]),
                SearchOptions { k: 0, score_threshold: None },
            )
            .await
            .unwrap();

        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn similarity_search_empty_store() {
        let store = InMemoryVectorStore::new();
        let results = store
            .similarity_search(
                &make_emb(vec![1.0, 0.0]),
                SearchOptions::default(),
            )
            .await
            .unwrap();

        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn duplicate_id_overwrites() {
        let store = InMemoryVectorStore::new();
        store
            .add_documents(vec![(make_doc("d1", "first"), make_emb(vec![1.0]))])
            .await
            .unwrap();
        store
            .add_documents(vec![(make_doc("d1", "second"), make_emb(vec![1.0]))])
            .await
            .unwrap();

        let doc = store.get("d1").await.unwrap().unwrap();
        assert_eq!(doc.content, "second");
    }

    #[tokio::test]
    async fn add_multiple_documents() {
        let store = InMemoryVectorStore::new();
        let ids = store
            .add_documents(vec![
                (make_doc("d1", "a"), make_emb(vec![1.0])),
                (make_doc("d2", "b"), make_emb(vec![2.0])),
                (make_doc("d3", "c"), make_emb(vec![3.0])),
            ])
            .await
            .unwrap();

        assert_eq!(ids, vec!["d1", "d2", "d3"]);
        assert!(store.get("d1").await.unwrap().is_some());
        assert!(store.get("d2").await.unwrap().is_some());
        assert!(store.get("d3").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn add_then_delete_then_search() {
        let store = InMemoryVectorStore::new();
        store
            .add_documents(vec![(make_doc("d1", "hello"), make_emb(vec![1.0, 0.0]))])
            .await
            .unwrap();

        store.delete(&["d1".into()]).await.unwrap();

        let results = store
            .similarity_search(
                &make_emb(vec![1.0, 0.0]),
                SearchOptions::default(),
            )
            .await
            .unwrap();

        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn search_results_ordered_by_score() {
        let store = InMemoryVectorStore::new();
        store
            .add_documents(vec![
                (make_doc("d1", "low"), make_emb(vec![0.0, 1.0])),
                (make_doc("d2", "high"), make_emb(vec![1.0, 0.0])),
                (make_doc("d3", "mid"), make_emb(vec![0.7, 0.3])),
            ])
            .await
            .unwrap();

        let query = make_emb(vec![1.0, 0.0]);
        let results = store
            .similarity_search(&query, SearchOptions { k: 3, score_threshold: None })
            .await
            .unwrap();

        assert_eq!(results.len(), 3);
        assert!(results[0].score >= results[1].score);
        assert!(results[1].score >= results[2].score);
    }
}
