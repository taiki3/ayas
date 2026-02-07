use std::collections::HashMap;
use std::sync::Arc;

use ayas_core::config::RunnableConfig;
use ayas_graph::prelude::NodeFn;
use serde_json::Value;

use crate::error::AdlError;

/// Factory function signature: (node_id, config) -> NodeFn.
pub type NodeFactory =
    Arc<dyn Fn(&str, &HashMap<String, Value>) -> Result<NodeFn, AdlError> + Send + Sync>;

/// Registry mapping node type strings to factory functions.
pub struct ComponentRegistry {
    node_factories: HashMap<String, NodeFactory>,
}

impl ComponentRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            node_factories: HashMap::new(),
        }
    }

    /// Create a registry pre-loaded with builtin node types.
    pub fn with_builtins() -> Self {
        let mut registry = Self::new();
        registry.register("passthrough", passthrough_factory());
        registry.register("transform", transform_factory());
        registry
    }

    /// Register a node factory for a given type name.
    pub fn register(&mut self, type_name: impl Into<String>, factory: NodeFactory) {
        self.node_factories.insert(type_name.into(), factory);
    }

    /// Check if a node type is registered.
    pub fn has_type(&self, type_name: &str) -> bool {
        self.node_factories.contains_key(type_name)
    }

    /// Create a node from the registry using the type name and config.
    pub fn create_node(
        &self,
        node_id: &str,
        node_type: &str,
        config: &HashMap<String, Value>,
    ) -> Result<NodeFn, AdlError> {
        let factory = self.node_factories.get(node_type).ok_or_else(|| {
            AdlError::UnknownNodeType {
                node_type: node_type.to_string(),
            }
        })?;
        factory(node_id, config)
    }
}

impl Default for ComponentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Built-in passthrough factory: returns the input state as-is.
fn passthrough_factory() -> NodeFactory {
    Arc::new(|node_id: &str, _config: &HashMap<String, Value>| {
        Ok(NodeFn::new(
            node_id.to_string(),
            |state: Value, _config: RunnableConfig| async move { Ok(state) },
        ))
    })
}

/// Built-in transform factory: copies specified fields from state to output.
///
/// Config:
/// - `"mapping"`: JSON object mapping output keys to input keys.
///   e.g. `{"result": "input_value"}` copies `state["input_value"]` to `output["result"]`.
///
/// If no mapping is provided, behaves like passthrough.
fn transform_factory() -> NodeFactory {
    Arc::new(|node_id: &str, config: &HashMap<String, Value>| {
        let mapping = config.get("mapping").cloned();
        Ok(NodeFn::new(
            node_id.to_string(),
            move |state: Value, _config: RunnableConfig| {
                let mapping = mapping.clone();
                async move {
                    let Some(Value::Object(map)) = mapping else {
                        return Ok(state);
                    };
                    let mut output = serde_json::Map::new();
                    for (out_key, in_key_val) in &map {
                        if let Some(in_key) = in_key_val.as_str()
                            && let Some(val) = state.get(in_key)
                        {
                            output.insert(out_key.clone(), val.clone());
                        }
                    }
                    Ok(Value::Object(output))
                }
            },
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn registry_register_and_has_type() {
        let mut registry = ComponentRegistry::new();
        assert!(!registry.has_type("passthrough"));
        registry.register("passthrough", passthrough_factory());
        assert!(registry.has_type("passthrough"));
    }

    #[test]
    fn registry_with_builtins() {
        let registry = ComponentRegistry::with_builtins();
        assert!(registry.has_type("passthrough"));
        assert!(registry.has_type("transform"));
        assert!(!registry.has_type("nonexistent"));
    }

    #[test]
    fn create_node_unknown_type_errors() {
        let registry = ComponentRegistry::new();
        let result = registry.create_node("n1", "unknown", &HashMap::new());
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(matches!(err, AdlError::UnknownNodeType { .. }));
    }

    #[tokio::test]
    async fn passthrough_node_returns_input() {
        let registry = ComponentRegistry::with_builtins();
        let node = registry
            .create_node("p1", "passthrough", &HashMap::new())
            .unwrap();
        assert_eq!(node.name(), "p1");
        let config = RunnableConfig::default();
        let result = node.invoke(json!({"x": 42}), &config).await.unwrap();
        assert_eq!(result, json!({"x": 42}));
    }

    #[tokio::test]
    async fn transform_node_with_mapping() {
        let registry = ComponentRegistry::with_builtins();
        let mut config = HashMap::new();
        config.insert(
            "mapping".to_string(),
            json!({"result": "input_value"}),
        );
        let node = registry.create_node("t1", "transform", &config).unwrap();
        let rt_config = RunnableConfig::default();
        let result = node
            .invoke(json!({"input_value": "hello", "other": "ignore"}), &rt_config)
            .await
            .unwrap();
        assert_eq!(result, json!({"result": "hello"}));
    }

    #[tokio::test]
    async fn transform_node_without_mapping_is_passthrough() {
        let registry = ComponentRegistry::with_builtins();
        let node = registry
            .create_node("t2", "transform", &HashMap::new())
            .unwrap();
        let config = RunnableConfig::default();
        let input = json!({"a": 1});
        let result = node.invoke(input.clone(), &config).await.unwrap();
        assert_eq!(result, input);
    }
}
