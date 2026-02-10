use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;

use ayas_core::config::RunnableConfig;
use ayas_core::runnable::Runnable;

use crate::compiled::CompiledStateGraph;
use crate::node::NodeFn;

/// Create a `NodeFn` that executes a sub-graph as a node in a parent graph.
///
/// - `name`: the node name in the parent graph
/// - `inner_graph`: the compiled sub-graph to execute
/// - `input_mapping`: maps parent state keys to sub-graph input keys
///   e.g., `{"parent_messages" => "messages"}` means the parent's "parent_messages"
///   channel value is passed as "messages" to the sub-graph input.
/// - `output_mapping`: maps sub-graph output keys to parent state update keys
///   e.g., `{"result" => "sub_result"}` means the sub-graph's "result" output
///   is written to the parent's "sub_result" channel.
///
/// If input_mapping is empty, the entire parent state is passed as sub-graph input.
/// If output_mapping is empty, the entire sub-graph output is returned as-is.
pub fn subgraph_node(
    name: impl Into<String>,
    inner_graph: Arc<CompiledStateGraph>,
    input_mapping: HashMap<String, String>,
    output_mapping: HashMap<String, String>,
) -> NodeFn {
    NodeFn::new(name, move |state: Value, config: RunnableConfig| {
        let graph = Arc::clone(&inner_graph);
        let in_map = input_mapping.clone();
        let out_map = output_mapping.clone();
        async move {
            // Build sub-graph input from parent state using input_mapping
            let sub_input = if in_map.is_empty() {
                state.clone()
            } else {
                let mut input = serde_json::Map::new();
                if let Value::Object(parent_state) = &state {
                    for (parent_key, sub_key) in &in_map {
                        if let Some(val) = parent_state.get(parent_key) {
                            input.insert(sub_key.clone(), val.clone());
                        }
                    }
                }
                Value::Object(input)
            };

            // Execute sub-graph with reduced recursion limit to prevent infinite nesting
            let sub_config = RunnableConfig {
                recursion_limit: config.recursion_limit.saturating_sub(1),
                ..config
            };

            let sub_output = graph.invoke(sub_input, &sub_config).await?;

            // Map sub-graph output back to parent state updates
            if out_map.is_empty() {
                Ok(sub_output)
            } else {
                let mut output = serde_json::Map::new();
                if let Value::Object(sub_state) = &sub_output {
                    for (sub_key, parent_key) in &out_map {
                        if let Some(val) = sub_state.get(sub_key) {
                            output.insert(parent_key.clone(), val.clone());
                        }
                    }
                }
                Ok(Value::Object(output))
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    use crate::node::NodeFn;
    use crate::state_graph::StateGraph;

    /// Helper: build a simple inner graph with a "count" channel.
    /// Node "a" sets count to 1, node "b" increments count by 1.
    fn build_inner_graph() -> CompiledStateGraph {
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

    #[tokio::test]
    async fn test_subgraph_basic() {
        let inner = Arc::new(build_inner_graph());

        // Outer graph: start → sub → end
        let mut outer = StateGraph::new();
        outer.add_last_value_channel("count", json!(0));

        let sub_node = subgraph_node("sub", inner, HashMap::new(), HashMap::new());
        outer.add_node(sub_node).unwrap();
        outer.set_entry_point("sub");
        outer.set_finish_point("sub");

        let compiled = outer.compile().unwrap();
        let config = RunnableConfig::default();
        let result = compiled.invoke(json!({}), &config).await.unwrap();

        // Inner graph: a sets 1, b increments to 2
        assert_eq!(result["count"], json!(2));
    }

    #[tokio::test]
    async fn test_subgraph_with_input_mapping() {
        let inner = Arc::new(build_inner_graph());

        // Parent has "parent_count", inner expects "count"
        let mut in_map = HashMap::new();
        in_map.insert("parent_count".to_string(), "count".to_string());

        let mut outer = StateGraph::new();
        outer.add_last_value_channel("parent_count", json!(0));
        outer.add_last_value_channel("count", json!(0));

        let sub_node = subgraph_node("sub", inner, in_map, HashMap::new());
        outer.add_node(sub_node).unwrap();
        outer.set_entry_point("sub");
        outer.set_finish_point("sub");

        let compiled = outer.compile().unwrap();
        let config = RunnableConfig::default();
        let result = compiled.invoke(json!({"parent_count": 10}), &config).await.unwrap();

        // Input mapping sends parent_count=10 as count=10 to inner graph
        // Inner: a ignores input and sets count=1, b increments to 2
        // Output (no output mapping) returns count=2
        assert_eq!(result["count"], json!(2));
    }

    #[tokio::test]
    async fn test_subgraph_with_output_mapping() {
        let inner = Arc::new(build_inner_graph());

        // Inner produces "count", outer expects "sub_result"
        let mut out_map = HashMap::new();
        out_map.insert("count".to_string(), "sub_result".to_string());

        let mut outer = StateGraph::new();
        outer.add_last_value_channel("sub_result", json!(0));

        let sub_node = subgraph_node("sub", inner, HashMap::new(), out_map);
        outer.add_node(sub_node).unwrap();
        outer.set_entry_point("sub");
        outer.set_finish_point("sub");

        let compiled = outer.compile().unwrap();
        let config = RunnableConfig::default();
        let result = compiled.invoke(json!({}), &config).await.unwrap();

        // Inner graph produces count=2, mapped to sub_result=2
        assert_eq!(result["sub_result"], json!(2));
    }

    #[tokio::test]
    async fn test_subgraph_with_both_mappings() {
        // Inner graph that reads "x" and produces "y"
        let mut ig = StateGraph::new();
        ig.add_last_value_channel("x", json!(0));
        ig.add_last_value_channel("y", json!(0));

        ig.add_node(NodeFn::new("compute", |state: Value, _cfg| async move {
            let x = state["x"].as_i64().unwrap_or(0);
            Ok(json!({"y": x * 2}))
        }))
        .unwrap();
        ig.set_entry_point("compute");
        ig.set_finish_point("compute");
        let inner = Arc::new(ig.compile().unwrap());

        // Input: parent "input_val" → inner "x"
        // Output: inner "y" → parent "output_val"
        let mut in_map = HashMap::new();
        in_map.insert("input_val".to_string(), "x".to_string());
        let mut out_map = HashMap::new();
        out_map.insert("y".to_string(), "output_val".to_string());

        let mut outer = StateGraph::new();
        outer.add_last_value_channel("input_val", json!(0));
        outer.add_last_value_channel("output_val", json!(0));

        let sub_node = subgraph_node("sub", inner, in_map, out_map);
        outer.add_node(sub_node).unwrap();
        outer.set_entry_point("sub");
        outer.set_finish_point("sub");

        let compiled = outer.compile().unwrap();
        let config = RunnableConfig::default();
        let result = compiled.invoke(json!({"input_val": 5}), &config).await.unwrap();

        // input_val=5 → x=5 → y=10 → output_val=10
        assert_eq!(result["output_val"], json!(10));
    }

    #[tokio::test]
    async fn test_subgraph_passthrough_no_mapping() {
        let inner = Arc::new(build_inner_graph());

        let mut outer = StateGraph::new();
        outer.add_last_value_channel("count", json!(0));

        // Empty mappings = passthrough
        let sub_node = subgraph_node("sub", inner, HashMap::new(), HashMap::new());
        outer.add_node(sub_node).unwrap();
        outer.set_entry_point("sub");
        outer.set_finish_point("sub");

        let compiled = outer.compile().unwrap();
        let config = RunnableConfig::default();
        let result = compiled.invoke(json!({"count": 100}), &config).await.unwrap();

        // Passthrough: entire parent state passed to inner graph
        // Inner: a sets count=1 (ignores input), b increments to 2
        assert_eq!(result["count"], json!(2));
    }

    #[tokio::test]
    async fn test_subgraph_recursion_limit() {
        let inner = Arc::new(build_inner_graph());

        let mut outer = StateGraph::new();
        outer.add_last_value_channel("count", json!(0));

        let sub_node = subgraph_node("sub", inner, HashMap::new(), HashMap::new());
        outer.add_node(sub_node).unwrap();
        outer.set_entry_point("sub");
        outer.set_finish_point("sub");

        let compiled = outer.compile().unwrap();
        // Set recursion limit to 1: the sub-graph will get limit 0,
        // which means it will hit the limit on the first step
        let config = RunnableConfig::default().with_recursion_limit(1);
        let result = compiled.invoke(json!({}), &config).await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Recursion limit"));
    }

    #[tokio::test]
    async fn test_subgraph_nested() {
        // Level 2 (innermost): count += 1
        let mut l2 = StateGraph::new();
        l2.add_last_value_channel("count", json!(0));
        l2.add_node(NodeFn::new("inc", |state: Value, _cfg| async move {
            let c = state["count"].as_i64().unwrap_or(0);
            Ok(json!({"count": c + 1}))
        }))
        .unwrap();
        l2.set_entry_point("inc");
        l2.set_finish_point("inc");
        let inner2 = Arc::new(l2.compile().unwrap());

        // Level 1 (middle): wraps level 2 as subgraph, then adds 10
        let mut l1 = StateGraph::new();
        l1.add_last_value_channel("count", json!(0));
        let sub_l2 = subgraph_node("sub_inner", inner2, HashMap::new(), HashMap::new());
        l1.add_node(sub_l2).unwrap();
        l1.add_node(NodeFn::new("add_ten", |state: Value, _cfg| async move {
            let c = state["count"].as_i64().unwrap_or(0);
            Ok(json!({"count": c + 10}))
        }))
        .unwrap();
        l1.set_entry_point("sub_inner");
        l1.add_edge("sub_inner", "add_ten");
        l1.set_finish_point("add_ten");
        let inner1 = Arc::new(l1.compile().unwrap());

        // Level 0 (outer): wraps level 1 as subgraph
        let mut outer = StateGraph::new();
        outer.add_last_value_channel("count", json!(0));
        let sub_l1 = subgraph_node("sub_outer", inner1, HashMap::new(), HashMap::new());
        outer.add_node(sub_l1).unwrap();
        outer.set_entry_point("sub_outer");
        outer.set_finish_point("sub_outer");

        let compiled = outer.compile().unwrap();
        let config = RunnableConfig::default();
        let result = compiled.invoke(json!({"count": 0}), &config).await.unwrap();

        // L2: 0 + 1 = 1; L1: 1 + 10 = 11
        assert_eq!(result["count"], json!(11));
    }

    #[tokio::test]
    async fn test_subgraph_in_complex_graph() {
        let inner = Arc::new(build_inner_graph());

        // Outer graph: node_a → subgraph_node → node_c
        let mut outer = StateGraph::new();
        outer.add_last_value_channel("count", json!(0));

        outer
            .add_node(NodeFn::new("node_a", |_state: Value, _cfg| async move {
                Ok(json!({"count": 5}))
            }))
            .unwrap();

        let sub_node = subgraph_node("sub", inner, HashMap::new(), HashMap::new());
        outer.add_node(sub_node).unwrap();

        outer
            .add_node(NodeFn::new(
                "node_c",
                |state: Value, _cfg| async move {
                    let c = state["count"].as_i64().unwrap_or(0);
                    Ok(json!({"count": c + 100}))
                },
            ))
            .unwrap();

        outer.set_entry_point("node_a");
        outer.add_edge("node_a", "sub");
        outer.add_edge("sub", "node_c");
        outer.set_finish_point("node_c");

        let compiled = outer.compile().unwrap();
        let config = RunnableConfig::default();
        let result = compiled.invoke(json!({}), &config).await.unwrap();

        // node_a: count=5
        // sub (inner graph): a sets count=1, b sets count=2
        //   (inner graph gets full parent state with count=5, but a ignores it)
        // node_c: count=2+100=102
        assert_eq!(result["count"], json!(102));
    }
}
