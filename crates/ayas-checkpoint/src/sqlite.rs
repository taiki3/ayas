use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

use ayas_core::error::{GraphError, Result};

use crate::store::CheckpointStore;
use crate::types::{Checkpoint, CheckpointMetadata};

/// SQLite-backed checkpoint store for durable persistence.
///
/// Thread-safe via `Arc<Mutex<Connection>>`. All SQLite operations are
/// dispatched to a blocking thread via `tokio::task::spawn_blocking`.
pub struct SqliteCheckpointStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteCheckpointStore {
    /// Open (or create) a SQLite database at the given path.
    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path)
            .map_err(|e| GraphError::Checkpoint(format!("failed to open database: {e}")))?;
        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        store.create_table()?;
        Ok(store)
    }

    /// Create an in-memory SQLite database (useful for tests).
    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()
            .map_err(|e| GraphError::Checkpoint(format!("failed to open in-memory db: {e}")))?;
        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        store.create_table()?;
        Ok(store)
    }

    fn create_table(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS checkpoints (
                id TEXT NOT NULL,
                thread_id TEXT NOT NULL,
                parent_id TEXT,
                step INTEGER NOT NULL,
                channel_values TEXT NOT NULL,
                pending_nodes TEXT NOT NULL,
                metadata TEXT NOT NULL,
                created_at TEXT NOT NULL,
                PRIMARY KEY (thread_id, id)
            );
            CREATE INDEX IF NOT EXISTS idx_checkpoints_thread
                ON checkpoints(thread_id, step);",
        )
        .map_err(|e| GraphError::Checkpoint(format!("failed to create table: {e}")))?;
        Ok(())
    }
}

fn row_to_checkpoint(row: &rusqlite::Row<'_>) -> rusqlite::Result<Checkpoint> {
    let id: String = row.get(0)?;
    let thread_id: String = row.get(1)?;
    let parent_id: Option<String> = row.get(2)?;
    let step: i64 = row.get(3)?;
    let channel_values_json: String = row.get(4)?;
    let pending_nodes_json: String = row.get(5)?;
    let metadata_json: String = row.get(6)?;
    let created_at_str: String = row.get(7)?;

    let channel_values: HashMap<String, Value> =
        serde_json::from_str(&channel_values_json).unwrap_or_default();
    let pending_nodes: Vec<String> =
        serde_json::from_str(&pending_nodes_json).unwrap_or_default();
    let metadata: CheckpointMetadata =
        serde_json::from_str(&metadata_json).unwrap_or(CheckpointMetadata {
            source: "unknown".into(),
            step: step as usize,
            node_name: None,
        });
    let created_at: DateTime<Utc> = created_at_str
        .parse()
        .unwrap_or_else(|_| Utc::now());

    Ok(Checkpoint {
        id,
        thread_id,
        parent_id,
        step: step as usize,
        channel_values,
        pending_nodes,
        metadata,
        created_at,
    })
}

