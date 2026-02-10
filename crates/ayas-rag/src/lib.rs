pub mod embedding;
pub mod memory;
pub mod retriever;
pub mod store;
pub mod types;

pub mod prelude {
    pub use crate::embedding::Embedding;
    pub use crate::memory::InMemoryVectorStore;
    pub use crate::retriever::Retriever;
    pub use crate::store::VectorStore;
    pub use crate::types::{Document, EmbeddingVector, SearchOptions, SearchResult};
}
