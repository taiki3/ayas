//! E2E tests: Human-in-the-Loop (HITL) interrupt and resume.
//!
//! Tests the full interrupt → human response → resume cycle,
//! including multiple sequential interrupts in a single graph.

use ayas_checkpoint::prelude::*;
use ayas_core::config::RunnableConfig;
use ayas_graph::prelude::*;
use serde_json::{json, Value};

/// Build graph: prepare → approval_gate (interrupts) → finalize
fn build_approval_graph() -> CompiledStateGraph {
    let mut g = StateGraph::new();
    g.add_last_value_channel("summary", json!(""));
    g.add_last_value_channel("approved", json!(false));
    g.add_last_value_channel("resume_value", json!(null));

    g.add_node(NodeFn::new(
        "prepare",
        |_state: Value, _cfg| async move {
            Ok(json!({"summary": "Generated summary of document XYZ"}))
        },
    ))
    .unwrap();

    g.add_node(NodeFn::new(
        "approval_gate",
        |state: Value, _cfg| async move {
            let summary = state["summary"].as_str().unwrap_or("");
            Ok(interrupt_output(json!({
                "question": "Do you approve this summary?",
                "summary": summary
            })))
        },
    ))
    .unwrap();

    g.add_node(NodeFn::new(
        "finalize",
        |state: Value, _cfg| async move {
            let resume = &state["resume_value"];
            let approved = resume.as_str() == Some("yes")
                || resume.as_bool().unwrap_or(false);
            Ok(json!({"approved": approved}))
        },
    ))
    .unwrap();

    g.set_entry_point("prepare");
    g.add_edge("prepare", "approval_gate");
    g.add_edge("approval_gate", "finalize");
    g.set_finish_point("finalize");
    g.compile().unwrap()
}

#[tokio::test]
async fn interrupt_returns_interrupted_output() {
    let graph = build_approval_graph();
    let store = MemoryCheckpointStore::new();
    let config = RunnableConfig::default().with_thread_id("hitl-interrupt");

    let result = graph
        .invoke_resumable(json!({}), &config, &store)
        .await
        .unwrap();

    assert!(result.is_interrupted());
    assert!(!result.is_complete());
}

#[tokio::test]
async fn interrupt_provides_checkpoint_id() {
    let graph = build_approval_graph();
    let store = MemoryCheckpointStore::new();
    let config = RunnableConfig::default().with_thread_id("hitl-cpid");

    let result = graph
        .invoke_resumable(json!({}), &config, &store)
        .await
        .unwrap();

    match &result {
        GraphOutput::Interrupted { checkpoint_id, .. } => {
            assert!(!checkpoint_id.is_empty());
            // The checkpoint should exist in the store
            let cp = store.get("hitl-cpid", checkpoint_id).await.unwrap();
            assert!(cp.is_some());
        }
        _ => panic!("Expected Interrupted output"),
    }
}

#[tokio::test]
async fn interrupt_value_contains_question() {
    let graph = build_approval_graph();
    let store = MemoryCheckpointStore::new();
    let config = RunnableConfig::default().with_thread_id("hitl-value");

    let result = graph
        .invoke_resumable(json!({}), &config, &store)
        .await
        .unwrap();

    match &result {
        GraphOutput::Interrupted {
            interrupt_value, ..
        } => {
            assert_eq!(
                interrupt_value["question"],
                json!("Do you approve this summary?")
            );
            assert_eq!(
                interrupt_value["summary"],
                json!("Generated summary of document XYZ")
            );
        }
        _ => panic!("Expected Interrupted output"),
    }
}

#[tokio::test]
async fn interrupt_checkpoint_metadata_is_interrupt() {
    let graph = build_approval_graph();
    let store = MemoryCheckpointStore::new();
    let config = RunnableConfig::default().with_thread_id("hitl-meta");

    let result = graph
        .invoke_resumable(json!({}), &config, &store)
        .await
        .unwrap();

    match &result {
        GraphOutput::Interrupted { checkpoint_id, .. } => {
            let cp = store.get("hitl-meta", checkpoint_id).await.unwrap().unwrap();
            assert_eq!(cp.metadata.source, "interrupt");
            assert_eq!(cp.metadata.node_name.as_deref(), Some("approval_gate"));
        }
        _ => panic!("Expected Interrupted"),
    }
}

