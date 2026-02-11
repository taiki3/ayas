use std::collections::HashMap;

use async_trait::async_trait;
use serde_json::Value;
use tokio_postgres::{Client, NoTls};

use ayas_core::error::{AyasError, Result};

use crate::store::CheckpointStore;
use crate::types::{Checkpoint, CheckpointMetadata};

/// PostgreSQL-backed checkpoint store.
///
/// Feature-gated behind `postgres` feature flag.
/// Falls back gracefully if the connection fails.
pub struct PostgresCheckpointStore {
    client: Client,
    _handle: tokio::task::JoinHandle<()>,
}

impl PostgresCheckpointStore {
    /// Connect to PostgreSQL using the `DATABASE_URL` environment variable.
    pub async fn from_env() -> Result<Self> {
        let url = std::env::var("DATABASE_URL").map_err(|_| {
            AyasError::Other("DATABASE_URL environment variable not set".into())
        })?;
        Self::connect(&url).await
    }

    /// Connect to PostgreSQL using the given connection URL.
    pub async fn connect(url: &str) -> Result<Self> {
        let (client, connection) = tokio_postgres::connect(url, NoTls)
            .await
            .map_err(|e| AyasError::Other(format!("PostgreSQL connection error: {e}")))?;

        let handle = tokio::spawn(async move {
            if let Err(e) = connection.await {
                tracing::error!("PostgreSQL connection error: {e}");
            }
        });

        let store = Self {
            client,
            _handle: handle,
        };
        store.create_table().await?;
        Ok(store)
    }

    async fn create_table(&self) -> Result<()> {
        self.client
            .execute(
                "CREATE TABLE IF NOT EXISTS checkpoints (
                    id TEXT NOT NULL,
                    thread_id TEXT NOT NULL,
                    parent_id TEXT,
                    step INTEGER NOT NULL,
                    channel_values JSONB NOT NULL,
                    pending_nodes JSONB NOT NULL,
                    metadata JSONB NOT NULL,
                    created_at TIMESTAMPTZ NOT NULL,
                    PRIMARY KEY (thread_id, id)
                )",
                &[],
            )
            .await
            .map_err(|e| AyasError::Other(format!("PostgreSQL create table error: {e}")))?;

        self.client
            .execute(
                "CREATE INDEX IF NOT EXISTS idx_checkpoints_thread_step
                 ON checkpoints (thread_id, step)",
                &[],
            )
            .await
            .map_err(|e| AyasError::Other(format!("PostgreSQL create index error: {e}")))?;

        Ok(())
    }
}

#[async_trait]
impl CheckpointStore for PostgresCheckpointStore {
    async fn put(&self, checkpoint: Checkpoint) -> Result<()> {
        let channel_values = serde_json::to_value(&checkpoint.channel_values)
            .map_err(|e| AyasError::Other(e.to_string()))?;
        let pending_nodes = serde_json::to_value(&checkpoint.pending_nodes)
            .map_err(|e| AyasError::Other(e.to_string()))?;
        let metadata = serde_json::to_value(&checkpoint.metadata)
            .map_err(|e| AyasError::Other(e.to_string()))?;

        self.client
            .execute(
                "INSERT INTO checkpoints (id, thread_id, parent_id, step, channel_values, pending_nodes, metadata, created_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                 ON CONFLICT (thread_id, id) DO UPDATE SET
                     parent_id = EXCLUDED.parent_id,
                     step = EXCLUDED.step,
                     channel_values = EXCLUDED.channel_values,
                     pending_nodes = EXCLUDED.pending_nodes,
                     metadata = EXCLUDED.metadata,
                     created_at = EXCLUDED.created_at",
                &[
                    &checkpoint.id,
                    &checkpoint.thread_id,
                    &checkpoint.parent_id,
                    &(checkpoint.step as i32),
                    &channel_values,
                    &pending_nodes,
                    &metadata,
                    &checkpoint.created_at,
                ],
            )
            .await
            .map_err(|e| AyasError::Other(format!("PostgreSQL put error: {e}")))?;

        Ok(())
    }

