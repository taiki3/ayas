pub mod embedding;
pub mod gemini_embedding;
pub mod memory;
pub mod openai_embedding;
pub mod qdrant_store;
pub mod retriever;
pub mod store;
pub mod types;

pub mod prelude {
    pub use crate::embedding::Embedding;
    pub use crate::gemini_embedding::GeminiEmbedding;
    pub use crate::memory::InMemoryVectorStore;
    pub use crate::openai_embedding::{OpenAiEmbedding, OpenAiEmbeddingModel};
    pub use crate::qdrant_store::QdrantStore;
    pub use crate::retriever::{
        mmr_select, MaxMarginalRelevanceRetriever, Retriever, SimilarityRetriever,
        ThresholdRetriever,
    };
    pub use crate::store::VectorStore;
    pub use crate::types::{Document, EmbeddingVector, SearchOptions, SearchResult};
}
