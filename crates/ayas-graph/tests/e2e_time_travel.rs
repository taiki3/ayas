//! E2E tests for graph time-travel and breakpoint workflows.
//!
//! Covers the full lifecycle: execute with checkpoints, get history, fork,
//! replay, breakpoint-driven interrupts, resume, and thread isolation.

use ayas_checkpoint::prelude::*;
use ayas_core::config::RunnableConfig;
use ayas_graph::prelude::*;
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Graph builders
// ---------------------------------------------------------------------------

/// 3-node linear graph: a → b → c.  Each node increments `count` by 1.
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

/// 4-node linear graph: a → b → c → d.  Tracks `count` and `path`.
fn build_4node_graph() -> CompiledStateGraph {
    let mut g = StateGraph::new();
    g.add_last_value_channel("count", json!(0));
    g.add_last_value_channel("path", json!(""));

    g.add_node(NodeFn::new("a", |_state: Value, _cfg| async move {
        Ok(json!({"count": 1, "path": "a"}))
    }))
    .unwrap();
    g.add_node(NodeFn::new("b", |state: Value, _cfg| async move {
        let c = state["count"].as_i64().unwrap_or(0);
        let p = state["path"].as_str().unwrap_or("");
        Ok(json!({"count": c + 1, "path": format!("{p}->b")}))
    }))
    .unwrap();
    g.add_node(NodeFn::new("c", |state: Value, _cfg| async move {
        let c = state["count"].as_i64().unwrap_or(0);
        let p = state["path"].as_str().unwrap_or("");
        Ok(json!({"count": c + 1, "path": format!("{p}->c")}))
    }))
    .unwrap();
    g.add_node(NodeFn::new("d", |state: Value, _cfg| async move {
        let c = state["count"].as_i64().unwrap_or(0);
        let p = state["path"].as_str().unwrap_or("");
        Ok(json!({"count": c + 1, "path": format!("{p}->d")}))
    }))
    .unwrap();

    g.set_entry_point("a");
    g.add_edge("a", "b");
    g.add_edge("b", "c");
    g.add_edge("c", "d");
    g.set_finish_point("d");
    g.compile().unwrap()
}

// ---------------------------------------------------------------------------
// E2E tests
// ---------------------------------------------------------------------------

