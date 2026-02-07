use std::collections::HashMap;

use serde::Deserialize;
use serde_json::Value;

/// Top-level ADL document.
#[derive(Debug, Deserialize)]
pub struct AdlDocument {
    /// Schema version (currently "1.0").
    pub version: String,
    /// Agent metadata.
    #[serde(default)]
    pub agent: Option<AgentMetadata>,
    /// Channel definitions.
    #[serde(default)]
    pub channels: Vec<AdlChannel>,
    /// Node definitions.
    #[serde(default)]
    pub nodes: Vec<AdlNode>,
    /// Edge definitions.
    #[serde(default)]
    pub edges: Vec<AdlEdge>,
}

/// Agent metadata.
#[derive(Debug, Deserialize)]
pub struct AgentMetadata {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// A channel definition in the ADL document.
#[derive(Debug, Deserialize)]
pub struct AdlChannel {
    /// Channel name (corresponds to a state key).
    pub name: String,
    /// Channel type.
    #[serde(rename = "type")]
    pub channel_type: AdlChannelType,
    /// Optional JSON schema for the channel value.
    #[serde(default)]
    pub schema: Option<Value>,
    /// Default value for LastValue channels.
    #[serde(default)]
    pub default: Option<Value>,
}

/// Channel type enum.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdlChannelType {
    LastValue,
    Append,
    /// Topic maps to Append behavior.
    Topic,
}

/// A node definition in the ADL document.
#[derive(Debug, Deserialize)]
pub struct AdlNode {
    /// Unique node identifier.
    pub id: String,
    /// Node type (must be registered in the ComponentRegistry).
    #[serde(rename = "type")]
    pub node_type: String,
    /// Node-specific configuration.
    #[serde(default)]
    pub config: HashMap<String, Value>,
}

/// An edge definition in the ADL document.
#[derive(Debug, Deserialize)]
pub struct AdlEdge {
    /// Source node ID (or "__start__" / "START").
    pub from: String,
    /// Target node ID (or "__end__" / "END") — used for static edges.
    #[serde(default)]
    pub to: Option<String>,
    /// Edge type (defaults to Static).
    #[serde(rename = "type", default = "default_edge_type")]
    pub edge_type: AdlEdgeType,
    /// Conditions for conditional edges.
    #[serde(default)]
    pub conditions: Vec<AdlCondition>,
}

fn default_edge_type() -> AdlEdgeType {
    AdlEdgeType::Static
}

/// Edge type enum.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdlEdgeType {
    Static,
    Conditional,
}

/// A condition in a conditional edge.
#[derive(Debug, Deserialize)]
pub struct AdlCondition {
    /// Rhai expression to evaluate against the state.
    /// "default" is treated as always-true (fallback).
    pub expression: String,
    /// Target node ID if the expression evaluates to true.
    pub to: String,
}

/// Normalize sentinel names: accept both `__start__`/`START` and `__end__`/`END`.
pub fn normalize_sentinel(name: &str) -> String {
    match name {
        "START" | "__start__" => "__start__".to_string(),
        "END" | "__end__" => "__end__".to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn deserialize_minimal_yaml() {
        let yaml = r#"
version: "1.0"
nodes: []
edges: []
"#;
        let doc: AdlDocument = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(doc.version, "1.0");
        assert!(doc.nodes.is_empty());
        assert!(doc.edges.is_empty());
        assert!(doc.agent.is_none());
        assert!(doc.channels.is_empty());
    }

    #[test]
    fn deserialize_full_yaml() {
        let yaml = r#"
version: "1.0"
agent:
  name: "test-agent"
  description: "A test agent"
channels:
  - name: messages
    type: append
  - name: count
    type: last_value
    default: 0
nodes:
  - id: greeter
    type: passthrough
  - id: counter
    type: transform
    config:
      output_key: "count"
edges:
  - from: __start__
    to: greeter
    type: static
  - from: greeter
    to: counter
  - from: counter
    to: __end__
"#;
        let doc: AdlDocument = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(doc.version, "1.0");
        assert_eq!(doc.agent.as_ref().unwrap().name, "test-agent");
        assert_eq!(doc.channels.len(), 2);
        assert_eq!(doc.channels[0].channel_type, AdlChannelType::Append);
        assert_eq!(doc.channels[1].channel_type, AdlChannelType::LastValue);
        assert_eq!(doc.channels[1].default, Some(json!(0)));
        assert_eq!(doc.nodes.len(), 2);
        assert_eq!(doc.nodes[0].node_type, "passthrough");
        assert_eq!(doc.edges.len(), 3);
    }

    #[test]
    fn deserialize_conditional_edge_yaml() {
        let yaml = r#"
version: "1.0"
nodes:
  - id: router
    type: passthrough
  - id: path_a
    type: passthrough
  - id: path_b
    type: passthrough
edges:
  - from: router
    type: conditional
    conditions:
      - expression: 'state.choice == "a"'
        to: path_a
      - expression: default
        to: path_b
"#;
        let doc: AdlDocument = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(doc.edges.len(), 1);
        let edge = &doc.edges[0];
        assert_eq!(edge.edge_type, AdlEdgeType::Conditional);
        assert_eq!(edge.conditions.len(), 2);
        assert_eq!(edge.conditions[0].to, "path_a");
        assert_eq!(edge.conditions[1].expression, "default");
    }

    #[test]
    fn deserialize_from_json() {
        let json_str = r#"{
            "version": "1.0",
            "nodes": [
                { "id": "a", "type": "passthrough" }
            ],
            "edges": [
                { "from": "__start__", "to": "a" },
                { "from": "a", "to": "__end__" }
            ]
        }"#;
        let doc: AdlDocument = serde_json::from_str(json_str).unwrap();
        assert_eq!(doc.version, "1.0");
        assert_eq!(doc.nodes.len(), 1);
        assert_eq!(doc.edges.len(), 2);
    }

    #[test]
    fn channel_type_topic_maps_to_topic() {
        let yaml = r#"
name: events
type: topic
"#;
        let ch: AdlChannel = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(ch.channel_type, AdlChannelType::Topic);
    }

    #[test]
    fn normalize_sentinels() {
        assert_eq!(normalize_sentinel("START"), "__start__");
        assert_eq!(normalize_sentinel("__start__"), "__start__");
        assert_eq!(normalize_sentinel("END"), "__end__");
        assert_eq!(normalize_sentinel("__end__"), "__end__");
        assert_eq!(normalize_sentinel("my_node"), "my_node");
    }
}
