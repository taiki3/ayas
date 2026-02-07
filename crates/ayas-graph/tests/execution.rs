use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use ayas_core::config::RunnableConfig;
use ayas_core::error::AyasError;
use ayas_core::runnable::Runnable;
use ayas_graph::prelude::*;
use serde_json::{json, Value};

/// Linear execution: START -> a -> b -> c -> END
/// Each node sets a key in the partial output.
#[tokio::test]
async fn execute_linear_graph() {
    let mut graph = StateGraph::new();
    graph.add_last_value_channel("step", json!(null));

    graph
        .add_node(NodeFn::new("a", |_state: Value, _config| async move {
            Ok(json!({"step": "a"}))
        }))
        .unwrap();
    graph
        .add_node(NodeFn::new("b", |_state: Value, _config| async move {
            Ok(json!({"step": "b"}))
        }))
        .unwrap();
    graph
        .add_node(NodeFn::new("c", |_state: Value, _config| async move {
            Ok(json!({"step": "c"}))
        }))
        .unwrap();

    graph.set_entry_point("a");
    graph.add_edge("a", "b");
    graph.add_edge("b", "c");
    graph.set_finish_point("c");

    let compiled = graph.compile().unwrap();
    let config = RunnableConfig::default();
    let result = compiled.invoke(json!({}), &config).await.unwrap();

    assert_eq!(result["step"], json!("c"));
}

/// Nodes read from state and accumulate values.
#[tokio::test]
async fn execute_with_accumulation() {
    let mut graph = StateGraph::new();
    graph.add_last_value_channel("count", json!(0));

    graph
        .add_node(NodeFn::new("inc1", |state: Value, _config| async move {
            let count = state["count"].as_i64().unwrap_or(0);
            Ok(json!({"count": count + 1}))
        }))
        .unwrap();
    graph
        .add_node(NodeFn::new("inc2", |state: Value, _config| async move {
            let count = state["count"].as_i64().unwrap_or(0);
            Ok(json!({"count": count + 10}))
        }))
        .unwrap();

    graph.set_entry_point("inc1");
    graph.add_edge("inc1", "inc2");
    graph.set_finish_point("inc2");

    let compiled = graph.compile().unwrap();
    let config = RunnableConfig::default();
    let result = compiled.invoke(json!({"count": 5}), &config).await.unwrap();

    // 5 -> inc1 makes it 6 -> inc2 makes it 16
    assert_eq!(result["count"], json!(16));
}

/// Conditional routing: router decides path based on state.
#[tokio::test]
async fn execute_conditional_routing() {
    let mut graph = StateGraph::new();
    graph.add_last_value_channel("score", json!(0.0));
    graph.add_last_value_channel("result", json!(null));

    graph
        .add_node(NodeFn::new(
            "router",
            |_state: Value, _config| async move { Ok(json!({})) },
        ))
        .unwrap();
    graph
        .add_node(NodeFn::new(
            "approve",
            |_state: Value, _config| async move { Ok(json!({"result": "approved"})) },
        ))
        .unwrap();
    graph
        .add_node(NodeFn::new(
            "reject",
            |_state: Value, _config| async move { Ok(json!({"result": "rejected"})) },
        ))
        .unwrap();

    graph.set_entry_point("router");

    let mut path_map = HashMap::new();
    path_map.insert("yes".to_string(), "approve".to_string());
    path_map.insert("no".to_string(), "reject".to_string());

    graph.add_conditional_edges(ConditionalEdge::new(
        "router",
        |state: &Value| {
            if state["score"].as_f64().unwrap_or(0.0) > 0.5 {
                "yes".to_string()
            } else {
                "no".to_string()
            }
        },
        Some(path_map),
    ));

    graph.set_finish_point("approve");
    graph.set_finish_point("reject");

    let compiled = graph.compile().unwrap();
    let config = RunnableConfig::default();

    // High score -> approve
    let result = compiled
        .invoke(json!({"score": 0.8}), &config)
        .await
        .unwrap();
    assert_eq!(result["result"], json!("approved"));

    // Low score -> reject
    let result = compiled
        .invoke(json!({"score": 0.2}), &config)
        .await
        .unwrap();
    assert_eq!(result["result"], json!("rejected"));
}

