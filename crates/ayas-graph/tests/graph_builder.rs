use std::collections::HashMap;

use ayas_core::config::RunnableConfig;
use ayas_graph::prelude::*;
use serde_json::{json, Value};

fn make_node(name: &str, key: &str, val: Value) -> NodeFn {
    let key = key.to_string();
    NodeFn::new(name, move |mut state: Value, _config| {
        let key = key.clone();
        let val = val.clone();
        async move {
            state[key] = val;
            Ok(state)
        }
    })
}

/// Linear graph: START -> a -> b -> c -> END
#[tokio::test]
async fn e2e_linear_graph() {
    let mut graph = StateGraph::new();
    graph.add_last_value_channel("step", json!(null));

    graph.add_node(make_node("a", "step", json!("a"))).unwrap();
    graph.add_node(make_node("b", "step", json!("b"))).unwrap();
    graph.add_node(make_node("c", "step", json!("c"))).unwrap();

    graph.set_entry_point("a");
    graph.add_edge("a", "b");
    graph.add_edge("b", "c");
    graph.set_finish_point("c");

    let compiled = graph.compile().unwrap();

    // Verify topology
    assert_eq!(compiled.entry_point(), "a");
    assert!(compiled.edges_from(START).contains(&"a".to_string()));
    assert!(compiled.edges_from("a").contains(&"b".to_string()));
    assert!(compiled.edges_from("b").contains(&"c".to_string()));
    assert!(compiled.edges_from("c").contains(&END.to_string()));
    assert_eq!(compiled.finish_points(), &["c"]);

    // Verify nodes can be invoked
    let config = RunnableConfig::default();
    let node_a = compiled.node("a").unwrap();
    let result: Value = node_a.invoke(json!({}), &config).await.unwrap();
    assert_eq!(result["step"], json!("a"));
}

/// Conditional graph: START -> router --(yes)--> approve -> END
///                                     --(no)---> reject -> END
#[tokio::test]
async fn e2e_conditional_graph() {
    let mut graph = StateGraph::new();
    graph.add_last_value_channel("decision", json!(null));

    graph
        .add_node(NodeFn::new("router", |state: Value, _config| async move {
            Ok(state)
        }))
        .unwrap();
    graph
        .add_node(make_node("approve", "decision", json!("approved")))
        .unwrap();
    graph
        .add_node(make_node("reject", "decision", json!("rejected")))
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

    // Verify topology
    assert_eq!(compiled.entry_point(), "router");
    assert_eq!(compiled.node_names().len(), 3);
    assert_eq!(compiled.finish_points().len(), 2);

    // Verify conditional routing
    let ce = &compiled.conditional_edges()[0];
    assert_eq!(ce.resolve(&json!({"score": 0.8})), "approve");
    assert_eq!(ce.resolve(&json!({"score": 0.3})), "reject");
}

/// Cyclic graph: START -> agent -> tool -> agent (cycle) -> END
#[tokio::test]
async fn e2e_cyclic_graph() {
    let mut graph = StateGraph::new();

    graph
        .add_node(NodeFn::new("agent", |state: Value, _config| async move {
            Ok(state)
        }))
        .unwrap();
    graph
        .add_node(NodeFn::new("tool", |state: Value, _config| async move {
            Ok(state)
        }))
        .unwrap();

    graph.set_entry_point("agent");

    // agent can go to tool or END
    let mut path_map = HashMap::new();
    path_map.insert("continue".to_string(), "tool".to_string());
    path_map.insert("finish".to_string(), END.to_string());

    graph.add_conditional_edges(ConditionalEdge::new(
        "agent",
        |state: &Value| {
            if state["needs_tool"].as_bool().unwrap_or(false) {
                "continue".to_string()
            } else {
                "finish".to_string()
            }
        },
        Some(path_map),
    ));

    // tool -> agent (back edge forming cycle)
    graph.add_edge("tool", "agent");

    let compiled = graph.compile().unwrap();

    // Verify cycle is valid
    assert_eq!(compiled.entry_point(), "agent");
    assert!(compiled.edges_from("tool").contains(&"agent".to_string()));

    // Both nodes reachable
    let names = compiled.node_names();
    assert!(names.contains(&"agent"));
    assert!(names.contains(&"tool"));
}

/// Diamond graph: START -> a -> b -> d -> END
///                          \-> c -/
#[tokio::test]
async fn e2e_diamond_graph() {
    let mut graph = StateGraph::new();

    graph.add_node(make_node("a", "a", json!(true))).unwrap();
    graph.add_node(make_node("b", "b", json!(true))).unwrap();
    graph.add_node(make_node("c", "c", json!(true))).unwrap();
    graph.add_node(make_node("d", "d", json!(true))).unwrap();

    graph.set_entry_point("a");
    graph.add_edge("a", "b");
    graph.add_edge("a", "c");
    graph.add_edge("b", "d");
    graph.add_edge("c", "d");
    graph.set_finish_point("d");

    let compiled = graph.compile().unwrap();
    assert_eq!(compiled.entry_point(), "a");

    let from_a = compiled.edges_from("a");
    assert!(from_a.contains(&"b".to_string()));
    assert!(from_a.contains(&"c".to_string()));
}

/// Verify that AppendChannel works across multiple updates.
#[test]
fn e2e_append_channel_accumulation() {
    use ayas_graph::channel::Channel;

    let mut ch = AppendChannel::new();

    // Simulate multiple step updates
    ch.update(vec![json!({"role": "user", "content": "hello"})]).unwrap();
    ch.update(vec![json!({"role": "ai", "content": "hi"})]).unwrap();
    ch.update(vec![json!({"role": "user", "content": "bye"})]).unwrap();

    let messages = ch.get();
    assert_eq!(
        messages,
        &json!([
            {"role": "user", "content": "hello"},
            {"role": "ai", "content": "hi"},
            {"role": "user", "content": "bye"}
        ])
    );

    // Checkpoint and restore
    let cp = ch.checkpoint();
    ch.update(vec![json!({"role": "ai", "content": "goodbye"})]).unwrap();
    assert_eq!(ch.get().as_array().unwrap().len(), 4);

    ch.restore(cp);
    assert_eq!(ch.get().as_array().unwrap().len(), 3);
}
