//! Property-based tests for graph time-travel and breakpoint invariants.

use proptest::prelude::*;
use serde_json::{json, Value};
use std::collections::HashMap;

use ayas_checkpoint::prelude::*;
use ayas_core::config::RunnableConfig;
use ayas_graph::prelude::*;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn node_names() -> Vec<&'static str> {
    vec!["a", "b", "c"]
}

/// Build a 3-node linear graph: a → b → c
/// Node a sets count=1, b increments, c increments.
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

fn make_checkpoint(id: &str, thread_id: &str, step: usize, value: Value) -> Checkpoint {
    Checkpoint {
        id: id.into(),
        thread_id: thread_id.into(),
        parent_id: if step > 0 {
            Some(format!("cp-{}", step - 1))
        } else {
            None
        },
        step,
        channel_values: HashMap::from([("count".into(), value)]),
        pending_nodes: vec![format!("node_{}", step + 1)],
        metadata: CheckpointMetadata {
            source: "test".into(),
            step,
            node_name: Some(format!("node_{step}")),
        },
        created_at: chrono::Utc::now(),
    }
}

// ---------------------------------------------------------------------------
// Property-based tests
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// fork_from_checkpoint creates an independent copy — modifying the fork
    /// does not affect the original thread's checkpoints.
    #[test]
    fn fork_creates_independent_copy(
        num_checkpoints in 1usize..5,
        value in -1000i64..1000,
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let store = MemoryCheckpointStore::new();

            // Put checkpoints on the original thread
            for i in 0..num_checkpoints {
                let cp = make_checkpoint(
                    &format!("cp-{i}"),
                    "original",
                    i,
                    json!(value * (i as i64 + 1)),
                );
                store.put(cp).await.unwrap();
            }

            // Fork from the last checkpoint
            let last_cp_id = format!("cp-{}", num_checkpoints - 1);
            fork_from_checkpoint(&store, "original", &last_cp_id, "forked")
                .await
                .unwrap();

            // Forked thread has exactly one checkpoint
            let forked = store.list("forked").await.unwrap();
            prop_assert_eq!(forked.len(), 1);

            // Original thread is unchanged
            let original = store.list("original").await.unwrap();
            prop_assert_eq!(original.len(), num_checkpoints);

            // Forked checkpoint has the same channel values as the source
            let source_cp = store.get("original", &last_cp_id).await.unwrap().unwrap();
            prop_assert_eq!(&forked[0].channel_values, &source_cp.channel_values);

            // Forked checkpoint is on the new thread
            prop_assert_eq!(&forked[0].thread_id, "forked");

            // Adding checkpoints to the forked thread doesn't affect original
            let extra = Checkpoint {
                id: "extra-cp".into(),
                thread_id: "forked".into(),
                parent_id: Some(forked[0].id.clone()),
                step: 1,
                channel_values: HashMap::from([("count".into(), json!(999))]),
                pending_nodes: vec![],
                metadata: CheckpointMetadata {
                    source: "test".into(),
                    step: 1,
                    node_name: Some("extra".into()),
                },
                created_at: chrono::Utc::now(),
            };
            store.put(extra).await.unwrap();

            let original_after = store.list("original").await.unwrap();
            prop_assert_eq!(original_after.len(), num_checkpoints);
            for (before, after) in original.iter().zip(original_after.iter()) {
                prop_assert_eq!(&before.channel_values, &after.channel_values);
            }

            Ok(())
        })?;
    }

    /// replay_to_step(step=N) returns the checkpoint with step == N when it
    /// exists, or None when it does not.
    #[test]
    fn replay_returns_correct_step(
        num_steps in 1usize..6,
        query_step in 0usize..8,
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let store = MemoryCheckpointStore::new();

            for i in 0..num_steps {
                let cp = make_checkpoint(&format!("cp-{i}"), "t1", i, json!(i * 10));
                store.put(cp).await.unwrap();
            }

            let result = replay_to_step(&store, "t1", query_step).await.unwrap();

            if query_step < num_steps {
                let cp = result.unwrap();
                prop_assert_eq!(cp.step, query_step);
            } else {
                prop_assert!(result.is_none());
            }

            Ok(())
        })?;
    }

    /// get_state_history always returns checkpoints in ascending step order,
    /// regardless of insertion order.
    #[test]
    fn history_ascending_step_order(num_steps in 1usize..8) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let store = MemoryCheckpointStore::new();

            // Insert in reverse order to test sorting
            for i in (0..num_steps).rev() {
                let cp = make_checkpoint(&format!("cp-{i}"), "t1", i, json!(i));
                store.put(cp).await.unwrap();
            }

            let history = get_state_history(&store, "t1").await.unwrap();
            prop_assert_eq!(history.len(), num_steps);

            for i in 1..history.len() {
                prop_assert!(
                    history[i].step >= history[i - 1].step,
                    "History not in ascending order: step {} followed by step {}",
                    history[i - 1].step,
                    history[i].step,
                );
            }

            Ok(())
        })?;
    }

    /// A breakpoint *before* node X always stops execution before X runs.
    /// For the a→b→c graph with count channel:
    ///   before a → count=0, before b → count=1, before c → count=2.
    #[test]
    fn break_before_stops_before_execution(node_idx in 0usize..3) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let graph = build_linear_graph();
            let store = MemoryCheckpointStore::new();
            let names = node_names();
            let target = names[node_idx];
            let config = RunnableConfig::default()
                .with_thread_id(&format!("bp-before-{node_idx}"));

            let bp = BreakpointConfig::before(vec![target.into()]);

            let result = graph
                .invoke_with_breakpoints(json!({}), &config, &store, &bp)
                .await
                .unwrap();

            prop_assert!(result.is_interrupted(), "Expected interruption before '{}'", target);

            match &result {
                GraphOutput::Interrupted { interrupt_value, state, .. } => {
                    prop_assert_eq!(&interrupt_value["breakpoint"], &json!("before"));
                    prop_assert_eq!(&interrupt_value["node"], &json!(target));
                    // count equals the number of nodes that executed before the target
                    let expected_count = node_idx as i64;
                    prop_assert_eq!(
                        &state["count"],
                        &json!(expected_count),
                        "Before '{}', count should be {}",
                        target,
                        expected_count,
                    );
                }
                _ => panic!("Expected Interrupted"),
            }

            Ok(())
        })?;
    }

    /// A breakpoint *after* node X always includes X's output in the state.
    /// For the a→b→c graph:
    ///   after a → count=1, after b → count=2, after c → count=3.
    #[test]
    fn break_after_includes_node_output(node_idx in 0usize..3) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let graph = build_linear_graph();
            let store = MemoryCheckpointStore::new();
            let names = node_names();
            let target = names[node_idx];
            let config = RunnableConfig::default()
                .with_thread_id(&format!("bp-after-{node_idx}"));

            let bp = BreakpointConfig::after(vec![target.into()]);

            let result = graph
                .invoke_with_breakpoints(json!({}), &config, &store, &bp)
                .await
                .unwrap();

            prop_assert!(result.is_interrupted(), "Expected interruption after '{}'", target);

            match &result {
                GraphOutput::Interrupted { interrupt_value, state, .. } => {
                    prop_assert_eq!(&interrupt_value["breakpoint"], &json!("after"));
                    prop_assert_eq!(&interrupt_value["node"], &json!(target));
                    // count = node_idx + 1 (the target node has executed)
                    let expected_count = (node_idx + 1) as i64;
                    prop_assert_eq!(
                        &state["count"],
                        &json!(expected_count),
                        "After '{}', count should be {}",
                        target,
                        expected_count,
                    );
                }
                _ => panic!("Expected Interrupted"),
            }

            Ok(())
        })?;
    }

    /// An empty BreakpointConfig never interrupts — the graph always completes.
    #[test]
    fn empty_breakpoints_complete_normally(thread_suffix in 0u32..1000) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let graph = build_linear_graph();
            let store = MemoryCheckpointStore::new();
            let config = RunnableConfig::default()
                .with_thread_id(&format!("bp-empty-{thread_suffix}"));

            let bp = BreakpointConfig::new();

            let result = graph
                .invoke_with_breakpoints(json!({}), &config, &store, &bp)
                .await
                .unwrap();

            prop_assert!(result.is_complete(), "Empty breakpoint config should complete normally");
            prop_assert_eq!(&result.into_value()["count"], &json!(3));

            Ok(())
        })?;
    }
}
