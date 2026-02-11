//! Integration tests for PostgresCheckpointStore.
//!
//! Requires: `docker compose -f docker-compose.test.yml up -d postgres`
//! Run with: `cargo test -p ayas-checkpoint --features postgres --test integration_postgres`

#![cfg(feature = "postgres")]

use std::collections::HashMap;

use chrono::Utc;
use serde_json::json;

use ayas_checkpoint::postgres::PostgresCheckpointStore;
use ayas_checkpoint::store::CheckpointStore;
use ayas_checkpoint::types::{Checkpoint, CheckpointMetadata};

const TEST_URL: &str = "host=localhost port=15432 user=ayas password=ayas dbname=ayas_test";

async fn setup() -> PostgresCheckpointStore {
    PostgresCheckpointStore::connect(TEST_URL)
        .await
        .expect("Failed to connect to PostgreSQL — is docker-compose.test.yml running?")
}

fn make_checkpoint(thread_id: &str, id: &str, step: usize) -> Checkpoint {
    let mut channel_values = HashMap::new();
    channel_values.insert("messages".into(), json!(["hello"]));
    channel_values.insert("count".into(), json!(step));

    Checkpoint {
        id: id.into(),
        thread_id: thread_id.into(),
        parent_id: if step > 0 {
            Some(format!("cp-{}", step - 1))
        } else {
            None
        },
        step,
        channel_values,
        pending_nodes: vec![],
        metadata: CheckpointMetadata {
            source: "loop".into(),
            step,
            node_name: Some("agent".into()),
        },
        created_at: Utc::now(),
    }
}

#[tokio::test]
async fn put_and_get() {
    let store = setup().await;
    let thread_id = format!("thread-{}", uuid::Uuid::new_v4());

    let cp = make_checkpoint(&thread_id, "cp-0", 0);
    store.put(cp.clone()).await.unwrap();

    let fetched = store.get(&thread_id, "cp-0").await.unwrap();
    assert!(fetched.is_some());
    let fetched = fetched.unwrap();
    assert_eq!(fetched.id, "cp-0");
    assert_eq!(fetched.thread_id, thread_id);
    assert_eq!(fetched.step, 0);
    assert_eq!(fetched.metadata.source, "loop");
}

#[tokio::test]
async fn get_latest() {
    let store = setup().await;
    let thread_id = format!("thread-{}", uuid::Uuid::new_v4());

    for i in 0..5 {
        let cp = make_checkpoint(&thread_id, &format!("cp-{i}"), i);
        store.put(cp).await.unwrap();
    }

    let latest = store.get_latest(&thread_id).await.unwrap();
    assert!(latest.is_some());
    assert_eq!(latest.unwrap().step, 4);
}

#[tokio::test]
async fn list_checkpoints() {
    let store = setup().await;
    let thread_id = format!("thread-{}", uuid::Uuid::new_v4());

    for i in 0..3 {
        let cp = make_checkpoint(&thread_id, &format!("cp-{i}"), i);
        store.put(cp).await.unwrap();
    }

    let list = store.list(&thread_id).await.unwrap();
    assert_eq!(list.len(), 3);
    // Should be ordered by step ascending
    assert_eq!(list[0].step, 0);
    assert_eq!(list[1].step, 1);
    assert_eq!(list[2].step, 2);
}

#[tokio::test]
async fn delete_thread() {
    let store = setup().await;
    let thread_id = format!("thread-{}", uuid::Uuid::new_v4());

    for i in 0..3 {
        let cp = make_checkpoint(&thread_id, &format!("cp-{i}"), i);
        store.put(cp).await.unwrap();
    }

    let before = store.list(&thread_id).await.unwrap();
    assert_eq!(before.len(), 3);

    store.delete_thread(&thread_id).await.unwrap();

    let after = store.list(&thread_id).await.unwrap();
    assert_eq!(after.len(), 0);
}

#[tokio::test]
async fn upsert_overwrites() {
    let store = setup().await;
    let thread_id = format!("thread-{}", uuid::Uuid::new_v4());

    let mut cp = make_checkpoint(&thread_id, "cp-0", 0);
    store.put(cp.clone()).await.unwrap();

    // Update the channel_values
    cp.channel_values
        .insert("count".into(), json!(999));
    store.put(cp).await.unwrap();

    let fetched = store.get(&thread_id, "cp-0").await.unwrap().unwrap();
    assert_eq!(fetched.channel_values["count"], json!(999));
}

#[tokio::test]
async fn get_nonexistent() {
    let store = setup().await;
    let result = store.get("no-such-thread", "no-such-id").await.unwrap();
    assert!(result.is_none());
}
