use serde_json::{json, Value};
use tokio::sync::mpsc;

use ayas_checkpoint::prelude::{
    command_output, send_output, CheckpointConfigExt, MemoryCheckpointStore, SendDirective,
};
use ayas_core::config::RunnableConfig;
use ayas_graph::prelude::*;

/// Helper: build a 2-node linear graph: a → b → END
fn build_ab_graph() -> CompiledStateGraph {
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

    g.set_entry_point("a");
    g.add_edge("a", "b");
    g.set_finish_point("b");
    g.compile().unwrap()
}

/// Helper: build a 3-node linear graph: a → b → c → END
fn build_abc_graph() -> CompiledStateGraph {
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

fn default_config() -> RunnableConfig {
    RunnableConfig::default()
}

/// Collect all events from a receiver into a Vec.
async fn collect_events(mut rx: mpsc::Receiver<StreamEvent>) -> Vec<StreamEvent> {
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }
    events
}

#[tokio::test]
async fn test_streaming_basic() {
    let graph = build_ab_graph();
    let config = default_config();
    let (tx, rx) = mpsc::channel(64);

    let result = graph
        .invoke_with_streaming(json!({}), &config, tx)
        .await
        .unwrap();

    assert_eq!(result["count"], json!(2));

    let events = collect_events(rx).await;

    // Expected: NodeStart(a), NodeEnd(a), NodeStart(b), NodeEnd(b), GraphComplete
    assert_eq!(events.len(), 5);

    assert!(matches!(&events[0], StreamEvent::NodeStart { node_name, step } if node_name == "a" && *step == 0));
    assert!(matches!(&events[1], StreamEvent::NodeEnd { node_name, step, .. } if node_name == "a" && *step == 0));
    assert!(matches!(&events[2], StreamEvent::NodeStart { node_name, step } if node_name == "b" && *step == 1));
    assert!(matches!(&events[3], StreamEvent::NodeEnd { node_name, step, .. } if node_name == "b" && *step == 1));
    assert!(matches!(&events[4], StreamEvent::GraphComplete { .. }));
}

#[tokio::test]
async fn test_streaming_event_count() {
    let graph = build_abc_graph();
    let config = default_config();
    let (tx, rx) = mpsc::channel(64);

    graph
        .invoke_with_streaming(json!({}), &config, tx)
        .await
        .unwrap();

    let events = collect_events(rx).await;

    // 3 nodes → 3 * 2 (NodeStart + NodeEnd) + 1 GraphComplete = 7
    let node_starts = events
        .iter()
        .filter(|e| matches!(e, StreamEvent::NodeStart { .. }))
        .count();
    let node_ends = events
        .iter()
        .filter(|e| matches!(e, StreamEvent::NodeEnd { .. }))
        .count();
    let graph_completes = events
        .iter()
        .filter(|e| matches!(e, StreamEvent::GraphComplete { .. }))
        .count();

    assert_eq!(node_starts, 3);
    assert_eq!(node_ends, 3);
    assert_eq!(graph_completes, 1);
    assert_eq!(events.len(), 7);
}

#[tokio::test]
async fn test_streaming_node_end_state() {
    let graph = build_abc_graph();
    let config = default_config();
    let (tx, rx) = mpsc::channel(64);

    graph
        .invoke_with_streaming(json!({}), &config, tx)
        .await
        .unwrap();

    let events = collect_events(rx).await;

    // Check state after each NodeEnd
    let node_ends: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::NodeEnd {
                node_name, state, ..
            } => Some((node_name.clone(), state.clone())),
            _ => None,
        })
        .collect();

    assert_eq!(node_ends[0].0, "a");
    assert_eq!(node_ends[0].1["count"], json!(1));
    assert_eq!(node_ends[1].0, "b");
    assert_eq!(node_ends[1].1["count"], json!(2));
    assert_eq!(node_ends[2].0, "c");
    assert_eq!(node_ends[2].1["count"], json!(3));
}

#[tokio::test]
async fn test_streaming_interrupt() {
    let mut g = StateGraph::new();
    g.add_last_value_channel("count", json!(0));

    g.add_node(NodeFn::new("a", |_state: Value, _cfg| async move {
        Ok(json!({"count": 1}))
    }))
    .unwrap();
    g.add_node(NodeFn::new(
        "interrupter",
        |state: Value, _cfg| async move {
            let c = state["count"].as_i64().unwrap_or(0);
            Ok(json!({
                "count": c + 1,
                "__interrupt__": {"value": "approve?"}
            }))
        },
    ))
    .unwrap();
    g.add_node(NodeFn::new("c", |state: Value, _cfg| async move {
        let c = state["count"].as_i64().unwrap_or(0);
        Ok(json!({"count": c + 1}))
    }))
    .unwrap();

    g.set_entry_point("a");
    g.add_edge("a", "interrupter");
    g.add_edge("interrupter", "c");
    g.set_finish_point("c");

    let graph = g.compile().unwrap();
    let store = MemoryCheckpointStore::new();
    let config = default_config().with_thread_id("stream-interrupt");
    let (tx, rx) = mpsc::channel(64);

    let result = graph
        .invoke_resumable_with_streaming(json!({}), &config, &store, tx)
        .await
        .unwrap();

    assert!(result.is_interrupted());

    let events = collect_events(rx).await;

    // NodeStart(a), NodeEnd(a), NodeStart(interrupter), NodeEnd(interrupter), Interrupted
    assert_eq!(events.len(), 5);
    assert!(matches!(&events[0], StreamEvent::NodeStart { node_name, .. } if node_name == "a"));
    assert!(matches!(&events[1], StreamEvent::NodeEnd { node_name, .. } if node_name == "a"));
    assert!(matches!(&events[2], StreamEvent::NodeStart { node_name, .. } if node_name == "interrupter"));
    assert!(matches!(&events[3], StreamEvent::NodeEnd { node_name, .. } if node_name == "interrupter"));
    assert!(
        matches!(&events[4], StreamEvent::Interrupted { interrupt_value, .. } if *interrupt_value == json!("approve?"))
    );
}

