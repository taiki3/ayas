use serde_json::Value;

use ayas_core::error::Result;
use ayas_graph::channel::ChannelSpec;
use ayas_graph::compiled::CompiledStateGraph;
use ayas_graph::constants::END;
use ayas_graph::edge::ConditionalEdge;
use ayas_graph::node::NodeFn;
use ayas_graph::state_graph::StateGraph;

use crate::types::{GraphChannelDto, GraphEdgeDto, GraphNodeDto};

/// Convert frontend graph DTOs into a compiled StateGraph.
pub fn convert_to_state_graph(
    nodes: &[GraphNodeDto],
    edges: &[GraphEdgeDto],
    channels: &[GraphChannelDto],
) -> Result<CompiledStateGraph> {
    let mut graph = StateGraph::new();

    // Add channels
    for ch in channels {
        let spec = match ch.channel_type.as_str() {
            "Append" | "append" => ChannelSpec::Append,
            _ => {
                let default = ch.default.clone().unwrap_or(Value::String(String::new()));
                ChannelSpec::LastValue { default }
            }
        };
        graph.add_channel(&ch.key, spec);
    }

    // If no channels provided, add a default "value" channel
    if channels.is_empty() {
        graph.add_channel(
            "value",
            ChannelSpec::LastValue {
                default: Value::String(String::new()),
            },
        );
    }

    // Determine entry and finish from edges
    let mut entry_node: Option<String> = None;
    let mut finish_nodes: Vec<String> = Vec::new();

    for edge in edges {
        if edge.from == "start" && edge.to != "end" {
            entry_node = Some(edge.to.clone());
        }
        if edge.to == "end" && edge.from != "start" {
            finish_nodes.push(edge.from.clone());
        }
    }

    // Handle start→end direct edge: need a synthetic passthrough node
    let direct_start_end = edges
        .iter()
        .any(|e| e.from == "start" && e.to == "end");

    if direct_start_end && entry_node.is_none() {
        // Create a synthetic passthrough node for start→end
        let synthetic = NodeFn::new("__passthrough__", |state: Value, _cfg| async move {
            Ok(state)
        });
        graph.add_node(synthetic)?;
        entry_node = Some("__passthrough__".to_string());
        finish_nodes.push("__passthrough__".to_string());
    }

    // Add nodes (skip start/end - they are virtual)
    for node in nodes {
        match node.node_type.as_str() {
            "start" | "end" => continue,
            "llm" => {
                let config = node.config.clone().unwrap_or(Value::Null);
                let id = node.id.clone();
                let node_fn = NodeFn::new(id, move |state: Value, _cfg| {
                    let config = config.clone();
                    Box::pin(async move {
                        let mut state = state;
                        if let Value::Object(ref mut map) = state
                            && let Some(prompt) =
                                config.get("prompt").and_then(|v| v.as_str())
                        {
                            map.insert(
                                "last_prompt".to_string(),
                                Value::String(prompt.to_string()),
                            );
                        }
                        Ok(state)
                    })
                });
                graph.add_node(node_fn)?;
            }
            "transform" => {
                let config = node.config.clone().unwrap_or(Value::Null);
                let id = node.id.clone();
                let node_fn = NodeFn::new(id, move |state: Value, _cfg| {
                    let config = config.clone();
                    Box::pin(async move {
                        let mut state = state;
                        if let Value::Object(ref mut map) = state
                            && let Some(expr) =
                                config.get("expression").and_then(|v| v.as_str())
                        {
                            map.insert(
                                "transform_applied".to_string(),
                                Value::String(expr.to_string()),
                            );
                        }
                        Ok(state)
                    })
                });
                graph.add_node(node_fn)?;
            }
            _ => {
                // passthrough / conditional / unknown → passthrough node
                let id = node.id.clone();
                let node_fn =
                    NodeFn::new(id, |state: Value, _cfg| async move { Ok(state) });
                graph.add_node(node_fn)?;
            }
        }
    }

    // Set entry point
    if let Some(ref entry) = entry_node {
        graph.set_entry_point(entry);
    }

    // Set finish points
    for fp in &finish_nodes {
        graph.set_finish_point(fp);
    }

    // Add edges (skip start→X and X→end, handled by entry/finish points)
    for edge in edges {
        if edge.from == "start" || edge.to == "end" {
            continue;
        }

        if let Some(condition) = &edge.condition {
            // Conditional edge: route based on a state field
            let condition = condition.clone();
            let to_target = edge.to.clone();

            let cond_edge = ConditionalEdge::new(
                &edge.from,
                move |state: &Value| {
                    if let Some(val) = state.get(&condition)
                        && (val.as_bool().unwrap_or(false)
                            || val.as_str().is_some_and(|s| !s.is_empty()))
                    {
                        return to_target.clone();
                    }
                    END.to_string()
                },
                None,
            );
            graph.add_conditional_edges(cond_edge);
        } else {
            graph.add_edge(&edge.from, &edge.to);
        }
    }

    graph.compile()
}