#[tokio::test]
async fn resume_with_approval_completes_graph() {
    let graph = build_approval_graph();
    let store = MemoryCheckpointStore::new();
    let config = RunnableConfig::default().with_thread_id("hitl-approve");

    // First run: should interrupt at approval_gate
    let result = graph
        .invoke_resumable(json!({}), &config, &store)
        .await
        .unwrap();

    let checkpoint_id = match &result {
        GraphOutput::Interrupted { checkpoint_id, .. } => checkpoint_id.clone(),
        _ => panic!("Expected Interrupted"),
    };

    // Resume with approval
    let resume_config = RunnableConfig::default()
        .with_thread_id("hitl-approve")
        .with_checkpoint_id(&checkpoint_id)
        .with_resume_value(json!("yes"));

    let result = graph
        .invoke_resumable(json!({}), &resume_config, &store)
        .await
        .unwrap();

    assert!(result.is_complete());
    let final_state = result.into_value();
    assert_eq!(final_state["approved"], json!(true));
    assert_eq!(final_state["resume_value"], json!("yes"));
}

#[tokio::test]
async fn resume_with_rejection_completes_graph() {
    let graph = build_approval_graph();
    let store = MemoryCheckpointStore::new();
    let config = RunnableConfig::default().with_thread_id("hitl-reject");

    let result = graph
        .invoke_resumable(json!({}), &config, &store)
        .await
        .unwrap();

    let checkpoint_id = match &result {
        GraphOutput::Interrupted { checkpoint_id, .. } => checkpoint_id.clone(),
        _ => panic!("Expected Interrupted"),
    };

    // Resume with rejection
    let resume_config = RunnableConfig::default()
        .with_thread_id("hitl-reject")
        .with_checkpoint_id(&checkpoint_id)
        .with_resume_value(json!("no"));

    let result = graph
        .invoke_resumable(json!({}), &resume_config, &store)
        .await
        .unwrap();

    assert!(result.is_complete());
    let final_state = result.into_value();
    assert_eq!(final_state["approved"], json!(false));
}

/// Build graph with two interrupt points:
///   step1 → gate1 (interrupt) → step2 → gate2 (interrupt) → step3
fn build_multi_interrupt_graph() -> CompiledStateGraph {
    let mut g = StateGraph::new();
    g.add_last_value_channel("count", json!(0));
    g.add_last_value_channel("resume_value", json!(null));
    g.add_append_channel("decisions");

    g.add_node(NodeFn::new("step1", |_state: Value, _cfg| async move {
        Ok(json!({"count": 1}))
    }))
    .unwrap();

    g.add_node(NodeFn::new("gate1", |state: Value, _cfg| async move {
        let c = state["count"].as_i64().unwrap_or(0);
        Ok(json!({
            "count": c + 1,
            "__interrupt__": {"value": {"question": "First approval needed"}}
        }))
    }))
    .unwrap();

    g.add_node(NodeFn::new("step2", |state: Value, _cfg| async move {
        let c = state["count"].as_i64().unwrap_or(0);
        let decision = state["resume_value"].as_str().unwrap_or("unknown");
        Ok(json!({
            "count": c + 1,
            "decisions": format!("gate1:{decision}")
        }))
    }))
    .unwrap();

    g.add_node(NodeFn::new("gate2", |state: Value, _cfg| async move {
        let c = state["count"].as_i64().unwrap_or(0);
        Ok(json!({
            "count": c + 1,
            "__interrupt__": {"value": {"question": "Second approval needed"}}
        }))
    }))
    .unwrap();

    g.add_node(NodeFn::new("step3", |state: Value, _cfg| async move {
        let c = state["count"].as_i64().unwrap_or(0);
        let decision = state["resume_value"].as_str().unwrap_or("unknown");
        Ok(json!({
            "count": c + 1,
            "decisions": format!("gate2:{decision}")
        }))
    }))
    .unwrap();

    g.set_entry_point("step1");
    g.add_edge("step1", "gate1");
    g.add_edge("gate1", "step2");
    g.add_edge("step2", "gate2");
    g.add_edge("gate2", "step3");
    g.set_finish_point("step3");
    g.compile().unwrap()
}

