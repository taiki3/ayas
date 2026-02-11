use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::types::{
    AdlChannel, AdlChannelType, AdlCondition, AdlDocument, AdlEdge, AdlEdgeType, AdlNode,
    AgentMetadata,
};

// ---------------------------------------------------------------------------
// ReactFlow types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReactFlowGraph {
    pub nodes: Vec<ReactFlowNode>,
    pub edges: Vec<ReactFlowEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReactFlowNode {
    pub id: String,
    #[serde(rename = "type")]
    pub type_: String,
    pub position: Position,
    pub data: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReactFlowEdge {
    pub id: String,
    pub source: String,
    pub target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    pub x: f64,
    pub y: f64,
}

// ---------------------------------------------------------------------------
// ADL → ReactFlow
// ---------------------------------------------------------------------------

/// Convert an ADL document to a ReactFlow graph.
/// Includes simple auto-layout (top-to-bottom, dagre-style placement).
pub fn adl_to_reactflow(doc: &AdlDocument) -> ReactFlowGraph {
    let node_spacing_y = 150.0;
    let node_spacing_x = 250.0;

    // Build a set of all referenced node IDs (including sentinels)
    let mut all_node_ids: Vec<String> = Vec::new();

    // Add sentinel start
    all_node_ids.push("__start__".to_string());

    // Add user-defined nodes
    for node in &doc.nodes {
        all_node_ids.push(node.id.clone());
    }

    // Add sentinel end
    all_node_ids.push("__end__".to_string());

    // Simple layout: vertical stack
    let nodes: Vec<ReactFlowNode> = all_node_ids
        .iter()
        .enumerate()
        .map(|(i, id)| {
            let (type_, data) = if id == "__start__" {
                ("input".to_string(), serde_json::json!({"label": "START"}))
            } else if id == "__end__" {
                ("output".to_string(), serde_json::json!({"label": "END"}))
            } else {
                // Find the ADL node
                let adl_node = doc.nodes.iter().find(|n| n.id == *id);
                let node_type = adl_node
                    .map(|n| n.node_type.clone())
                    .unwrap_or("default".to_string());
                let config = adl_node
                    .map(|n| serde_json::to_value(&n.config).unwrap_or(Value::Object(Default::default())))
                    .unwrap_or(Value::Object(Default::default()));
                (
                    node_type.clone(),
                    serde_json::json!({
                        "label": id,
                        "nodeType": node_type,
                        "config": config,
                    }),
                )
            };

            ReactFlowNode {
                id: id.clone(),
                type_,
                position: Position {
                    x: node_spacing_x,
                    y: i as f64 * node_spacing_y,
                },
                data,
            }
        })
        .collect();

    // Convert edges
    let mut edges = Vec::new();
    for (i, adl_edge) in doc.edges.iter().enumerate() {
        let from = normalize_id(&adl_edge.from);

        match adl_edge.edge_type {
            AdlEdgeType::Static => {
                let to = adl_edge
                    .to
                    .as_ref()
                    .map(|t| normalize_id(t))
                    .unwrap_or_else(|| "__end__".to_string());

                edges.push(ReactFlowEdge {
                    id: format!("e{i}"),
                    source: from,
                    target: to,
                    label: None,
                });
            }
            AdlEdgeType::Conditional => {
                for (j, cond) in adl_edge.conditions.iter().enumerate() {
                    let to = normalize_id(&cond.to);
                    let label = if cond.expression == "default" {
                        Some("default".to_string())
                    } else {
                        Some(cond.expression.clone())
                    };
                    edges.push(ReactFlowEdge {
                        id: format!("e{i}c{j}"),
                        source: from.clone(),
                        target: to,
                        label,
                    });
                }
            }
        }
    }

    ReactFlowGraph { nodes, edges }
}

// ---------------------------------------------------------------------------
// ReactFlow → ADL
// ---------------------------------------------------------------------------

/// Convert a ReactFlow graph back to an ADL document.
pub fn reactflow_to_adl(graph: &ReactFlowGraph) -> AdlDocument {
    let mut nodes = Vec::new();
    let mut channels = Vec::new();
    let mut agent = None;

    for rf_node in &graph.nodes {
        // Skip sentinel nodes
        if rf_node.id == "__start__" || rf_node.id == "__end__" {
            continue;
        }

        let node_type = rf_node
            .data
            .get("nodeType")
            .and_then(|v| v.as_str())
            .unwrap_or(&rf_node.type_)
            .to_string();

        let config = rf_node
            .data
            .get("config")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        nodes.push(AdlNode {
            id: rf_node.id.clone(),
            node_type,
            config,
        });

        // Extract channel info from data if present
        if let Some(ch_array) = rf_node.data.get("channels") {
            if let Some(arr) = ch_array.as_array() {
                for ch_val in arr {
                    if let Ok(ch) = serde_json::from_value::<ChannelDef>(ch_val.clone()) {
                        channels.push(AdlChannel {
                            name: ch.name,
                            channel_type: match ch.channel_type.as_str() {
                                "append" => AdlChannelType::Append,
                                "topic" => AdlChannelType::Topic,
                                _ => AdlChannelType::LastValue,
                            },
                            schema: None,
                            default: ch.default,
                        });
                    }
                }
            }
        }

        // Extract agent metadata
        if let Some(agent_val) = rf_node.data.get("agent") {
            if let Some(name) = agent_val.get("name").and_then(|v| v.as_str()) {
                agent = Some(AgentMetadata {
                    name: name.to_string(),
                    description: agent_val
                        .get("description")
                        .and_then(|v| v.as_str())
                        .map(String::from),
                });
            }
        }
    }

    // Group edges by source to detect conditional edges
    let mut edge_groups: std::collections::HashMap<String, Vec<&ReactFlowEdge>> =
        std::collections::HashMap::new();
    for edge in &graph.edges {
        edge_groups.entry(edge.source.clone()).or_default().push(edge);
    }

    let mut edges = Vec::new();
    for (source, group) in &edge_groups {
        // If any edge in this group has a label, treat as conditional
        let has_labels = group.iter().any(|e| e.label.is_some());

        if has_labels && group.len() > 1 {
            let conditions: Vec<AdlCondition> = group
                .iter()
                .map(|e| AdlCondition {
                    expression: e
                        .label
                        .as_deref()
                        .unwrap_or("default")
                        .to_string(),
                    to: normalize_id(&e.target),
                })
                .collect();

            edges.push(AdlEdge {
                from: normalize_id(source),
                to: None,
                edge_type: AdlEdgeType::Conditional,
                conditions,
            });
        } else {
            for e in group {
                edges.push(AdlEdge {
                    from: normalize_id(source),
                    to: Some(normalize_id(&e.target)),
                    edge_type: AdlEdgeType::Static,
                    conditions: vec![],
                });
            }
        }
    }

    AdlDocument {
        version: "1.0".to_string(),
        agent,
        channels,
        nodes,
        edges,
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn normalize_id(id: &str) -> String {
    crate::types::normalize_sentinel(id)
}

#[derive(Deserialize)]
struct ChannelDef {
    name: String,
    #[serde(rename = "type")]
    channel_type: String,
    #[serde(default)]
    default: Option<Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    fn make_simple_adl() -> AdlDocument {
        AdlDocument {
            version: "1.0".to_string(),
            agent: Some(AgentMetadata {
                name: "test-agent".into(),
                description: None,
            }),
            channels: vec![AdlChannel {
                name: "messages".into(),
                channel_type: AdlChannelType::Append,
                schema: None,
                default: None,
            }],
            nodes: vec![
                AdlNode {
                    id: "node_a".into(),
                    node_type: "passthrough".into(),
                    config: HashMap::new(),
                },
                AdlNode {
                    id: "node_b".into(),
                    node_type: "transform".into(),
                    config: HashMap::from([("key".into(), json!("value"))]),
                },
            ],
            edges: vec![
                AdlEdge {
                    from: "__start__".into(),
                    to: Some("node_a".into()),
                    edge_type: AdlEdgeType::Static,
                    conditions: vec![],
                },
                AdlEdge {
                    from: "node_a".into(),
                    to: Some("node_b".into()),
                    edge_type: AdlEdgeType::Static,
                    conditions: vec![],
                },
                AdlEdge {
                    from: "node_b".into(),
                    to: Some("__end__".into()),
                    edge_type: AdlEdgeType::Static,
                    conditions: vec![],
                },
            ],
        }
    }

    fn make_conditional_adl() -> AdlDocument {
        AdlDocument {
            version: "1.0".to_string(),
            agent: None,
            channels: vec![],
            nodes: vec![
                AdlNode {
                    id: "router".into(),
                    node_type: "passthrough".into(),
                    config: HashMap::new(),
                },
                AdlNode {
                    id: "path_a".into(),
                    node_type: "passthrough".into(),
                    config: HashMap::new(),
                },
                AdlNode {
                    id: "path_b".into(),
                    node_type: "passthrough".into(),
                    config: HashMap::new(),
                },
            ],
            edges: vec![
                AdlEdge {
                    from: "__start__".into(),
                    to: Some("router".into()),
                    edge_type: AdlEdgeType::Static,
                    conditions: vec![],
                },
                AdlEdge {
                    from: "router".into(),
                    to: None,
                    edge_type: AdlEdgeType::Conditional,
                    conditions: vec![
                        AdlCondition {
                            expression: "state.x > 0".into(),
                            to: "path_a".into(),
                        },
                        AdlCondition {
                            expression: "default".into(),
                            to: "path_b".into(),
                        },
                    ],
                },
            ],
        }
    }

    // --- ADL → ReactFlow tests ---

    #[test]
    fn adl_to_reactflow_simple() {
        let doc = make_simple_adl();
        let graph = adl_to_reactflow(&doc);

        // 2 user nodes + 2 sentinels = 4
        assert_eq!(graph.nodes.len(), 4);
        assert_eq!(graph.nodes[0].id, "__start__");
        assert_eq!(graph.nodes[3].id, "__end__");

        // 3 edges
        assert_eq!(graph.edges.len(), 3);
        assert_eq!(graph.edges[0].source, "__start__");
        assert_eq!(graph.edges[0].target, "node_a");
    }

    #[test]
    fn adl_to_reactflow_conditional_edges() {
        let doc = make_conditional_adl();
        let graph = adl_to_reactflow(&doc);

        // 3 user nodes + 2 sentinels = 5
        assert_eq!(graph.nodes.len(), 5);

        // 1 static edge + 2 conditional edges = 3
        assert_eq!(graph.edges.len(), 3);

        // Find the conditional edges
        let cond_edges: Vec<_> = graph.edges.iter().filter(|e| e.label.is_some()).collect();
        assert_eq!(cond_edges.len(), 2);
        assert!(cond_edges.iter().any(|e| e.target == "path_a"));
        assert!(cond_edges.iter().any(|e| e.target == "path_b"));
    }

    #[test]
    fn adl_to_reactflow_node_positions() {
        let doc = make_simple_adl();
        let graph = adl_to_reactflow(&doc);

        // Each node should be at increasing y positions
        for i in 1..graph.nodes.len() {
            assert!(graph.nodes[i].position.y > graph.nodes[i - 1].position.y);
        }
    }

    #[test]
    fn adl_to_reactflow_node_data() {
        let doc = make_simple_adl();
        let graph = adl_to_reactflow(&doc);

        let node_a = graph.nodes.iter().find(|n| n.id == "node_a").unwrap();
        assert_eq!(node_a.data["label"], "node_a");
        assert_eq!(node_a.data["nodeType"], "passthrough");
    }

    // --- ReactFlow → ADL tests ---

    #[test]
    fn reactflow_to_adl_simple() {
        let graph = ReactFlowGraph {
            nodes: vec![
                ReactFlowNode {
                    id: "__start__".into(),
                    type_: "input".into(),
                    position: Position { x: 0.0, y: 0.0 },
                    data: json!({"label": "START"}),
                },
                ReactFlowNode {
                    id: "my_node".into(),
                    type_: "passthrough".into(),
                    position: Position { x: 0.0, y: 150.0 },
                    data: json!({"label": "my_node", "nodeType": "passthrough", "config": {}}),
                },
                ReactFlowNode {
                    id: "__end__".into(),
                    type_: "output".into(),
                    position: Position { x: 0.0, y: 300.0 },
                    data: json!({"label": "END"}),
                },
            ],
            edges: vec![
                ReactFlowEdge {
                    id: "e0".into(),
                    source: "__start__".into(),
                    target: "my_node".into(),
                    label: None,
                },
                ReactFlowEdge {
                    id: "e1".into(),
                    source: "my_node".into(),
                    target: "__end__".into(),
                    label: None,
                },
            ],
        };

        let doc = reactflow_to_adl(&graph);
        assert_eq!(doc.version, "1.0");
        assert_eq!(doc.nodes.len(), 1);
        assert_eq!(doc.nodes[0].id, "my_node");
        assert_eq!(doc.edges.len(), 2);
    }

    // --- Round-trip tests ---

    #[test]
    fn roundtrip_adl_to_reactflow_to_adl() {
        let original = make_simple_adl();
        let graph = adl_to_reactflow(&original);
        let result = reactflow_to_adl(&graph);

        // Same number of nodes
        assert_eq!(result.nodes.len(), original.nodes.len());

        // Same node IDs
        let original_ids: std::collections::HashSet<_> =
            original.nodes.iter().map(|n| &n.id).collect();
        let result_ids: std::collections::HashSet<_> =
            result.nodes.iter().map(|n| &n.id).collect();
        assert_eq!(original_ids, result_ids);
    }

    #[test]
    fn roundtrip_reactflow_to_adl_to_reactflow() {
        let original = ReactFlowGraph {
            nodes: vec![
                ReactFlowNode {
                    id: "__start__".into(),
                    type_: "input".into(),
                    position: Position { x: 250.0, y: 0.0 },
                    data: json!({"label": "START"}),
                },
                ReactFlowNode {
                    id: "processor".into(),
                    type_: "transform".into(),
                    position: Position { x: 250.0, y: 150.0 },
                    data: json!({"label": "processor", "nodeType": "transform", "config": {}}),
                },
                ReactFlowNode {
                    id: "__end__".into(),
                    type_: "output".into(),
                    position: Position { x: 250.0, y: 300.0 },
                    data: json!({"label": "END"}),
                },
            ],
            edges: vec![
                ReactFlowEdge {
                    id: "e0".into(),
                    source: "__start__".into(),
                    target: "processor".into(),
                    label: None,
                },
                ReactFlowEdge {
                    id: "e1".into(),
                    source: "processor".into(),
                    target: "__end__".into(),
                    label: None,
                },
            ],
        };

        let adl = reactflow_to_adl(&original);
        let result = adl_to_reactflow(&adl);

        // Same number of nodes (adl nodes + 2 sentinels)
        assert_eq!(result.nodes.len(), original.nodes.len());

        // The "processor" node should exist in both
        assert!(result.nodes.iter().any(|n| n.id == "processor"));
    }

    #[test]
    fn reactflow_graph_serde_roundtrip() {
        let graph = ReactFlowGraph {
            nodes: vec![ReactFlowNode {
                id: "n1".into(),
                type_: "default".into(),
                position: Position { x: 10.0, y: 20.0 },
                data: json!({"label": "Node 1"}),
            }],
            edges: vec![ReactFlowEdge {
                id: "e1".into(),
                source: "n1".into(),
                target: "n2".into(),
                label: Some("connection".into()),
            }],
        };

        let json = serde_json::to_string(&graph).unwrap();
        let parsed: ReactFlowGraph = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.nodes.len(), 1);
        assert_eq!(parsed.edges.len(), 1);
        assert_eq!(parsed.edges[0].label.as_deref(), Some("connection"));
    }

    #[test]
    fn empty_document_conversion() {
        let doc = AdlDocument {
            version: "1.0".into(),
            agent: None,
            channels: vec![],
            nodes: vec![],
            edges: vec![],
        };

        let graph = adl_to_reactflow(&doc);
        // Should still have start and end sentinels
        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(graph.edges.len(), 0);
    }
}
