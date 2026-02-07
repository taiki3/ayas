use std::collections::HashMap;

use ayas_graph::prelude::{ChannelSpec, CompiledStateGraph, ConditionalEdge, StateGraph};
use serde_json::Value;

use crate::error::AdlError;
use crate::expression;
use crate::registry::ComponentRegistry;
use crate::types::{AdlChannelType, AdlCondition, AdlDocument, AdlEdgeType, normalize_sentinel};
use crate::validation;

/// Builder that converts ADL documents into compiled state graphs.
pub struct AdlBuilder {
    registry: ComponentRegistry,
}

impl AdlBuilder {
    /// Create a builder with the given registry.
    pub fn new(registry: ComponentRegistry) -> Self {
        Self { registry }
    }

    /// Create a builder with builtin node types pre-registered.
    pub fn with_defaults() -> Self {
        Self::new(ComponentRegistry::with_builtins())
    }

    /// Build a `CompiledStateGraph` from a YAML string.
    pub fn build_from_yaml(&self, yaml_str: &str) -> Result<CompiledStateGraph, AdlError> {
        let doc: AdlDocument = serde_yaml::from_str(yaml_str)?;
        self.build(doc)
    }

    /// Build a `CompiledStateGraph` from a JSON string.
    pub fn build_from_json(&self, json_str: &str) -> Result<CompiledStateGraph, AdlError> {
        let doc: AdlDocument =
            serde_json::from_str(json_str).map_err(|e| AdlError::Parse(e.to_string()))?;
        self.build(doc)
    }

