use async_trait::async_trait;

use ayas_core::error::Result;

use crate::types::EmbeddingVector;

/// Trait for text embedding models.
#[async_trait]
pub trait Embedding: Send + Sync {
    /// Embed a single text string.
    async fn embed(&self, text: &str) -> Result<EmbeddingVector>;

    /// Embed multiple texts in a batch.
    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<EmbeddingVector>> {
        let mut results = Vec::with_capacity(texts.len());
        for text in texts {
            results.push(self.embed(text).await?);
        }
        Ok(results)
    }

    /// The dimensionality of the embedding vectors.
    fn dimension(&self) -> usize;
}
