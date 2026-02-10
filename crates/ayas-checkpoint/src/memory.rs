use std::collections::HashMap;
use std::sync::RwLock;

use async_trait::async_trait;

use ayas_core::error::Result;

use crate::store::CheckpointStore;
use crate::types::Checkpoint;

/// In-memory checkpoint store for testing and short-lived workflows.
///
/// Thread-safe via `RwLock`. All data is lost when the store is dropped.
pub struct MemoryCheckpointStore {
    /// Map: thread_id → Vec<Checkpoint> (ordered by step)
    data: RwLock<HashMap<String, Vec<Checkpoint>>>,
}

impl MemoryCheckpointStore {
    pub fn new() -> Self {
        Self {
            data: RwLock::new(HashMap::new()),
        }
    }
}

impl Default for MemoryCheckpointStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CheckpointStore for MemoryCheckpointStore {
    async fn put(&self, checkpoint: Checkpoint) -> Result<()> {
        let mut data = self.data.write().unwrap();
        let thread = data.entry(checkpoint.thread_id.clone()).or_default();

        // Replace if same ID exists, otherwise append
        if let Some(pos) = thread.iter().position(|cp| cp.id == checkpoint.id) {
            thread[pos] = checkpoint;
        } else {
            thread.push(checkpoint);
        }

        // Keep sorted by step
        thread.sort_by_key(|cp| cp.step);
        Ok(())
    }

    async fn get(&self, thread_id: &str, checkpoint_id: &str) -> Result<Option<Checkpoint>> {
        let data = self.data.read().unwrap();
        Ok(data
            .get(thread_id)
            .and_then(|thread| thread.iter().find(|cp| cp.id == checkpoint_id).cloned()))
    }

    async fn get_latest(&self, thread_id: &str) -> Result<Option<Checkpoint>> {
        let data = self.data.read().unwrap();
        Ok(data
            .get(thread_id)
            .and_then(|thread| thread.last().cloned()))
    }

    async fn list(&self, thread_id: &str) -> Result<Vec<Checkpoint>> {
        let data = self.data.read().unwrap();
        Ok(data.get(thread_id).cloned().unwrap_or_default())
    }

    async fn delete_thread(&self, thread_id: &str) -> Result<()> {
        let mut data = self.data.write().unwrap();
        data.remove(thread_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::CheckpointMetadata;
    use chrono::Utc;
    use serde_json::json;
    use std::collections::HashMap;

    fn make_checkpoint(id: &str, thread_id: &str, step: usize) -> Checkpoint {
        Checkpoint {
            id: id.into(),
            thread_id: thread_id.into(),
            parent_id: if step > 0 {
                Some(format!("cp-{}", step - 1))
            } else {
                None
            },
            step,
            channel_values: HashMap::from([("count".into(), json!(step))]),
            pending_nodes: vec![],
            metadata: CheckpointMetadata {
                source: "loop".into(),
                step,
                node_name: Some(format!("node_{step}")),
            },
            created_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn put_and_get() {
        let store = MemoryCheckpointStore::new();
        let cp = make_checkpoint("cp-0", "thread-1", 0);
        store.put(cp.clone()).await.unwrap();

        let retrieved = store.get("thread-1", "cp-0").await.unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().id, "cp-0");
    }

    #[tokio::test]
    async fn get_nonexistent() {
        let store = MemoryCheckpointStore::new();
        let result = store.get("no-thread", "no-cp").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn get_latest() {
        let store = MemoryCheckpointStore::new();
        store
            .put(make_checkpoint("cp-0", "thread-1", 0))
            .await
            .unwrap();
        store
            .put(make_checkpoint("cp-1", "thread-1", 1))
            .await
            .unwrap();
        store
            .put(make_checkpoint("cp-2", "thread-1", 2))
            .await
            .unwrap();

        let latest = store.get_latest("thread-1").await.unwrap().unwrap();
        assert_eq!(latest.id, "cp-2");
        assert_eq!(latest.step, 2);
    }

    #[tokio::test]
    async fn list_ordered_by_step() {
        let store = MemoryCheckpointStore::new();
        // Insert out of order
        store
            .put(make_checkpoint("cp-2", "thread-1", 2))
            .await
            .unwrap();
        store
            .put(make_checkpoint("cp-0", "thread-1", 0))
            .await
            .unwrap();
        store
            .put(make_checkpoint("cp-1", "thread-1", 1))
            .await
            .unwrap();

        let list = store.list("thread-1").await.unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].step, 0);
        assert_eq!(list[1].step, 1);
        assert_eq!(list[2].step, 2);
    }

    #[tokio::test]
    async fn separate_threads() {
        let store = MemoryCheckpointStore::new();
        store
            .put(make_checkpoint("cp-a", "thread-a", 0))
            .await
            .unwrap();
        store
            .put(make_checkpoint("cp-b", "thread-b", 0))
            .await
            .unwrap();

        assert!(store.get("thread-a", "cp-a").await.unwrap().is_some());
        assert!(store.get("thread-a", "cp-b").await.unwrap().is_none());
        assert!(store.get("thread-b", "cp-b").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn delete_thread() {
        let store = MemoryCheckpointStore::new();
        store
            .put(make_checkpoint("cp-0", "thread-1", 0))
            .await
            .unwrap();
        store
            .put(make_checkpoint("cp-1", "thread-1", 1))
            .await
            .unwrap();

        store.delete_thread("thread-1").await.unwrap();
        assert!(store.get_latest("thread-1").await.unwrap().is_none());
        assert!(store.list("thread-1").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn overwrite_existing_checkpoint() {
        let store = MemoryCheckpointStore::new();
        let mut cp = make_checkpoint("cp-0", "thread-1", 0);
        store.put(cp.clone()).await.unwrap();

        cp.channel_values
            .insert("count".into(), json!(999));
        store.put(cp).await.unwrap();

        let retrieved = store.get("thread-1", "cp-0").await.unwrap().unwrap();
        assert_eq!(retrieved.channel_values["count"], json!(999));

        // Should not create a duplicate
        assert_eq!(store.list("thread-1").await.unwrap().len(), 1);
    }
}
