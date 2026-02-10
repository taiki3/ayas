use async_trait::async_trait;

use ayas_core::error::Result;

use crate::types::Checkpoint;

/// Async storage backend for graph checkpoints.
///
/// Implementations must be thread-safe (`Send + Sync`).
#[async_trait]
pub trait CheckpointStore: Send + Sync {
    /// Store a checkpoint. If a checkpoint with the same ID exists, it is overwritten.
    async fn put(&self, checkpoint: Checkpoint) -> Result<()>;

    /// Retrieve a specific checkpoint by thread ID and checkpoint ID.
    async fn get(&self, thread_id: &str, checkpoint_id: &str) -> Result<Option<Checkpoint>>;

    /// Retrieve the latest (most recent) checkpoint for a thread.
    async fn get_latest(&self, thread_id: &str) -> Result<Option<Checkpoint>>;

    /// List all checkpoints for a thread, ordered by step (ascending).
    async fn list(&self, thread_id: &str) -> Result<Vec<Checkpoint>>;

    /// Delete all checkpoints for a given thread.
    async fn delete_thread(&self, thread_id: &str) -> Result<()>;
}