    async fn get(&self, thread_id: &str, checkpoint_id: &str) -> Result<Option<Checkpoint>> {
        let row = self
            .client
            .query_opt(
                "SELECT id, thread_id, parent_id, step, channel_values, pending_nodes, metadata, created_at
                 FROM checkpoints
                 WHERE thread_id = $1 AND id = $2",
                &[&thread_id, &checkpoint_id],
            )
            .await
            .map_err(|e| AyasError::Other(format!("PostgreSQL get error: {e}")))?;

        match row {
            Some(row) => Ok(Some(row_to_checkpoint(&row)?)),
            None => Ok(None),
        }
    }

    async fn get_latest(&self, thread_id: &str) -> Result<Option<Checkpoint>> {
        let row = self
            .client
            .query_opt(
                "SELECT id, thread_id, parent_id, step, channel_values, pending_nodes, metadata, created_at
                 FROM checkpoints
                 WHERE thread_id = $1
                 ORDER BY step DESC
                 LIMIT 1",
                &[&thread_id],
            )
            .await
            .map_err(|e| AyasError::Other(format!("PostgreSQL get_latest error: {e}")))?;

        match row {
            Some(row) => Ok(Some(row_to_checkpoint(&row)?)),
            None => Ok(None),
        }
    }

    async fn list(&self, thread_id: &str) -> Result<Vec<Checkpoint>> {
        let rows = self
            .client
            .query(
                "SELECT id, thread_id, parent_id, step, channel_values, pending_nodes, metadata, created_at
                 FROM checkpoints
                 WHERE thread_id = $1
                 ORDER BY step ASC",
                &[&thread_id],
            )
            .await
            .map_err(|e| AyasError::Other(format!("PostgreSQL list error: {e}")))?;

        rows.iter().map(row_to_checkpoint).collect()
    }

    async fn delete_thread(&self, thread_id: &str) -> Result<()> {
        self.client
            .execute(
                "DELETE FROM checkpoints WHERE thread_id = $1",
                &[&thread_id],
            )
            .await
            .map_err(|e| AyasError::Other(format!("PostgreSQL delete error: {e}")))?;

        Ok(())
    }
}

fn row_to_checkpoint(row: &tokio_postgres::Row) -> Result<Checkpoint> {
    let channel_values_json: Value = row.get("channel_values");
    let channel_values: HashMap<String, Value> =
        serde_json::from_value(channel_values_json).unwrap_or_default();

    let pending_nodes_json: Value = row.get("pending_nodes");
    let pending_nodes: Vec<String> =
        serde_json::from_value(pending_nodes_json).unwrap_or_default();

    let metadata_json: Value = row.get("metadata");
    let metadata: CheckpointMetadata = serde_json::from_value(metadata_json).map_err(|e| {
        AyasError::Other(format!("Failed to parse checkpoint metadata: {e}"))
    })?;

    let step: i32 = row.get("step");

    Ok(Checkpoint {
        id: row.get("id"),
        thread_id: row.get("thread_id"),
        parent_id: row.get("parent_id"),
        step: step as usize,
        channel_values,
        pending_nodes,
        metadata,
        created_at: row.get("created_at"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkpoint_metadata_serde() {
        let meta = CheckpointMetadata {
            source: "loop".into(),
            step: 3,
            node_name: Some("agent".into()),
        };
        let json = serde_json::to_value(&meta).unwrap();
        let parsed: CheckpointMetadata = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.source, "loop");
        assert_eq!(parsed.step, 3);
        assert_eq!(parsed.node_name.as_deref(), Some("agent"));
    }

    #[test]
    fn checkpoint_channel_values_serde() {
        let mut channel_values = HashMap::new();
        channel_values.insert("messages".into(), serde_json::json!(["hello"]));
        channel_values.insert("count".into(), serde_json::json!(42));

        let json = serde_json::to_value(&channel_values).unwrap();
        let parsed: HashMap<String, Value> = serde_json::from_value(json).unwrap();
        assert_eq!(parsed["count"], serde_json::json!(42));
    }

    // Integration tests with actual PostgreSQL would be #[ignore]
    #[test]
    fn missing_database_url_errors() {
        let original = std::env::var("DATABASE_URL").ok();
        unsafe { std::env::remove_var("DATABASE_URL") };

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(PostgresCheckpointStore::from_env());
        assert!(result.is_err());

        if let Some(url) = original {
            unsafe { std::env::set_var("DATABASE_URL", url) };
        }
    }
}
