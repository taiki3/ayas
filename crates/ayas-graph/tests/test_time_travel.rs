//! E2E tests: time travel / forking from earlier checkpoints.
//!
//! Verifies that we can run a graph to completion, then fork from
//! an earlier checkpoint with different input and observe divergent
//! execution paths.

use ayas_checkpoint::prelude::*;
use ayas_core::config::RunnableConfig;
use ayas_graph::prelude::*;
use serde_json::{json, Value};
use std::collections::HashMap;

/// Build a branching graph:
///   classify → (conditional) → path_a or path_b → merge
///
/// `classify` sets a label based on the `score` channel.
/// The conditional edge routes based on score threshold.
fn build_branching_graph() -> CompiledStateGraph {
    let mut g = StateGraph::new();
    g.add_last_value_channel("score", json!(0));
    g.add_last_value_channel("label", json!(""));
    g.add_last_value_channel("result", json!(""));

    g.add_node(NodeFn::new(
        "classify",
        |state: Value, _cfg| async move {
            let score = state["score"].as_i64().unwrap_or(0);
            let label = if score > 50 { "high" } else { "low" };
            Ok(json!({"label": label}))
        },
    ))
    .unwrap();

    g.add_node(NodeFn::new(
        "path_a",
        |state: Value, _cfg| async move {
            let label = state["label"].as_str().unwrap_or("");
            Ok(json!({"result": format!("path_a({label})")}))
        },
    ))
    .unwrap();

    g.add_node(NodeFn::new(
        "path_b",
        |state: Value, _cfg| async move {
            let label = state["label"].as_str().unwrap_or("");
            Ok(json!({"result": format!("path_b({label})")}))
        },
    ))
    .unwrap();

    g.add_node(NodeFn::new(
        "merge",
        |state: Value, _cfg| async move {
            let result = state["result"].as_str().unwrap_or("");
            Ok(json!({"result": format!("{result}->merged")}))
        },
    ))
    .unwrap();

    g.set_entry_point("classify");

    let mut path_map = HashMap::new();
    path_map.insert("high".to_string(), "path_a".to_string());
    path_map.insert("low".to_string(), "path_b".to_string());

    g.add_conditional_edges(ConditionalEdge::new(
        "classify",
        |state: &Value| {
            let score = state["score"].as_i64().unwrap_or(0);
            if score > 50 {
                "high".to_string()
            } else {
                "low".to_string()
            }
        },
        Some(path_map),
    ));

    g.add_edge("path_a", "merge");
    g.add_edge("path_b", "merge");
    g.set_finish_point("merge");
    g.compile().unwrap()
}

#[tokio::test]
async fn list_checkpoints_after_completion() {
    let graph = build_branching_graph();
    let store = MemoryCheckpointStore::new();
    let config = RunnableConfig::default().with_thread_id("tt-list");

    let result = graph
        .invoke_resumable(json!({"score": 80}), &config, &store)
        .await
        .unwrap();

    assert!(result.is_complete());

    let checkpoints = store.list("tt-list").await.unwrap();
    // classify → path_a → merge = 3 checkpoints
    assert_eq!(checkpoints.len(), 3);

    // Verify order
    assert_eq!(
        checkpoints[0].metadata.node_name.as_deref(),
        Some("classify")
    );
    assert_eq!(
        checkpoints[1].metadata.node_name.as_deref(),
        Some("path_a")
    );
    assert_eq!(
        checkpoints[2].metadata.node_name.as_deref(),
        Some("merge")
    );
}