/// Validate a graph structure without compiling.
pub fn validate_graph(
    nodes: &[GraphNodeDto],
    edges: &[GraphEdgeDto],
    _channels: &[GraphChannelDto],
) -> Vec<String> {
    let mut errors = Vec::new();

    // Check for start edge
    let has_start = edges.iter().any(|e| e.from == "start");
    if !has_start {
        errors.push("Graph must have an edge from 'start'".into());
    }

    // Check for end edge
    let has_end = edges.iter().any(|e| e.to == "end");
    if !has_end {
        errors.push("Graph must have an edge to 'end'".into());
    }

    // Check all edge targets exist as nodes
    let node_ids: std::collections::HashSet<&str> =
        nodes.iter().map(|n| n.id.as_str()).collect();
    for edge in edges {
        if edge.from != "start" && !node_ids.contains(edge.from.as_str()) {
            errors.push(format!("Edge from unknown node '{}'", edge.from));
        }
        if edge.to != "end" && !node_ids.contains(edge.to.as_str()) {
            errors.push(format!("Edge to unknown node '{}'", edge.to));
        }
    }

    // Check for unreachable nodes
    let mut reachable: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut queue: Vec<&str> = edges
        .iter()
        .filter(|e| e.from == "start")
        .map(|e| e.to.as_str())
        .collect();
    while let Some(node) = queue.pop() {
        if node == "end" || !reachable.insert(node) {
            continue;
        }
        for edge in edges {
            if edge.from == node {
                queue.push(&edge.to);
            }
        }
    }
    for node in nodes {
        if node.node_type != "start"
            && node.node_type != "end"
            && !reachable.contains(node.id.as_str())
        {
            errors.push(format!("Node '{}' is unreachable", node.id));
        }
    }

    errors
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{GraphChannelDto, GraphEdgeDto, GraphNodeDto};

    fn node(id: &str, node_type: &str) -> GraphNodeDto {
        GraphNodeDto {
            id: id.into(),
            node_type: node_type.into(),
            label: None,
            config: None,
        }
    }

    fn edge(from: &str, to: &str) -> GraphEdgeDto {
        GraphEdgeDto {
            from: from.into(),
            to: to.into(),
            condition: None,
        }
    }

    fn channel(key: &str, channel_type: &str) -> GraphChannelDto {
        GraphChannelDto {
            key: key.into(),
            channel_type: channel_type.into(),
            default: None,
        }
    }

    #[test]
    fn convert_start_end_only() {
        let nodes = vec![];
        let edges = vec![edge("start", "end")];
        let channels = vec![channel("value", "LastValue")];
        let result = convert_to_state_graph(&nodes, &edges, &channels);
        assert!(result.is_ok());
    }

    #[test]
    fn convert_linear_pipeline() {
        let nodes = vec![node("transform_1", "transform")];
        let edges = vec![edge("start", "transform_1"), edge("transform_1", "end")];
        let channels = vec![channel("value", "LastValue")];
        let result = convert_to_state_graph(&nodes, &edges, &channels);
        assert!(result.is_ok());
        let compiled = result.unwrap();
        assert!(compiled.node_names().contains(&"transform_1"));
    }

    #[test]
    fn convert_llm_node_with_config() {
        let mut n = node("llm_1", "llm");
        n.config = Some(serde_json::json!({
            "prompt": "Hello",
            "provider": "gemini",
            "model": "gemini-2.0-flash"
        }));
        let nodes = vec![n];
        let edges = vec![edge("start", "llm_1"), edge("llm_1", "end")];
        let channels = vec![channel("value", "LastValue")];
        let result = convert_to_state_graph(&nodes, &edges, &channels);
        assert!(result.is_ok());
    }

    #[test]
    fn convert_conditional_node() {
        let nodes = vec![
            node("check", "conditional"),
            node("branch_a", "passthrough"),
        ];
        let edges = vec![
            edge("start", "check"),
            GraphEdgeDto {
                from: "check".into(),
                to: "branch_a".into(),
                condition: Some("flag".into()),
            },
            edge("branch_a", "end"),
        ];
        let channels = vec![channel("value", "LastValue"), channel("flag", "LastValue")];
        let result = convert_to_state_graph(&nodes, &edges, &channels);
        assert!(result.is_ok());
    }

    #[test]
    fn convert_channels_last_value() {
        let channels = vec![channel("text", "LastValue")];
        let nodes = vec![node("n1", "passthrough")];
        let edges = vec![edge("start", "n1"), edge("n1", "end")];
        let result = convert_to_state_graph(&nodes, &edges, &channels);
        assert!(result.is_ok());
        let compiled = result.unwrap();
        assert!(compiled.has_channel("text"));
    }

    #[test]
    fn convert_channels_append() {
        let channels = vec![channel("messages", "Append")];
        let nodes = vec![node("n1", "passthrough")];
        let edges = vec![edge("start", "n1"), edge("n1", "end")];
        let result = convert_to_state_graph(&nodes, &edges, &channels);
        assert!(result.is_ok());
        let compiled = result.unwrap();
        assert!(compiled.has_channel("messages"));
    }

    #[test]
    fn convert_complex_graph() {
        let nodes = vec![
            node("preprocessor", "transform"),
            node("llm_1", "llm"),
            node("router", "conditional"),
            node("handler_a", "passthrough"),
            node("handler_b", "passthrough"),
        ];
        let edges = vec![
            edge("start", "preprocessor"),
            edge("preprocessor", "llm_1"),
            edge("llm_1", "router"),
            GraphEdgeDto {
                from: "router".into(),
                to: "handler_a".into(),
                condition: Some("route_a".into()),
            },
            edge("handler_a", "end"),
            edge("handler_b", "end"),
        ];
        let channels = vec![
            channel("value", "LastValue"),
            channel("route_a", "LastValue"),
        ];
        let result = convert_to_state_graph(&nodes, &edges, &channels);
        assert!(result.is_ok());
    }

    #[test]
    fn convert_and_compile_succeeds() {
        let nodes = vec![node("step1", "passthrough"), node("step2", "transform")];
        let edges = vec![
            edge("start", "step1"),
            edge("step1", "step2"),
            edge("step2", "end"),
        ];
        let channels = vec![channel("value", "LastValue")];
        let result = convert_to_state_graph(&nodes, &edges, &channels);
        assert!(result.is_ok());
        let compiled = result.unwrap();
        assert_eq!(compiled.node_names().len(), 2);
    }
}