#[tokio::test]
async fn test_streaming_command() {
    let mut g = StateGraph::new();
    g.add_last_value_channel("count", json!(0));

    g.add_node(NodeFn::new("a", |_state: Value, _cfg| async move {
        Ok(command_output(json!({"count": 10}), "b"))
    }))
    .unwrap();
    g.add_node(NodeFn::new("b", |state: Value, _cfg| async move {
        let c = state["count"].as_i64().unwrap_or(0);
        Ok(json!({"count": c + 1}))
    }))
    .unwrap();

    g.set_entry_point("a");
    g.add_edge("a", "b");
    g.set_finish_point("b");

    let graph = g.compile().unwrap();
    let config = default_config();
    let (tx, rx) = mpsc::channel(64);

    let result = graph
        .invoke_with_streaming(json!({}), &config, tx)
        .await
        .unwrap();

    assert_eq!(result["count"], json!(11));

    let events = collect_events(rx).await;

    // NodeStart(a), NodeEnd(a), NodeStart(b), NodeEnd(b), GraphComplete
    assert_eq!(events.len(), 5);
    assert!(matches!(&events[0], StreamEvent::NodeStart { node_name, .. } if node_name == "a"));
    assert!(matches!(&events[1], StreamEvent::NodeEnd { node_name, state, .. } if node_name == "a" && state["count"] == json!(10)));
    assert!(matches!(&events[2], StreamEvent::NodeStart { node_name, .. } if node_name == "b"));
    assert!(matches!(&events[3], StreamEvent::NodeEnd { node_name, state, .. } if node_name == "b" && state["count"] == json!(11)));
    assert!(matches!(&events[4], StreamEvent::GraphComplete { output } if output["count"] == json!(11)));
}

#[tokio::test]
async fn test_streaming_dropped_receiver() {
    let graph = build_abc_graph();
    let config = default_config();
    let (tx, rx) = mpsc::channel(64);

    // Drop receiver immediately
    drop(rx);

    // Graph should still complete without error even though receiver is gone
    let result = graph
        .invoke_with_streaming(json!({}), &config, tx)
        .await
        .unwrap();

    assert_eq!(result["count"], json!(3));
}

#[tokio::test]
async fn test_streaming_send() {
    let mut g = StateGraph::new();
    g.add_last_value_channel("count", json!(0));

    g.add_node(NodeFn::new(
        "dispatcher",
        |_state: Value, _cfg| async move {
            Ok(send_output(vec![
                SendDirective::new("worker_a", json!({})),
                SendDirective::new("worker_b", json!({})),
            ]))
        },
    ))
    .unwrap();
    g.add_node(NodeFn::new("worker_a", |state: Value, _cfg| async move {
        let c = state["count"].as_i64().unwrap_or(0);
        Ok(json!({"count": c + 10}))
    }))
    .unwrap();
    g.add_node(NodeFn::new("worker_b", |state: Value, _cfg| async move {
        let c = state["count"].as_i64().unwrap_or(0);
        Ok(json!({"count": c + 100}))
    }))
    .unwrap();
    g.add_node(NodeFn::new(
        "collector",
        |state: Value, _cfg| async move { Ok(state) },
    ))
    .unwrap();

    g.set_entry_point("dispatcher");
    g.add_edge("dispatcher", "collector");
    g.add_conditional_edges(ConditionalEdge::new(
        "collector",
        |_: &Value| END.to_string(),
        None,
    ));
    g.set_finish_point("collector");

    let graph = g.compile().unwrap();
    let config = default_config();
    let (tx, rx) = mpsc::channel(64);

    let result = graph
        .invoke_with_streaming(json!({}), &config, tx)
        .await
        .unwrap();

    assert_eq!(result["count"], json!(110));

    let events = collect_events(rx).await;

    // dispatcher: NodeStart + NodeEnd, collector: NodeStart + NodeEnd, GraphComplete
    let node_starts: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::NodeStart { node_name, .. } => Some(node_name.clone()),
            _ => None,
        })
        .collect();
    let node_ends: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::NodeEnd { node_name, .. } => Some(node_name.clone()),
            _ => None,
        })
        .collect();

    assert_eq!(node_starts, vec!["dispatcher", "collector"]);
    assert_eq!(node_ends, vec!["dispatcher", "collector"]);
    assert!(matches!(events.last().unwrap(), StreamEvent::GraphComplete { .. }));
}

#[tokio::test]
async fn test_streaming_serialization() {
    // Verify StreamEvent can be serialized to JSON
    let event = StreamEvent::NodeStart {
        node_name: "test".to_string(),
        step: 0,
    };
    let json_str = serde_json::to_string(&event).unwrap();
    assert!(json_str.contains("\"type\":\"node_start\""));
    assert!(json_str.contains("\"node_name\":\"test\""));

    let event = StreamEvent::GraphComplete {
        output: json!({"count": 42}),
    };
    let json_str = serde_json::to_string(&event).unwrap();
    assert!(json_str.contains("\"type\":\"graph_complete\""));

    // Roundtrip
    let deserialized: StreamEvent = serde_json::from_str(&json_str).unwrap();
    assert!(matches!(deserialized, StreamEvent::GraphComplete { output } if output["count"] == json!(42)));
}