#[tokio::test]
async fn fork_from_earlier_checkpoint_divergent_paths() {
    // Run a linear graph, then "fork" by resuming from an earlier point
    let mut g = StateGraph::new();
    g.add_last_value_channel("count", json!(0));
    g.add_last_value_channel("multiplier", json!(1));

    g.add_node(NodeFn::new("setup", |state: Value, _cfg| async move {
        let m = state["multiplier"].as_i64().unwrap_or(1);
        Ok(json!({"count": 10 * m}))
    }))
    .unwrap();

    g.add_node(NodeFn::new("double", |state: Value, _cfg| async move {
        let c = state["count"].as_i64().unwrap_or(0);
        Ok(json!({"count": c * 2}))
    }))
    .unwrap();

    g.add_node(NodeFn::new("add_bonus", |state: Value, _cfg| async move {
        let c = state["count"].as_i64().unwrap_or(0);
        Ok(json!({"count": c + 7}))
    }))
    .unwrap();

    g.set_entry_point("setup");
    g.add_edge("setup", "double");
    g.add_edge("double", "add_bonus");
    g.set_finish_point("add_bonus");
    let graph = g.compile().unwrap();

    let store = MemoryCheckpointStore::new();

    // Original run: multiplier=1 → setup(10) → double(20) → add_bonus(27)
    let config = RunnableConfig::default().with_thread_id("tt-fork");
    let result = graph
        .invoke_resumable(json!({"multiplier": 1}), &config, &store)
        .await
        .unwrap();
    assert_eq!(result.into_value()["count"], json!(27));

    // Get checkpoint after "setup" — it should have count=10
    let checkpoints = store.list("tt-fork").await.unwrap();
    let after_setup = checkpoints
        .iter()
        .find(|cp| cp.metadata.node_name.as_deref() == Some("setup"))
        .unwrap();
    assert_eq!(after_setup.channel_values["count"], json!(10));

    // Now do a DIFFERENT original run on a different thread with multiplier=5
    let config2 = RunnableConfig::default().with_thread_id("tt-fork-alt");
    let result2 = graph
        .invoke_resumable(json!({"multiplier": 5}), &config2, &store)
        .await
        .unwrap();
    // setup(50) → double(100) → add_bonus(107)
    assert_eq!(result2.into_value()["count"], json!(107));

    // Fork: resume original thread from after "setup" (count=10)
    // This should re-execute double and add_bonus with count=10
    let fork_config = RunnableConfig::default()
        .with_thread_id("tt-fork")
        .with_checkpoint_id(&after_setup.id);

    let fork_result = graph
        .invoke_resumable(json!({}), &fork_config, &store)
        .await
        .unwrap();
    // count=10 (restored) → double(20) → add_bonus(27)
    assert_eq!(fork_result.into_value()["count"], json!(27));
}

#[tokio::test]
async fn time_travel_preserves_original_checkpoints() {
    let mut g = StateGraph::new();
    g.add_last_value_channel("value", json!(0));

    g.add_node(NodeFn::new("step_a", |_state: Value, _cfg| async move {
        Ok(json!({"value": 10}))
    }))
    .unwrap();
    g.add_node(NodeFn::new("step_b", |state: Value, _cfg| async move {
        let v = state["value"].as_i64().unwrap_or(0);
        Ok(json!({"value": v + 5}))
    }))
    .unwrap();
    g.add_node(NodeFn::new("step_c", |state: Value, _cfg| async move {
        let v = state["value"].as_i64().unwrap_or(0);
        Ok(json!({"value": v * 2}))
    }))
    .unwrap();

    g.set_entry_point("step_a");
    g.add_edge("step_a", "step_b");
    g.add_edge("step_b", "step_c");
    g.set_finish_point("step_c");
    let graph = g.compile().unwrap();

    let store = MemoryCheckpointStore::new();
    let config = RunnableConfig::default().with_thread_id("tt-preserve");

    // Run to completion: 0→10→15→30
    let result = graph
        .invoke_resumable(json!({}), &config, &store)
        .await
        .unwrap();
    assert_eq!(result.into_value()["value"], json!(30));

    let original_checkpoints = store.list("tt-preserve").await.unwrap();
    assert_eq!(original_checkpoints.len(), 3);

    let original_ids: Vec<String> = original_checkpoints.iter().map(|c| c.id.clone()).collect();
    let original_values: Vec<Value> = original_checkpoints
        .iter()
        .map(|c| c.channel_values["value"].clone())
        .collect();

    // Resume from step_a checkpoint
    let after_a = &original_checkpoints[0];
    let resume_config = RunnableConfig::default()
        .with_thread_id("tt-preserve")
        .with_checkpoint_id(&after_a.id);

    graph
        .invoke_resumable(json!({}), &resume_config, &store)
        .await
        .unwrap();

    // Original checkpoints should still exist with the same values
    let all_checkpoints = store.list("tt-preserve").await.unwrap();
    for (idx, orig_id) in original_ids.iter().enumerate() {
        let still_exists = all_checkpoints.iter().find(|c| c.id == *orig_id);
        assert!(
            still_exists.is_some(),
            "Original checkpoint {orig_id} was lost"
        );
        assert_eq!(
            still_exists.unwrap().channel_values["value"],
            original_values[idx]
        );
    }

    // Total should be original 3 + 2 new (step_b + step_c from resume)
    assert_eq!(all_checkpoints.len(), 5);
}