/// Cyclic graph with recursion limit.
#[tokio::test]
async fn execute_cyclic_with_recursion_limit() {
    let mut graph = StateGraph::new();
    graph.add_last_value_channel("count", json!(0));

    graph
        .add_node(NodeFn::new("loop_node", |state: Value, _config| async move {
            let count = state["count"].as_i64().unwrap_or(0);
            Ok(json!({"count": count + 1}))
        }))
        .unwrap();

    graph.set_entry_point("loop_node");

    // Always loop back to self (never reaches END via conditional)
    let mut path_map = HashMap::new();
    path_map.insert("continue".to_string(), "loop_node".to_string());
    path_map.insert("done".to_string(), END.to_string());

    graph.add_conditional_edges(ConditionalEdge::new(
        "loop_node",
        |state: &Value| {
            if state["count"].as_i64().unwrap_or(0) >= 5 {
                "done".to_string()
            } else {
                "continue".to_string()
            }
        },
        Some(path_map),
    ));

    let compiled = graph.compile().unwrap();
    let config = RunnableConfig::default();
    let result = compiled.invoke(json!({}), &config).await.unwrap();

    // Loops 5 times: 0->1->2->3->4->5, then routes to END
    assert_eq!(result["count"], json!(5));
}

/// Recursion limit is enforced.
#[tokio::test]
async fn execute_recursion_limit_exceeded() {
    let mut graph = StateGraph::new();
    graph.add_last_value_channel("x", json!(0));

    graph
        .add_node(NodeFn::new(
            "infinite",
            |_state: Value, _config| async move { Ok(json!({})) },
        ))
        .unwrap();

    graph.set_entry_point("infinite");
    graph.add_edge("infinite", "infinite");

    let compiled = graph.compile().unwrap();
    let config = RunnableConfig::default().with_recursion_limit(3);
    let result = compiled.invoke(json!({}), &config).await;

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("Recursion limit"));
}

/// AppendChannel accumulates messages across steps.
#[tokio::test]
async fn execute_with_append_channel() {
    let mut graph = StateGraph::new();
    graph.add_append_channel("log");

    graph
        .add_node(NodeFn::new("step1", |_state: Value, _config| async move {
            Ok(json!({"log": "step1 done"}))
        }))
        .unwrap();
    graph
        .add_node(NodeFn::new("step2", |_state: Value, _config| async move {
            Ok(json!({"log": "step2 done"}))
        }))
        .unwrap();

    graph.set_entry_point("step1");
    graph.add_edge("step1", "step2");
    graph.set_finish_point("step2");

    let compiled = graph.compile().unwrap();
    let config = RunnableConfig::default();
    let result = compiled.invoke(json!({}), &config).await.unwrap();

    assert_eq!(result["log"], json!(["step1 done", "step2 done"]));
}

/// Node error is wrapped in GraphError::NodeExecution.
#[tokio::test]
async fn execute_node_error_propagation() {
    let mut graph = StateGraph::new();
    graph.add_last_value_channel("x", json!(0));

    graph
        .add_node(NodeFn::new(
            "fail_node",
            |_state: Value, _config| async move {
                Err(AyasError::Other("something broke".into()))
            },
        ))
        .unwrap();

    graph.set_entry_point("fail_node");
    graph.set_finish_point("fail_node");

    let compiled = graph.compile().unwrap();
    let config = RunnableConfig::default();
    let result = compiled.invoke(json!({}), &config).await;

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("fail_node"));
}