    /// Build a `CompiledStateGraph` from a parsed ADL document.
    fn build(&self, doc: AdlDocument) -> Result<CompiledStateGraph, AdlError> {
        // Step 1: Validate
        validation::validate_document(&doc, &self.registry)?;

        // Step 2: Build state graph
        let mut graph = StateGraph::new();

        // Step 3: Add channels
        for ch in &doc.channels {
            let spec = match ch.channel_type {
                AdlChannelType::LastValue => ChannelSpec::LastValue {
                    default: ch.default.clone().unwrap_or(Value::Null),
                },
                AdlChannelType::Append | AdlChannelType::Topic => ChannelSpec::Append,
            };
            graph.add_channel(&ch.name, spec);
        }

        // Step 4: Add nodes
        for node_def in &doc.nodes {
            let node = self
                .registry
                .create_node(&node_def.id, &node_def.node_type, &node_def.config)?;
            graph
                .add_node(node)
                .map_err(|e| AdlError::Validation(e.to_string()))?;
        }

        // Step 5: Process edges
        for edge_def in &doc.edges {
            let from = normalize_sentinel(&edge_def.from);

            match edge_def.edge_type {
                AdlEdgeType::Static => {
                    let to = normalize_sentinel(edge_def.to.as_deref().unwrap_or(""));

                    if from == "__start__" {
                        graph.set_entry_point(&to);
                    } else if to == "__end__" {
                        graph.set_finish_point(&from);
                    } else {
                        graph.add_edge(&from, &to);
                    }
                }
                AdlEdgeType::Conditional => {
                    let conditions: Vec<AdlCondition> = edge_def
                        .conditions
                        .iter()
                        .map(|c| AdlCondition {
                            expression: c.expression.clone(),
                            to: normalize_sentinel(&c.to),
                        })
                        .collect();

                    // Build path_map for all non-default conditions + default
                    let mut path_map = HashMap::new();
                    let expressions: Vec<(String, String)> = conditions
                        .iter()
                        .map(|c| (c.expression.clone(), c.to.clone()))
                        .collect();

                    for cond in &conditions {
                        path_map.insert(cond.to.clone(), cond.to.clone());
                    }
                    // Also map __end__ as itself
                    path_map.insert("__end__".to_string(), "__end__".to_string());

                    let from_clone = from.clone();
                    let ce = ConditionalEdge::new(
                        &from,
                        move |state: &Value| {
                            for (expr, target) in &expressions {
                                match expression::evaluate(expr, state) {
                                    Ok(true) => return target.clone(),
                                    Ok(false) => continue,
                                    Err(_) => continue,
                                }
                            }
                            // If no condition matches, route to __end__
                            "__end__".to_string()
                        },
                        Some(path_map),
                    );
                    graph.add_conditional_edges(ce);

                    // Check if any condition routes to __end__ — if so, set finish point
                    for cond in &conditions {
                        if cond.to == "__end__" {
                            // The finish point is the node with the conditional edge
                            // (it will route to END via the conditional edge)
                            // We don't set_finish_point here because ConditionalEdge handles it
                            let _ = &from_clone;
                        }
                    }
                }
            }
        }

        // Step 6: Compile
        graph
            .compile()
            .map_err(|e| AdlError::Validation(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ayas_core::config::RunnableConfig;
    use ayas_core::runnable::Runnable;
    use serde_json::json;

    #[test]
    fn build_linear_graph_from_yaml() {
        let yaml = r#"
version: "1.0"
channels:
  - name: value
    type: last_value
    default: "initial"
nodes:
  - id: node_a
    type: passthrough
  - id: node_b
    type: passthrough
edges:
  - from: __start__
    to: node_a
  - from: node_a
    to: node_b
  - from: node_b
    to: __end__
"#;
        let builder = AdlBuilder::with_defaults();
        let compiled = builder.build_from_yaml(yaml).unwrap();
        assert_eq!(compiled.entry_point(), "node_a");
        assert!(compiled.node_names().contains(&"node_a"));
        assert!(compiled.node_names().contains(&"node_b"));
    }

    #[test]
    fn build_from_json() {
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
        let builder = AdlBuilder::with_defaults();
        let compiled = builder.build_from_json(json_str).unwrap();
        assert_eq!(compiled.entry_point(), "a");
    }

    #[tokio::test]
    async fn execute_linear_graph() {
        let yaml = r#"
version: "1.0"
channels:
  - name: value
    type: last_value
    default: ""
nodes:
  - id: a
    type: passthrough
  - id: b
    type: passthrough
edges:
  - from: __start__
    to: a
  - from: a
    to: b
  - from: b
    to: __end__
"#;
        let builder = AdlBuilder::with_defaults();
        let compiled = builder.build_from_yaml(yaml).unwrap();
        let config = RunnableConfig::default();
        let result = compiled
            .invoke(json!({"value": "hello"}), &config)
            .await
            .unwrap();
        assert_eq!(result["value"], "hello");
    }

    #[tokio::test]
    async fn execute_conditional_graph() {
        let yaml = r#"
version: "1.0"
channels:
  - name: choice
    type: last_value
    default: ""
  - name: result
    type: last_value
    default: ""
nodes:
  - id: router
    type: passthrough
  - id: path_a
    type: transform
    config:
      mapping:
        result: choice
  - id: path_b
    type: transform
    config:
      mapping:
        result: choice
edges:
  - from: __start__
    to: router
  - from: router
    type: conditional
    conditions:
      - expression: 'state.choice == "a"'
        to: path_a
      - expression: default
        to: path_b
  - from: path_a
    to: __end__
  - from: path_b
    to: __end__
"#;
        let builder = AdlBuilder::with_defaults();
        let compiled = builder.build_from_yaml(yaml).unwrap();
        let config = RunnableConfig::default();

        // Route to path_a
        let result = compiled
            .invoke(json!({"choice": "a", "result": ""}), &config)
            .await
            .unwrap();
        assert_eq!(result["result"], "a");

        // Route to path_b (default)
        let result = compiled
            .invoke(json!({"choice": "b", "result": ""}), &config)
            .await
            .unwrap();
        assert_eq!(result["result"], "b");
    }

    #[test]
    fn build_invalid_yaml_fails() {
        let builder = AdlBuilder::with_defaults();
        let result = builder.build_from_yaml("not: valid: yaml: [");
        assert!(result.is_err());
    }

    #[test]
    fn build_invalid_json_fails() {
        let builder = AdlBuilder::with_defaults();
        let result = builder.build_from_json("{invalid json}");
        assert!(result.is_err());
    }

    #[test]
    fn build_with_start_alias() {
        let yaml = r#"
version: "1.0"
nodes:
  - id: a
    type: passthrough
edges:
  - from: START
    to: a
  - from: a
    to: END
"#;
        let builder = AdlBuilder::with_defaults();
        let compiled = builder.build_from_yaml(yaml).unwrap();
        assert_eq!(compiled.entry_point(), "a");
    }

    #[tokio::test]
    async fn e2e_transform_graph() {
        let yaml = r#"
version: "1.0"
agent:
  name: "transform-agent"
  description: "Tests transform node"
channels:
  - name: input_value
    type: last_value
    default: ""
  - name: output
    type: last_value
    default: ""
nodes:
  - id: transformer
    type: transform
    config:
      mapping:
        output: input_value
edges:
  - from: __start__
    to: transformer
  - from: transformer
    to: __end__
"#;
        let builder = AdlBuilder::with_defaults();
        let compiled = builder.build_from_yaml(yaml).unwrap();
        let config = RunnableConfig::default();
        let result = compiled
            .invoke(json!({"input_value": "transformed!", "output": ""}), &config)
            .await
            .unwrap();
        assert_eq!(result["output"], "transformed!");
    }
}