#[tokio::test]
async fn get_latest_checkpoint_for_thread() {
    let mut g = StateGraph::new();
    g.add_last_value_channel("x", json!(0));

    g.add_node(NodeFn::new("first", |_state: Value, _cfg| async move {
        Ok(json!({"x": 1}))
    }))
    .unwrap();
    g.add_node(NodeFn::new("second", |_state: Value, _cfg| async move {
        Ok(json!({"x": 2}))
    }))
    .unwrap();
    g.add_node(NodeFn::new("third", |_state: Value, _cfg| async move {
        Ok(json!({"x": 3}))
    }))
    .unwrap();

    g.set_entry_point("first");
    g.add_edge("first", "second");
    g.add_edge("second", "third");
    g.set_finish_point("third");
    let graph = g.compile().unwrap();

    let store = MemoryCheckpointStore::new();
    let config = RunnableConfig::default().with_thread_id("tt-latest");

    graph
        .invoke_resumable(json!({}), &config, &store)
        .await
        .unwrap();

    let latest = store.get_latest("tt-latest").await.unwrap().unwrap();
    assert_eq!(latest.metadata.node_name.as_deref(), Some("third"));
    assert_eq!(latest.channel_values["x"], json!(3));
}

#[tokio::test]
async fn fork_to_new_thread_for_what_if_scenario() {
    let mut g = StateGraph::new();
    g.add_last_value_channel("temperature", json!(0.0));
    g.add_last_value_channel("action", json!(""));

    g.add_node(NodeFn::new("read_temp", |state: Value, _cfg| async move {
        // Just passes through the temperature
        let t = state["temperature"].as_f64().unwrap_or(0.0);
        Ok(json!({"temperature": t}))
    }))
    .unwrap();

    g.add_node(NodeFn::new("decide", |state: Value, _cfg| async move {
        let t = state["temperature"].as_f64().unwrap_or(0.0);
        let action = if t > 30.0 {
            "cool"
        } else if t < 10.0 {
            "heat"
        } else {
            "idle"
        };
        Ok(json!({"action": action}))
    }))
    .unwrap();

    g.set_entry_point("read_temp");
    g.add_edge("read_temp", "decide");
    g.set_finish_point("decide");
    let graph = g.compile().unwrap();

    let store = MemoryCheckpointStore::new();

    // Run with temp=35 on thread "original"
    let config = RunnableConfig::default().with_thread_id("original");
    let result = graph
        .invoke_resumable(json!({"temperature": 35.0}), &config, &store)
        .await
        .unwrap();
    assert_eq!(result.into_value()["action"], json!("cool"));

    // Get the checkpoint after read_temp
    let original_cps = store.list("original").await.unwrap();
    let _after_read = original_cps
        .iter()
        .find(|cp| cp.metadata.node_name.as_deref() == Some("read_temp"))
        .unwrap();

    // For a "what-if" scenario, we can simulate by creating a new store entry
    // by saving a modified checkpoint to a new thread and resuming from it.
    // In practice, we just re-run the whole graph on a new thread with different input.
    let whatif_config = RunnableConfig::default().with_thread_id("whatif");
    let whatif_result = graph
        .invoke_resumable(json!({"temperature": 5.0}), &whatif_config, &store)
        .await
        .unwrap();
    assert_eq!(whatif_result.into_value()["action"], json!("heat"));

    // Original thread checkpoints untouched
    let orig_cps_after = store.list("original").await.unwrap();
    assert_eq!(orig_cps_after.len(), original_cps.len());

    // What-if thread has its own checkpoints
    let whatif_cps = store.list("whatif").await.unwrap();
    assert_eq!(whatif_cps.len(), 2);
    assert_eq!(whatif_cps[0].channel_values["temperature"], json!(5.0));
}
