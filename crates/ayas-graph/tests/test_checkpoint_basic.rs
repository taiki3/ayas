//! E2E tests: basic checkpoint functionality.
//!
//! Verifies that checkpoints are saved at each step during graph execution
//! and that resuming from a middle checkpoint produces the correct final state.

use ayas_checkpoint::prelude::*;
use ayas_core::config::RunnableConfig;
use ayas_graph::prelude::*;
use serde_json::{json, Value};

/// Build a 3-node linear graph: a → b → c
/// Each node increments `count` by 1.
fn build_linear_graph() -> CompiledStateGraph {
    let mut g = StateGraph::new();
    g.add_last_value_channel("count", json!(0));

    g.add_node(NodeFn::new("a", |_state: Value, _cfg| async move {
        Ok(json!({"count": 1}))
    }))
    .unwrap();
    g.add_node(NodeFn::new("b", |state: Value, _cfg| async move {
        let c = state["count"].as_i64().unwrap_or(0);
        Ok(json!({"count": c + 1}))
    }))
    .unwrap();
    g.add_node(NodeFn::new("c", |state: Value, _cfg| async move {
        let c = state["count"].as_i64().unwrap_or(0);
        Ok(json!({"count": c + 1}))
    }))
    .unwrap();

    g.set_entry_point("a");
    g.add_edge("a", "b");
    g.add_edge("b", "c");
    g.set_finish_point("c");
    g.compile().unwrap()
}

#[tokio::test]
async fn checkpoint_saved_at_each_step() {
    let graph = build_linear_graph();
    let store = MemoryCheckpointStore::new();
    let config = RunnableConfig::default().with_thread_id("basic-each-step");

    let result = graph
        .invoke_resumable(json!({}), &config, &store)
        .await
        .unwrap();

    assert!(result.is_complete());
    assert_eq!(result.into_value()["count"], json!(3));

    // One checkpoint per node: a, b, c
    let checkpoints = store.list("basic-each-step").await.unwrap();
    assert_eq!(checkpoints.len(), 3);

    // Checkpoints are ordered by step
    assert_eq!(checkpoints[0].step, 0);
    assert_eq!(checkpoints[1].step, 1);
    assert_eq!(checkpoints[2].step, 2);

    // Each checkpoint records which node produced it
    assert_eq!(
        checkpoints[0].metadata.node_name.as_deref(),
        Some("a")
    );
    assert_eq!(
        checkpoints[1].metadata.node_name.as_deref(),
        Some("b")
    );
    assert_eq!(
        checkpoints[2].metadata.node_name.as_deref(),
        Some("c")
    );
}

#[tokio::test]
async fn checkpoint_channel_values_match_state() {
    let graph = build_linear_graph();
    let store = MemoryCheckpointStore::new();
    let config = RunnableConfig::default().with_thread_id("basic-channel-vals");

    graph
        .invoke_resumable(json!({}), &config, &store)
        .await
        .unwrap();

    let checkpoints = store.list("basic-channel-vals").await.unwrap();

    // After node a: count = 1
    assert_eq!(checkpoints[0].channel_values["count"], json!(1));
    // After node b: count = 2
    assert_eq!(checkpoints[1].channel_values["count"], json!(2));
    // After node c: count = 3
    assert_eq!(checkpoints[2].channel_values["count"], json!(3));
}

#[tokio::test]
async fn checkpoint_metadata_source_is_loop() {
    let graph = build_linear_graph();
    let store = MemoryCheckpointStore::new();
    let config = RunnableConfig::default().with_thread_id("basic-metadata");

    graph
        .invoke_resumable(json!({}), &config, &store)
        .await
        .unwrap();

    let checkpoints = store.list("basic-metadata").await.unwrap();
    for cp in &checkpoints {
        assert_eq!(cp.metadata.source, "loop");
    }
}

#[tokio::test]
async fn checkpoint_parent_chain() {
    let graph = build_linear_graph();
    let store = MemoryCheckpointStore::new();
    let config = RunnableConfig::default().with_thread_id("basic-parent-chain");

    graph
        .invoke_resumable(json!({}), &config, &store)
        .await
        .unwrap();

    let checkpoints = store.list("basic-parent-chain").await.unwrap();

    // First checkpoint has no parent
    assert!(checkpoints[0].parent_id.is_none());
    // Second's parent is the first
    assert_eq!(checkpoints[1].parent_id.as_deref(), Some(checkpoints[0].id.as_str()));
    // Third's parent is the second
    assert_eq!(checkpoints[2].parent_id.as_deref(), Some(checkpoints[1].id.as_str()));
}

