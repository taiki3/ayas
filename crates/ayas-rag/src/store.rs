use async_trait::async_trait;

use ayas_core::error::Result;

use crate::types::{Document, EmbeddingVector, SearchOptions, SearchResult};

/// Trait for vector stores.
#[async_trait]
pub trait VectorStore: Send + Sync {
    /// Add documents with their embeddings. Returns the document IDs.
    async fn add_documents(&self, docs: Vec<(Document, EmbeddingVector)>) -> Result<Vec<String>>;

    /// Search for similar documents.
    async fn similarity_search(
        &self,
        query: &EmbeddingVector,
        options: SearchOptions,
    ) -> Result<Vec<SearchResult>>;

    /// Delete documents by ID.
    async fn delete(&self, ids: &[String]) -> Result<()>;

    /// Get a document by ID.
    async fn get(&self, id: &str) -> Result<Option<Document>>;
}