/// 1. Full workflow: execute graph with checkpoints → get history → fork → replay.
#[tokio::test]
async fn full_time_travel_workflow() {
    let graph = build_linear_graph();
    let store = MemoryCheckpointStore::new();
    let config = RunnableConfig::default().with_thread_id("tt-workflow");

    // Execute the full graph
    let result = graph
        .invoke_resumable(json!({}), &config, &store)
        .await
        .unwrap();
    assert!(result.is_complete());
    assert_eq!(result.into_value()["count"], json!(3));

    // Get history — should be 3 checkpoints in ascending step order
    let history = get_state_history(&store, "tt-workflow").await.unwrap();
    assert_eq!(history.len(), 3);
    assert_eq!(history[0].step, 0);
    assert_eq!(history[1].step, 1);
    assert_eq!(history[2].step, 2);

    // Fork from middle checkpoint (after node "a")
    let after_a = &history[0];
    fork_from_checkpoint(&store, "tt-workflow", &after_a.id, "tt-fork")
        .await
        .unwrap();

    let fork_history = get_state_history(&store, "tt-fork").await.unwrap();
    assert_eq!(fork_history.len(), 1);
    assert_eq!(fork_history[0].channel_values["count"], json!(1));

    // Replay to step 1 on the original thread
    let replayed = replay_to_step(&store, "tt-workflow", 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(replayed.channel_values["count"], json!(2));
    assert_eq!(replayed.metadata.node_name.as_deref(), Some("b"));
}

/// 2. Execute with break_before, resume, verify full completion.
#[tokio::test]
async fn break_before_resume_completes() {
    let graph = build_linear_graph();
    let store = MemoryCheckpointStore::new();
    let config = RunnableConfig::default().with_thread_id("tt-break-before");

    let bp = BreakpointConfig::before(vec!["b".into()]);
    let result = graph
        .invoke_with_breakpoints(json!({}), &config, &store, &bp)
        .await
        .unwrap();

    // Should pause before "b"
    assert!(result.is_interrupted());
    let checkpoint_id = match &result {
        GraphOutput::Interrupted {
            checkpoint_id,
            state,
            ..
        } => {
            assert_eq!(state["count"], json!(1)); // only "a" ran
            checkpoint_id.clone()
        }
        _ => panic!("Expected Interrupted"),
    };

    // Resume with no breakpoints
    let resume_config = RunnableConfig::default()
        .with_thread_id("tt-break-before")
        .with_checkpoint_id(&checkpoint_id);
    let bp_empty = BreakpointConfig::new();

    let result = graph
        .invoke_with_breakpoints(json!({}), &resume_config, &store, &bp_empty)
        .await
        .unwrap();

    assert!(result.is_complete());
    assert_eq!(result.into_value()["count"], json!(3));
}

/// 3. Execute with break_after, inspect intermediate state, resume.
#[tokio::test]
async fn break_after_inspect_and_resume() {
    let graph = build_linear_graph();
    let store = MemoryCheckpointStore::new();
    let config = RunnableConfig::default().with_thread_id("tt-break-after");

    let bp = BreakpointConfig::after(vec!["b".into()]);
    let result = graph
        .invoke_with_breakpoints(json!({}), &config, &store, &bp)
        .await
        .unwrap();

    assert!(result.is_interrupted());
    let checkpoint_id = match &result {
        GraphOutput::Interrupted {
            checkpoint_id,
            state,
            interrupt_value,
        } => {
            // "b" has executed: a→1, b→2
            assert_eq!(state["count"], json!(2));
            assert_eq!(interrupt_value["breakpoint"], json!("after"));
            assert_eq!(interrupt_value["node"], json!("b"));
            checkpoint_id.clone()
        }
        _ => panic!("Expected Interrupted"),
    };

    // Inspect the checkpoint directly from the store
    let cp = store
        .get("tt-break-after", &checkpoint_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cp.channel_values["count"], json!(2));

    // Resume to completion
    let resume_config = RunnableConfig::default()
        .with_thread_id("tt-break-after")
        .with_checkpoint_id(&checkpoint_id);
    let bp_empty = BreakpointConfig::new();

    let result = graph
        .invoke_with_breakpoints(json!({}), &resume_config, &store, &bp_empty)
        .await
        .unwrap();

    assert!(result.is_complete());
    assert_eq!(result.into_value()["count"], json!(3));
}

/// 4. Multiple breakpoints in sequence — stop, resume, stop again, resume to end.
#[tokio::test]
async fn multiple_breakpoints_sequence() {
    let graph = build_4node_graph();
    let store = MemoryCheckpointStore::new();
    let config = RunnableConfig::default().with_thread_id("tt-multi-bp");

    // First run with breakpoints before "b" and before "d"
    let bp_both = BreakpointConfig::before(vec!["b".into(), "d".into()]);

    let result = graph
        .invoke_with_breakpoints(json!({}), &config, &store, &bp_both)
        .await
        .unwrap();

    // Should stop before "b"
    assert!(result.is_interrupted());
    let cp_id_1 = match &result {
        GraphOutput::Interrupted {
            checkpoint_id,
            interrupt_value,
            state,
        } => {
            assert_eq!(interrupt_value["node"], json!("b"));
            assert_eq!(state["count"], json!(1)); // only "a" ran
            checkpoint_id.clone()
        }
        _ => panic!("Expected Interrupted"),
    };

    // Resume with only break_before "d" — should run b, c, then stop before "d"
    let resume_config_1 = RunnableConfig::default()
        .with_thread_id("tt-multi-bp")
        .with_checkpoint_id(&cp_id_1);
    let bp_d_only = BreakpointConfig::before(vec!["d".into()]);

    let result = graph
        .invoke_with_breakpoints(json!({}), &resume_config_1, &store, &bp_d_only)
        .await
        .unwrap();

    assert!(result.is_interrupted());
    let cp_id_2 = match &result {
        GraphOutput::Interrupted {
            checkpoint_id,
            interrupt_value,
            state,
        } => {
            assert_eq!(interrupt_value["node"], json!("d"));
            assert_eq!(state["count"], json!(3)); // a, b, c ran
            checkpoint_id.clone()
        }
        _ => panic!("Expected Interrupted"),
    };

    // Resume with no breakpoints — should complete
    let resume_config_2 = RunnableConfig::default()
        .with_thread_id("tt-multi-bp")
        .with_checkpoint_id(&cp_id_2);
    let bp_empty = BreakpointConfig::new();

    let result = graph
        .invoke_with_breakpoints(json!({}), &resume_config_2, &store, &bp_empty)
        .await
        .unwrap();

    assert!(result.is_complete());
    let final_state = result.into_value();
    assert_eq!(final_state["count"], json!(4));
    assert_eq!(final_state["path"], json!("a->b->c->d"));
}

/// 5. Fork from middle checkpoint, execute different path on fork.
#[tokio::test]
async fn fork_from_middle_and_execute() {
    let graph = build_linear_graph();
    let store = MemoryCheckpointStore::new();
    let config = RunnableConfig::default().with_thread_id("tt-fork-exec");

    // Run to completion on original thread
    let result = graph
        .invoke_resumable(json!({}), &config, &store)
        .await
        .unwrap();
    assert!(result.is_complete());
    assert_eq!(result.into_value()["count"], json!(3));

    // Get checkpoint after node "a" (step 0)
    let history = get_state_history(&store, "tt-fork-exec").await.unwrap();
    let after_a = &history[0];
    assert_eq!(after_a.channel_values["count"], json!(1));

    // Fork from after "a" to a new thread
    fork_from_checkpoint(&store, "tt-fork-exec", &after_a.id, "tt-forked")
        .await
        .unwrap();

    // Resume execution on the forked thread
    let fork_checkpoints = store.list("tt-forked").await.unwrap();
    let fork_cp = &fork_checkpoints[0];

    let fork_config = RunnableConfig::default()
        .with_thread_id("tt-forked")
        .with_checkpoint_id(&fork_cp.id);

    let fork_result = graph
        .invoke_resumable(json!({}), &fork_config, &store)
        .await
        .unwrap();
    assert!(fork_result.is_complete());
    // count=1 (from fork) → b makes 2 → c makes 3
    assert_eq!(fork_result.into_value()["count"], json!(3));

    // Original thread's checkpoints remain unchanged
    let original_history = get_state_history(&store, "tt-fork-exec").await.unwrap();
    assert_eq!(original_history.len(), 3);

    // Forked thread now has 1 (fork) + 2 (b, c) = 3 checkpoints
    let fork_history = get_state_history(&store, "tt-forked").await.unwrap();
    assert_eq!(fork_history.len(), 3);
}

/// 6. Time-travel: replay to step 0 vs step 2, verify different states.
#[tokio::test]
async fn time_travel_replay_different_steps() {
    let graph = build_linear_graph();
    let store = MemoryCheckpointStore::new();
    let config = RunnableConfig::default().with_thread_id("tt-replay");

    graph
        .invoke_resumable(json!({}), &config, &store)
        .await
        .unwrap();

    // Replay to step 0 (after node "a")
    let cp0 = replay_to_step(&store, "tt-replay", 0)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cp0.channel_values["count"], json!(1));
    assert_eq!(cp0.metadata.node_name.as_deref(), Some("a"));

    // Replay to step 2 (after node "c")
    let cp2 = replay_to_step(&store, "tt-replay", 2)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cp2.channel_values["count"], json!(3));
    assert_eq!(cp2.metadata.node_name.as_deref(), Some("c"));

    // Different states at different steps
    assert_ne!(cp0.channel_values["count"], cp2.channel_values["count"]);

    // Non-existent step returns None
    let cp99 = replay_to_step(&store, "tt-replay", 99).await.unwrap();
    assert!(cp99.is_none());
}