#[tokio::test]
async fn resume_from_middle_checkpoint() {
    let graph = build_linear_graph();
    let store = MemoryCheckpointStore::new();
    let config = RunnableConfig::default().with_thread_id("basic-resume-mid");

    // Run graph to completion
    let result = graph
        .invoke_resumable(json!({}), &config, &store)
        .await
        .unwrap();
    assert!(result.is_complete());
    assert_eq!(result.into_value()["count"], json!(3));

    // Get checkpoint after node "a" (step 0)
    let checkpoints = store.list("basic-resume-mid").await.unwrap();
    let after_a = checkpoints
        .iter()
        .find(|cp| cp.metadata.node_name.as_deref() == Some("a"))
        .unwrap();

    // Resume from after node a — should execute b and c
    let resume_config = RunnableConfig::default()
        .with_thread_id("basic-resume-mid")
        .with_checkpoint_id(&after_a.id);

    let result = graph
        .invoke_resumable(json!({}), &resume_config, &store)
        .await
        .unwrap();

    assert!(result.is_complete());
    // count=1 (restored from checkpoint) → b makes 2 → c makes 3
    assert_eq!(result.into_value()["count"], json!(3));
}

#[tokio::test]
async fn resume_from_second_checkpoint() {
    let graph = build_linear_graph();
    let store = MemoryCheckpointStore::new();
    let config = RunnableConfig::default().with_thread_id("basic-resume-b");

    graph
        .invoke_resumable(json!({}), &config, &store)
        .await
        .unwrap();

    // Get checkpoint after node "b" (step 1)
    let checkpoints = store.list("basic-resume-b").await.unwrap();
    let after_b = checkpoints
        .iter()
        .find(|cp| cp.metadata.node_name.as_deref() == Some("b"))
        .unwrap();

    // Resume from after node b — should only execute c
    let resume_config = RunnableConfig::default()
        .with_thread_id("basic-resume-b")
        .with_checkpoint_id(&after_b.id);

    let result = graph
        .invoke_resumable(json!({}), &resume_config, &store)
        .await
        .unwrap();

    assert!(result.is_complete());
    // count=2 (restored) → c makes 3
    assert_eq!(result.into_value()["count"], json!(3));
}

#[tokio::test]
async fn resume_from_nonexistent_checkpoint_errors() {
    let graph = build_linear_graph();
    let store = MemoryCheckpointStore::new();
    let config = RunnableConfig::default()
        .with_thread_id("basic-noexist")
        .with_checkpoint_id("does-not-exist");

    let result = graph.invoke_resumable(json!({}), &config, &store).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("not found"));
}

#[tokio::test]
async fn resume_adds_more_checkpoints() {
    let graph = build_linear_graph();
    let store = MemoryCheckpointStore::new();
    let config = RunnableConfig::default().with_thread_id("basic-resume-more");

    graph
        .invoke_resumable(json!({}), &config, &store)
        .await
        .unwrap();

    // 3 checkpoints from initial run
    let initial_count = store.list("basic-resume-more").await.unwrap().len();
    assert_eq!(initial_count, 3);

    // Resume from after node a
    let checkpoints = store.list("basic-resume-more").await.unwrap();
    let after_a = checkpoints
        .iter()
        .find(|cp| cp.metadata.node_name.as_deref() == Some("a"))
        .unwrap();

    let resume_config = RunnableConfig::default()
        .with_thread_id("basic-resume-more")
        .with_checkpoint_id(&after_a.id);

    graph
        .invoke_resumable(json!({}), &resume_config, &store)
        .await
        .unwrap();

    // 3 original + 2 from resume (b and c)
    let final_count = store.list("basic-resume-more").await.unwrap().len();
    assert_eq!(final_count, 5);
}

#[tokio::test]
async fn checkpoint_with_initial_input() {
    let graph = build_linear_graph();
    let store = MemoryCheckpointStore::new();
    let config = RunnableConfig::default().with_thread_id("basic-with-input");

    // Start with count=10
    let result = graph
        .invoke_resumable(json!({"count": 10}), &config, &store)
        .await
        .unwrap();

    assert!(result.is_complete());
    // Node a overwrites count to 1, b makes 2, c makes 3
    // (node a ignores state, always sets count=1)
    assert_eq!(result.into_value()["count"], json!(3));
}