/// Multiple invocations on the same compiled graph are independent.
#[tokio::test]
async fn execute_independent_invocations() {
    let counter = Arc::new(AtomicUsize::new(0));

    let mut graph = StateGraph::new();
    graph.add_last_value_channel("call_id", json!(0));

    let counter_clone = counter.clone();
    graph
        .add_node(NodeFn::new("counter", move |_state: Value, _config| {
            let c = counter_clone.clone();
            async move {
                let id = c.fetch_add(1, Ordering::Relaxed);
                Ok(json!({"call_id": id}))
            }
        }))
        .unwrap();

    graph.set_entry_point("counter");
    graph.set_finish_point("counter");

    let compiled = graph.compile().unwrap();
    let config = RunnableConfig::default();

    let r1 = compiled.invoke(json!({}), &config).await.unwrap();
    let r2 = compiled.invoke(json!({}), &config).await.unwrap();

    assert_ne!(r1["call_id"], r2["call_id"]);
    assert_eq!(counter.load(Ordering::Relaxed), 2);
}

/// Input values are properly loaded into channels.
#[tokio::test]
async fn execute_input_initialization() {
    let mut graph = StateGraph::new();
    graph.add_last_value_channel("name", json!("default"));
    graph.add_last_value_channel("greeting", json!(null));

    graph
        .add_node(NodeFn::new("greeter", |state: Value, _config| async move {
            let name = state["name"].as_str().unwrap_or("world");
            Ok(json!({"greeting": format!("Hello, {name}!")}))
        }))
        .unwrap();

    graph.set_entry_point("greeter");
    graph.set_finish_point("greeter");

    let compiled = graph.compile().unwrap();
    let config = RunnableConfig::default();

    let result = compiled
        .invoke(json!({"name": "Alice"}), &config)
        .await
        .unwrap();
    assert_eq!(result["greeting"], json!("Hello, Alice!"));
    assert_eq!(result["name"], json!("Alice"));
}

/// ReAct-style cyclic graph: agent -> tool -> agent -> END
#[tokio::test]
async fn execute_react_style_cycle() {
    let call_count = Arc::new(AtomicUsize::new(0));

    let mut graph = StateGraph::new();
    graph.add_append_channel("messages");
    graph.add_last_value_channel("needs_tool", json!(true));

    let cc = call_count.clone();
    graph
        .add_node(NodeFn::new("agent", move |_state: Value, _config| {
            let cc = cc.clone();
            async move {
                let count = cc.fetch_add(1, Ordering::Relaxed);
                if count == 0 {
                    Ok(json!({
                        "messages": {"role": "ai", "content": "I need to use a tool"},
                        "needs_tool": true
                    }))
                } else {
                    Ok(json!({
                        "messages": {"role": "ai", "content": "Final answer: 42"},
                        "needs_tool": false
                    }))
                }
            }
        }))
        .unwrap();

    graph
        .add_node(NodeFn::new(
            "tool",
            |_state: Value, _config| async move {
                Ok(json!({
                    "messages": {"role": "tool", "content": "tool result: 42"}
                }))
            },
        ))
        .unwrap();

    graph.set_entry_point("agent");

    let mut path_map = HashMap::new();
    path_map.insert("tool".to_string(), "tool".to_string());
    path_map.insert("end".to_string(), END.to_string());

    graph.add_conditional_edges(ConditionalEdge::new(
        "agent",
        |state: &Value| {
            if state["needs_tool"].as_bool().unwrap_or(false) {
                "tool".to_string()
            } else {
                "end".to_string()
            }
        },
        Some(path_map),
    ));

    graph.add_edge("tool", "agent");

    let compiled = graph.compile().unwrap();
    let config = RunnableConfig::default();

    let result = compiled
        .invoke(
            json!({"messages": {"role": "user", "content": "What is 6*7?"}}),
            &config,
        )
        .await
        .unwrap();

    let messages = result["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 4); // user, ai (tool call), tool result, ai (final)
    assert_eq!(call_count.load(Ordering::Relaxed), 2);
}