#[async_trait]
impl CheckpointStore for SqliteCheckpointStore {
    async fn put(&self, checkpoint: Checkpoint) -> Result<()> {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().unwrap();
            let channel_values_json = serde_json::to_string(&checkpoint.channel_values)
                .map_err(|e| GraphError::Checkpoint(format!("serialize channel_values: {e}")))?;
            let pending_nodes_json = serde_json::to_string(&checkpoint.pending_nodes)
                .map_err(|e| GraphError::Checkpoint(format!("serialize pending_nodes: {e}")))?;
            let metadata_json = serde_json::to_string(&checkpoint.metadata)
                .map_err(|e| GraphError::Checkpoint(format!("serialize metadata: {e}")))?;
            let created_at_str = checkpoint.created_at.to_rfc3339();

            conn.execute(
                "INSERT OR REPLACE INTO checkpoints
                    (id, thread_id, parent_id, step, channel_values, pending_nodes, metadata, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    checkpoint.id,
                    checkpoint.thread_id,
                    checkpoint.parent_id,
                    checkpoint.step as i64,
                    channel_values_json,
                    pending_nodes_json,
                    metadata_json,
                    created_at_str,
                ],
            )
            .map_err(|e| GraphError::Checkpoint(format!("insert checkpoint: {e}")))?;

            Ok(())
        })
        .await
        .map_err(|e| GraphError::Checkpoint(format!("spawn_blocking: {e}")))?
    }

    async fn get(&self, thread_id: &str, checkpoint_id: &str) -> Result<Option<Checkpoint>> {
        let conn = Arc::clone(&self.conn);
        let thread_id = thread_id.to_owned();
        let checkpoint_id = checkpoint_id.to_owned();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().unwrap();
            let mut stmt = conn
                .prepare(
                    "SELECT id, thread_id, parent_id, step, channel_values, pending_nodes, metadata, created_at
                     FROM checkpoints
                     WHERE thread_id = ?1 AND id = ?2",
                )
                .map_err(|e| GraphError::Checkpoint(format!("prepare: {e}")))?;

            let result = stmt
                .query_row(params![thread_id, checkpoint_id], row_to_checkpoint)
                .optional()
                .map_err(|e| GraphError::Checkpoint(format!("query: {e}")))?;

            Ok(result)
        })
        .await
        .map_err(|e| GraphError::Checkpoint(format!("spawn_blocking: {e}")))?
    }

    async fn get_latest(&self, thread_id: &str) -> Result<Option<Checkpoint>> {
        let conn = Arc::clone(&self.conn);
        let thread_id = thread_id.to_owned();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().unwrap();
            let mut stmt = conn
                .prepare(
                    "SELECT id, thread_id, parent_id, step, channel_values, pending_nodes, metadata, created_at
                     FROM checkpoints
                     WHERE thread_id = ?1
                     ORDER BY step DESC
                     LIMIT 1",
                )
                .map_err(|e| GraphError::Checkpoint(format!("prepare: {e}")))?;

            let result = stmt
                .query_row(params![thread_id], row_to_checkpoint)
                .optional()
                .map_err(|e| GraphError::Checkpoint(format!("query: {e}")))?;

            Ok(result)
        })
        .await
        .map_err(|e| GraphError::Checkpoint(format!("spawn_blocking: {e}")))?
    }

    async fn list(&self, thread_id: &str) -> Result<Vec<Checkpoint>> {
        let conn = Arc::clone(&self.conn);
        let thread_id = thread_id.to_owned();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().unwrap();
            let mut stmt = conn
                .prepare(
                    "SELECT id, thread_id, parent_id, step, channel_values, pending_nodes, metadata, created_at
                     FROM checkpoints
                     WHERE thread_id = ?1
                     ORDER BY step ASC",
                )
                .map_err(|e| GraphError::Checkpoint(format!("prepare: {e}")))?;

            let rows = stmt
                .query_map(params![thread_id], row_to_checkpoint)
                .map_err(|e| GraphError::Checkpoint(format!("query: {e}")))?;

            let mut checkpoints = Vec::new();
            for row in rows {
                checkpoints.push(
                    row.map_err(|e| GraphError::Checkpoint(format!("read row: {e}")))?,
                );
            }

            Ok(checkpoints)
        })
        .await
        .map_err(|e| GraphError::Checkpoint(format!("spawn_blocking: {e}")))?
    }

    async fn delete_thread(&self, thread_id: &str) -> Result<()> {
        let conn = Arc::clone(&self.conn);
        let thread_id = thread_id.to_owned();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().unwrap();
            conn.execute(
                "DELETE FROM checkpoints WHERE thread_id = ?1",
                params![thread_id],
            )
            .map_err(|e| GraphError::Checkpoint(format!("delete: {e}")))?;
            Ok(())
        })
        .await
        .map_err(|e| GraphError::Checkpoint(format!("spawn_blocking: {e}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::CheckpointMetadata;
    use chrono::Utc;
    use serde_json::json;

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
            pending_nodes: vec![format!("node_{step}")],
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
        let store = SqliteCheckpointStore::in_memory().unwrap();
        let cp = make_checkpoint("cp-0", "thread-1", 0);
        store.put(cp.clone()).await.unwrap();

        let retrieved = store.get("thread-1", "cp-0").await.unwrap();
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.id, "cp-0");
        assert_eq!(retrieved.thread_id, "thread-1");
        assert_eq!(retrieved.step, 0);
        assert!(retrieved.parent_id.is_none());
    }

    #[tokio::test]
    async fn get_nonexistent() {
        let store = SqliteCheckpointStore::in_memory().unwrap();
        let result = store.get("no-thread", "no-cp").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn get_latest() {
        let store = SqliteCheckpointStore::in_memory().unwrap();
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
    async fn get_latest_empty_thread() {
        let store = SqliteCheckpointStore::in_memory().unwrap();
        let result = store.get_latest("no-thread").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn list_ordered_by_step() {
        let store = SqliteCheckpointStore::in_memory().unwrap();
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
    async fn list_empty_thread() {
        let store = SqliteCheckpointStore::in_memory().unwrap();
        let list = store.list("no-thread").await.unwrap();
        assert!(list.is_empty());
    }

    #[tokio::test]
    async fn separate_threads() {
        let store = SqliteCheckpointStore::in_memory().unwrap();
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
        assert!(store.get("thread-b", "cp-a").await.unwrap().is_none());

        let list_a = store.list("thread-a").await.unwrap();
        assert_eq!(list_a.len(), 1);
        let list_b = store.list("thread-b").await.unwrap();
        assert_eq!(list_b.len(), 1);
    }

    #[tokio::test]
    async fn delete_thread() {
        let store = SqliteCheckpointStore::in_memory().unwrap();
        store
            .put(make_checkpoint("cp-0", "thread-1", 0))
            .await
            .unwrap();
        store
            .put(make_checkpoint("cp-1", "thread-1", 1))
            .await
            .unwrap();
        // Also add to a different thread to ensure isolation
        store
            .put(make_checkpoint("cp-x", "thread-2", 0))
            .await
            .unwrap();

        store.delete_thread("thread-1").await.unwrap();
        assert!(store.get_latest("thread-1").await.unwrap().is_none());
        assert!(store.list("thread-1").await.unwrap().is_empty());

        // Other thread is unaffected
        assert!(store.get("thread-2", "cp-x").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn overwrite_existing_checkpoint() {
        let store = SqliteCheckpointStore::in_memory().unwrap();
        let mut cp = make_checkpoint("cp-0", "thread-1", 0);
        store.put(cp.clone()).await.unwrap();

        cp.channel_values.insert("count".into(), json!(999));
        store.put(cp).await.unwrap();

        let retrieved = store.get("thread-1", "cp-0").await.unwrap().unwrap();
        assert_eq!(retrieved.channel_values["count"], json!(999));

        // Should not create a duplicate
        assert_eq!(store.list("thread-1").await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn channel_values_roundtrip() {
        let store = SqliteCheckpointStore::in_memory().unwrap();
        let mut cp = make_checkpoint("cp-0", "thread-1", 0);
        cp.channel_values = HashMap::from([
            ("messages".into(), json!([{"role": "user", "content": "hello"}])),
            ("count".into(), json!(42)),
            ("nested".into(), json!({"a": {"b": [1, 2, 3]}})),
        ]);
        cp.pending_nodes = vec!["agent".into(), "tool".into()];
        store.put(cp.clone()).await.unwrap();

        let retrieved = store.get("thread-1", "cp-0").await.unwrap().unwrap();
        assert_eq!(retrieved.channel_values["count"], json!(42));
        assert_eq!(
            retrieved.channel_values["messages"],
            json!([{"role": "user", "content": "hello"}])
        );
        assert_eq!(
            retrieved.channel_values["nested"],
            json!({"a": {"b": [1, 2, 3]}})
        );
        assert_eq!(retrieved.pending_nodes, vec!["agent", "tool"]);
    }

    #[tokio::test]
    async fn metadata_roundtrip() {
        let store = SqliteCheckpointStore::in_memory().unwrap();
        let cp = make_checkpoint("cp-0", "thread-1", 0);
        store.put(cp.clone()).await.unwrap();

        let retrieved = store.get("thread-1", "cp-0").await.unwrap().unwrap();
        assert_eq!(retrieved.metadata.source, "loop");
        assert_eq!(retrieved.metadata.step, 0);
        assert_eq!(retrieved.metadata.node_name, Some("node_0".into()));
    }

    #[tokio::test]
    async fn delete_nonexistent_thread_is_ok() {
        let store = SqliteCheckpointStore::in_memory().unwrap();
        // Should not error
        store.delete_thread("nonexistent").await.unwrap();
    }
}
