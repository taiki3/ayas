use ayas_checkpoint::prelude::{Checkpoint, CheckpointMetadata, CheckpointStore};
use ayas_core::error::{GraphError, Result};
use chrono::Utc;
use uuid::Uuid;

/// Get the full state history (all checkpoints) for a thread, ordered by step.
pub async fn get_state_history(
    store: &dyn CheckpointStore,
    thread_id: &str,
) -> Result<Vec<Checkpoint>> {
    store.list(thread_id).await
}

/// Fork from a specific checkpoint to create a new thread branch.
///
/// Copies the checkpoint's channel values and pending nodes into a new
/// checkpoint on `new_thread_id`, allowing independent execution from that point.
pub async fn fork_from_checkpoint(
    store: &dyn CheckpointStore,
    source_thread_id: &str,
    checkpoint_id: &str,
    new_thread_id: &str,
) -> Result<()> {
    let checkpoint = store
        .get(source_thread_id, checkpoint_id)
        .await?
        .ok_or_else(|| {
            GraphError::Checkpoint(format!(
                "Checkpoint '{checkpoint_id}' not found for thread '{source_thread_id}'"
            ))
        })?;

    let new_checkpoint = Checkpoint {
        id: Uuid::new_v4().to_string(),
        thread_id: new_thread_id.to_string(),
        parent_id: Some(checkpoint.id.clone()),
        step: 0,
        channel_values: checkpoint.channel_values.clone(),
        pending_nodes: checkpoint.pending_nodes.clone(),
        metadata: CheckpointMetadata {
            source: "fork".into(),
            step: 0,
            node_name: checkpoint.metadata.node_name.clone(),
        },
        created_at: Utc::now(),
    };

    store.put(new_checkpoint).await
}

/// Retrieve the checkpoint at a specific step in a thread.
///
/// Returns `None` if no checkpoint exists at that step.
pub async fn replay_to_step(
    store: &dyn CheckpointStore,
    thread_id: &str,
    step: usize,
) -> Result<Option<Checkpoint>> {
    let checkpoints = store.list(thread_id).await?;
    Ok(checkpoints.into_iter().find(|cp| cp.step == step))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ayas_checkpoint::prelude::MemoryCheckpointStore;
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
            channel_values: HashMap::from([("count".into(), json!(step * 10))]),
            pending_nodes: vec![format!("node_{}", step + 1)],
            metadata: CheckpointMetadata {
                source: "loop".into(),
                step,
                node_name: Some(format!("node_{step}")),
            },
            created_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn test_get_state_history() {
        let store = MemoryCheckpointStore::new();
        store.put(make_checkpoint("cp-0", "t1", 0)).await.unwrap();
        store.put(make_checkpoint("cp-1", "t1", 1)).await.unwrap();
        store.put(make_checkpoint("cp-2", "t1", 2)).await.unwrap();

        let history = get_state_history(&store, "t1").await.unwrap();
        assert_eq!(history.len(), 3);
        assert_eq!(history[0].step, 0);
        assert_eq!(history[1].step, 1);
        assert_eq!(history[2].step, 2);
    }

    #[tokio::test]
    async fn test_get_state_history_empty() {
        let store = MemoryCheckpointStore::new();
        let history = get_state_history(&store, "nonexistent").await.unwrap();
        assert!(history.is_empty());
    }

    #[tokio::test]
    async fn test_fork_from_checkpoint() {
        let store = MemoryCheckpointStore::new();
        store.put(make_checkpoint("cp-0", "t1", 0)).await.unwrap();
        store.put(make_checkpoint("cp-1", "t1", 1)).await.unwrap();

        fork_from_checkpoint(&store, "t1", "cp-1", "t2")
            .await
            .unwrap();

        // New thread should have exactly one checkpoint
        let t2_checkpoints = store.list("t2").await.unwrap();
        assert_eq!(t2_checkpoints.len(), 1);

        let forked = &t2_checkpoints[0];
        assert_eq!(forked.thread_id, "t2");
        assert_eq!(forked.step, 0);
        assert_eq!(forked.metadata.source, "fork");
        // Channel values are copied from source
        assert_eq!(forked.channel_values["count"], json!(10));
        // Parent ID points to the source checkpoint
        assert_eq!(forked.parent_id.as_deref(), Some("cp-1"));
        // Pending nodes are preserved
        assert_eq!(forked.pending_nodes, vec!["node_2".to_string()]);

        // Original thread should be unmodified
        let t1_checkpoints = store.list("t1").await.unwrap();
        assert_eq!(t1_checkpoints.len(), 2);
    }

    #[tokio::test]
    async fn test_fork_from_nonexistent_checkpoint() {
        let store = MemoryCheckpointStore::new();
        store.put(make_checkpoint("cp-0", "t1", 0)).await.unwrap();

        let result = fork_from_checkpoint(&store, "t1", "no-such-cp", "t2").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_replay_to_step() {
        let store = MemoryCheckpointStore::new();
        store.put(make_checkpoint("cp-0", "t1", 0)).await.unwrap();
        store.put(make_checkpoint("cp-1", "t1", 1)).await.unwrap();
        store.put(make_checkpoint("cp-2", "t1", 2)).await.unwrap();

        let cp = replay_to_step(&store, "t1", 1).await.unwrap();
        assert!(cp.is_some());
        let cp = cp.unwrap();
        assert_eq!(cp.id, "cp-1");
        assert_eq!(cp.step, 1);
        assert_eq!(cp.channel_values["count"], json!(10));
    }

    #[tokio::test]
    async fn test_replay_to_step_not_found() {
        let store = MemoryCheckpointStore::new();
        store.put(make_checkpoint("cp-0", "t1", 0)).await.unwrap();

        let cp = replay_to_step(&store, "t1", 99).await.unwrap();
        assert!(cp.is_none());
    }
}