/// 7. Thread isolation: checkpoints from thread A not visible in thread B.
#[tokio::test]
async fn thread_isolation() {
    let graph = build_linear_graph();
    let store = MemoryCheckpointStore::new();

    // Run on thread A
    let config_a = RunnableConfig::default().with_thread_id("thread-A");
    graph
        .invoke_resumable(json!({}), &config_a, &store)
        .await
        .unwrap();

    // Run on thread B
    let config_b = RunnableConfig::default().with_thread_id("thread-B");
    graph
        .invoke_resumable(json!({}), &config_b, &store)
        .await
        .unwrap();

    // Each thread has its own checkpoints
    let history_a = get_state_history(&store, "thread-A").await.unwrap();
    let history_b = get_state_history(&store, "thread-B").await.unwrap();

    assert_eq!(history_a.len(), 3);
    assert_eq!(history_b.len(), 3);

    // All checkpoints belong to their respective threads
    assert!(history_a.iter().all(|cp| cp.thread_id == "thread-A"));
    assert!(history_b.iter().all(|cp| cp.thread_id == "thread-B"));

    // No overlapping checkpoint IDs
    let ids_a: Vec<_> = history_a.iter().map(|cp| &cp.id).collect();
    let ids_b: Vec<_> = history_b.iter().map(|cp| &cp.id).collect();
    for id in &ids_a {
        assert!(!ids_b.contains(id));
    }

    // replay_to_step returns thread-specific checkpoints
    let step0_a = replay_to_step(&store, "thread-A", 0)
        .await
        .unwrap()
        .unwrap();
    let step0_b = replay_to_step(&store, "thread-B", 0)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(step0_a.id, step0_b.id);

    // Non-existent thread returns empty history
    let history_c = get_state_history(&store, "thread-C").await.unwrap();
    assert!(history_c.is_empty());
}

/// 8. Breakpoints on non-existent node names are ignored gracefully.
#[tokio::test]
async fn nonexistent_breakpoint_nodes_ignored() {
    let graph = build_linear_graph();
    let store = MemoryCheckpointStore::new();
    let config = RunnableConfig::default().with_thread_id("tt-nonexist-bp");

    // Set breakpoints on nodes that don't exist in the graph
    let bp = BreakpointConfig {
        break_before: vec!["nonexistent_node".into(), "fake_node".into()],
        break_after: vec!["another_fake".into()],
        condition: None,
    };

    let result = graph
        .invoke_with_breakpoints(json!({}), &config, &store, &bp)
        .await
        .unwrap();

    // Graph should complete normally
    assert!(result.is_complete());
    assert_eq!(result.into_value()["count"], json!(3));
}