#[tokio::test]
async fn multiple_sequential_interrupts() {
    let graph = build_multi_interrupt_graph();
    let store = MemoryCheckpointStore::new();
    let config = RunnableConfig::default().with_thread_id("hitl-multi");

    // First run: should interrupt at gate1
    let result = graph
        .invoke_resumable(json!({}), &config, &store)
        .await
        .unwrap();

    assert!(result.is_interrupted());
    let (cp_id_1, interrupt_val_1) = match &result {
        GraphOutput::Interrupted {
            checkpoint_id,
            interrupt_value,
            ..
        } => (checkpoint_id.clone(), interrupt_value.clone()),
        _ => panic!("Expected Interrupted"),
    };
    assert_eq!(interrupt_val_1["question"], json!("First approval needed"));

    // Resume from gate1 with "approved-1"
    let resume_config_1 = RunnableConfig::default()
        .with_thread_id("hitl-multi")
        .with_checkpoint_id(&cp_id_1)
        .with_resume_value(json!("approved-1"));

    let result = graph
        .invoke_resumable(json!({}), &resume_config_1, &store)
        .await
        .unwrap();

    // Should interrupt again at gate2
    assert!(result.is_interrupted());
    let (cp_id_2, interrupt_val_2) = match &result {
        GraphOutput::Interrupted {
            checkpoint_id,
            interrupt_value,
            ..
        } => (checkpoint_id.clone(), interrupt_value.clone()),
        _ => panic!("Expected Interrupted"),
    };
    assert_eq!(
        interrupt_val_2["question"],
        json!("Second approval needed")
    );
    assert_ne!(cp_id_1, cp_id_2);

    // Resume from gate2 with "approved-2"
    let resume_config_2 = RunnableConfig::default()
        .with_thread_id("hitl-multi")
        .with_checkpoint_id(&cp_id_2)
        .with_resume_value(json!("approved-2"));

    let result = graph
        .invoke_resumable(json!({}), &resume_config_2, &store)
        .await
        .unwrap();

    // Now it should complete
    assert!(result.is_complete());
    let final_state = result.into_value();

    // count: step1(1) + gate1(2) + step2(3) + gate2(4) + step3(5)
    assert_eq!(final_state["count"], json!(5));

    // decisions accumulated
    let decisions = final_state["decisions"].as_array().unwrap();
    assert_eq!(decisions.len(), 2);
    assert_eq!(decisions[0], json!("gate1:approved-1"));
    assert_eq!(decisions[1], json!("gate2:approved-2"));
}

#[tokio::test]
async fn interrupt_state_reflects_partial_execution() {
    let graph = build_approval_graph();
    let store = MemoryCheckpointStore::new();
    let config = RunnableConfig::default().with_thread_id("hitl-state");

    let result = graph
        .invoke_resumable(json!({}), &config, &store)
        .await
        .unwrap();

    match &result {
        GraphOutput::Interrupted { state, .. } => {
            // After prepare and approval_gate partial execution, summary should be set
            assert_eq!(state["summary"], json!("Generated summary of document XYZ"));
        }
        _ => panic!("Expected Interrupted"),
    }
}

#[tokio::test]
async fn interrupt_with_complex_value() {
    let mut g = StateGraph::new();
    g.add_last_value_channel("data", json!(null));

    g.add_node(NodeFn::new(
        "complex_interrupt",
        |_state: Value, _cfg| async move {
            Ok(interrupt_output(json!({
                "form": {
                    "fields": [
                        {"name": "title", "type": "text", "required": true},
                        {"name": "priority", "type": "select", "options": ["low", "medium", "high"]}
                    ],
                    "defaults": {"title": "Untitled", "priority": "medium"}
                }
            })))
        },
    ))
    .unwrap();

    g.set_entry_point("complex_interrupt");
    g.set_finish_point("complex_interrupt");
    let graph = g.compile().unwrap();

    let store = MemoryCheckpointStore::new();
    let config = RunnableConfig::default().with_thread_id("hitl-complex");

    let result = graph
        .invoke_resumable(json!({}), &config, &store)
        .await
        .unwrap();

    match &result {
        GraphOutput::Interrupted {
            interrupt_value, ..
        } => {
            assert!(interrupt_value["form"]["fields"].is_array());
            assert_eq!(
                interrupt_value["form"]["defaults"]["priority"],
                json!("medium")
            );
        }
        _ => panic!("Expected Interrupted"),
    }
}
