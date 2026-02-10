//! E2E tests: multiple threads running same graph concurrently.
//!
//! Verifies that each thread has independent checkpoints and that
//! concurrent execution does not interfere.

use std::sync::Arc;

use ayas_checkpoint::prelude::*;
use ayas_core::config::RunnableConfig;
use ayas_graph::prelude::*;
use serde_json::{json, Value};

fn build_accumulating_graph() -> CompiledStateGraph {
    let mut g = StateGraph::new();
    g.add_last_value_channel("count", json!(0));
    g.add_last_value_channel("label", json!(""));

    g.add_node(NodeFn::new("init", |state: Value, _cfg| async move {
        let c = state["count"].as_i64().unwrap_or(0);
        Ok(json!({"count": c + 1}))
    }))
    .unwrap();
    g.add_node(NodeFn::new("process", |state: Value, _cfg| async move {
        let c = state["count"].as_i64().unwrap_or(0);
        let label = state["label"].as_str().unwrap_or("");
        Ok(json!({
            "count": c * 2,
            "label": format!("{label}-processed")
        }))
    }))
    .unwrap();
    g.add_node(NodeFn::new("finish", |state: Value, _cfg| async move {
        let c = state["count"].as_i64().unwrap_or(0);
        Ok(json!({"count": c + 100}))
    }))
    .unwrap();

    g.set_entry_point("init");
    g.add_edge("init", "process");
    g.add_edge("process", "finish");
    g.set_finish_point("finish");
    g.compile().unwrap()
}

#[tokio::test]
async fn separate_threads_have_independent_checkpoints() {
    let graph = build_accumulating_graph();
    let store = MemoryCheckpointStore::new();

    // Thread A: starts with count=5, label="alpha"
    let config_a = RunnableConfig::default().with_thread_id("thread-a");
    let result_a = graph
        .invoke_resumable(
            json!({"count": 5, "label": "alpha"}),
            &config_a,
            &store,
        )
        .await
        .unwrap();

    // Thread B: starts with count=10, label="beta"
    let config_b = RunnableConfig::default().with_thread_id("thread-b");
    let result_b = graph
        .invoke_resumable(
            json!({"count": 10, "label": "beta"}),
            &config_b,
            &store,
        )
        .await
        .unwrap();

    // Results should be different
    let val_a = result_a.into_value();
    let val_b = result_b.into_value();

    // Thread A: 5 -> init(6) -> process(12, "alpha-processed") -> finish(112)
    assert_eq!(val_a["count"], json!(112));
    assert_eq!(val_a["label"], json!("alpha-processed"));

    // Thread B: 10 -> init(11) -> process(22, "beta-processed") -> finish(122)
    assert_eq!(val_b["count"], json!(122));
    assert_eq!(val_b["label"], json!("beta-processed"));

    // Each thread has its own 3 checkpoints
    let cps_a = store.list("thread-a").await.unwrap();
    let cps_b = store.list("thread-b").await.unwrap();
    assert_eq!(cps_a.len(), 3);
    assert_eq!(cps_b.len(), 3);

    // Thread IDs are isolated
    assert!(cps_a.iter().all(|cp| cp.thread_id == "thread-a"));
    assert!(cps_b.iter().all(|cp| cp.thread_id == "thread-b"));
}

#[tokio::test]
async fn checkpoint_ids_are_unique_across_threads() {
    let graph = build_accumulating_graph();
    let store = MemoryCheckpointStore::new();

    let config_a = RunnableConfig::default().with_thread_id("unique-a");
    let config_b = RunnableConfig::default().with_thread_id("unique-b");

    graph
        .invoke_resumable(json!({}), &config_a, &store)
        .await
        .unwrap();
    graph
        .invoke_resumable(json!({}), &config_b, &store)
        .await
        .unwrap();

    let ids_a: Vec<String> = store
        .list("unique-a")
        .await
        .unwrap()
        .iter()
        .map(|cp| cp.id.clone())
        .collect();
    let ids_b: Vec<String> = store
        .list("unique-b")
        .await
        .unwrap()
        .iter()
        .map(|cp| cp.id.clone())
        .collect();

    for id in &ids_a {
        assert!(!ids_b.contains(id), "Checkpoint ID collision: {id}");
    }
}

#[tokio::test]
async fn concurrent_threads_with_shared_store() {
    let graph = Arc::new(build_accumulating_graph());
    let store = Arc::new(MemoryCheckpointStore::new());

    let mut handles = Vec::new();
    for i in 0..5 {
        let g = graph.clone();
        let s = store.clone();
        handles.push(tokio::spawn(async move {
            let thread_id = format!("concurrent-{i}");
            let config = RunnableConfig::default().with_thread_id(&thread_id);
            let result = g
                .invoke_resumable(json!({"count": i}), &config, s.as_ref())
                .await
                .unwrap();
            (thread_id, result)
        }));
    }

    let results: Vec<_> = futures::future::join_all(handles)
        .await
        .into_iter()
        .map(|r| r.unwrap())
        .collect();

    // Each thread should complete with its own result
    for (thread_id, result) in &results {
        assert!(result.is_complete(), "Thread {thread_id} did not complete");
    }

    // Each thread should have exactly 3 checkpoints
    for i in 0..5 {
        let thread_id = format!("concurrent-{i}");
        let cps = store.list(&thread_id).await.unwrap();
        assert_eq!(cps.len(), 3, "Thread {thread_id} checkpoint count mismatch");
    }
}

#[tokio::test]
async fn resume_on_one_thread_does_not_affect_other() {
    let graph = build_accumulating_graph();
    let store = MemoryCheckpointStore::new();

    // Run both threads
    let config_a = RunnableConfig::default().with_thread_id("isolated-a");
    let config_b = RunnableConfig::default().with_thread_id("isolated-b");

    graph
        .invoke_resumable(json!({"count": 1}), &config_a, &store)
        .await
        .unwrap();
    graph
        .invoke_resumable(json!({"count": 100}), &config_b, &store)
        .await
        .unwrap();

    // Get thread-a checkpoint after init
    let cps_a = store.list("isolated-a").await.unwrap();
    let after_init_a = cps_a
        .iter()
        .find(|cp| cp.metadata.node_name.as_deref() == Some("init"))
        .unwrap();

    // Resume thread-a from init
    let resume_config = RunnableConfig::default()
        .with_thread_id("isolated-a")
        .with_checkpoint_id(&after_init_a.id);

    graph
        .invoke_resumable(json!({}), &resume_config, &store)
        .await
        .unwrap();

    // Thread-b should still have exactly 3 checkpoints, unmodified
    let cps_b = store.list("isolated-b").await.unwrap();
    assert_eq!(cps_b.len(), 3);
    assert!(cps_b.iter().all(|cp| cp.thread_id == "isolated-b"));
}

#[tokio::test]
async fn delete_one_thread_preserves_other() {
    let graph = build_accumulating_graph();
    let store = MemoryCheckpointStore::new();

    let config_a = RunnableConfig::default().with_thread_id("delete-a");
    let config_b = RunnableConfig::default().with_thread_id("delete-b");

    graph
        .invoke_resumable(json!({}), &config_a, &store)
        .await
        .unwrap();
    graph
        .invoke_resumable(json!({}), &config_b, &store)
        .await
        .unwrap();

    // Delete thread-a
    store.delete_thread("delete-a").await.unwrap();

    assert!(store.list("delete-a").await.unwrap().is_empty());
    assert_eq!(store.list("delete-b").await.unwrap().len(), 3);
}
