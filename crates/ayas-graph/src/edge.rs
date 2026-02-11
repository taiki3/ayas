use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;

/// A static edge connecting two nodes.
#[derive(Debug, Clone)]
pub struct Edge {
    pub from: String,
    pub to: String,
}

impl Edge {
    pub fn new(from: impl Into<String>, to: impl Into<String>) -> Self {
        Self {
            from: from.into(),
            to: to.into(),
        }
    }
}

type RouteFn = dyn Fn(&Value) -> String + Send + Sync;

/// A conditional edge that routes to different targets based on state.
///
/// The routing function inspects the current state and returns a key.
/// If a `path_map` is provided, the key is looked up in the map to
/// determine the actual target node. Otherwise the key itself is used
/// as the target node name.
pub struct ConditionalEdge {
    pub from: String,
    route_fn: Arc<RouteFn>,
    path_map: Option<HashMap<String, String>>,
}

impl ConditionalEdge {
    /// Create a new conditional edge.
    ///
    /// - `from`: source node name
    /// - `route_fn`: synchronous function that returns a routing key
    /// - `path_map`: optional mapping from routing key to target node name
    pub fn new<F>(
        from: impl Into<String>,
        route_fn: F,
        path_map: Option<HashMap<String, String>>,
    ) -> Self
    where
        F: Fn(&Value) -> String + Send + Sync + 'static,
    {
        Self {
            from: from.into(),
            route_fn: Arc::new(route_fn),
            path_map,
        }
    }

    /// Get the path map, if any.
    pub fn path_map(&self) -> Option<&HashMap<String, String>> {
        self.path_map.as_ref()
    }

    /// Resolve the target node name for the given state.
    pub fn resolve(&self, state: &Value) -> String {
        let key = (self.route_fn)(state);
        match &self.path_map {
            Some(map) => map.get(&key).cloned().unwrap_or(key),
            None => key,
        }
    }
}

type FanOutRouteFn = dyn Fn(&Value) -> Vec<String> + Send + Sync;

/// A conditional edge that can route to *multiple* target nodes (fan-out).
///
/// Unlike `ConditionalEdge` which resolves to a single target, this edge
/// resolves to zero or more targets that will be executed in parallel.
pub struct ConditionalFanOutEdge {
    pub from: String,
    route_fn: Arc<FanOutRouteFn>,
    target_map: HashMap<String, String>,
}

impl ConditionalFanOutEdge {
    /// Create a new fan-out conditional edge.
    ///
    /// - `from`: source node name
    /// - `route_fn`: function that returns a list of routing keys
    /// - `target_map`: mapping from routing key to target node name
    pub fn new<F>(
        from: impl Into<String>,
        route_fn: F,
        target_map: HashMap<String, String>,
    ) -> Self
    where
        F: Fn(&Value) -> Vec<String> + Send + Sync + 'static,
    {
        Self {
            from: from.into(),
            route_fn: Arc::new(route_fn),
            target_map,
        }
    }

    /// Get the target map.
    pub fn target_map(&self) -> &HashMap<String, String> {
        &self.target_map
    }

    /// Resolve the target node names for the given state.
    ///
    /// Returns all targets whose routing keys are present in the target map.
    /// Unknown keys are silently ignored.
    pub fn resolve(&self, state: &Value) -> Vec<String> {
        let keys = (self.route_fn)(state);
        keys.iter()
            .filter_map(|k| self.target_map.get(k).cloned())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn edge_creation() {
        let edge = Edge::new("a", "b");
        assert_eq!(edge.from, "a");
        assert_eq!(edge.to, "b");
    }

    #[test]
    fn conditional_edge_without_path_map() {
        let ce = ConditionalEdge::new(
            "router",
            |state: &Value| state["next"].as_str().unwrap_or("default").to_string(),
            None,
        );

        let target = ce.resolve(&json!({"next": "node_a"}));
        assert_eq!(target, "node_a");
    }

    #[test]
    fn conditional_edge_with_path_map() {
        let mut map = HashMap::new();
        map.insert("yes".to_string(), "approve_node".to_string());
        map.insert("no".to_string(), "reject_node".to_string());

        let ce = ConditionalEdge::new(
            "checker",
            |state: &Value| {
                if state["score"].as_f64().unwrap_or(0.0) > 0.5 {
                    "yes".to_string()
                } else {
                    "no".to_string()
                }
            },
            Some(map),
        );

        assert_eq!(ce.resolve(&json!({"score": 0.8})), "approve_node");
        assert_eq!(ce.resolve(&json!({"score": 0.2})), "reject_node");
    }

    #[test]
    fn conditional_edge_path_map_fallback() {
        let map = HashMap::new();
        let ce = ConditionalEdge::new(
            "router",
            |_state: &Value| "unknown_key".to_string(),
            Some(map),
        );

        // Key not in map → falls back to the key itself
        assert_eq!(ce.resolve(&json!({})), "unknown_key");
    }

    #[test]
    fn fan_out_edge_multiple_targets() {
        let mut map = HashMap::new();
        map.insert("research".to_string(), "researcher".to_string());
        map.insert("coding".to_string(), "coder".to_string());
        map.insert("review".to_string(), "reviewer".to_string());

        let ce = ConditionalFanOutEdge::new(
            "router",
            |_state: &Value| vec!["research".to_string(), "coding".to_string()],
            map,
        );

        let targets = ce.resolve(&json!({}));
        assert_eq!(targets.len(), 2);
        assert!(targets.contains(&"researcher".to_string()));
        assert!(targets.contains(&"coder".to_string()));
    }

    #[test]
    fn fan_out_edge_single_target() {
        let mut map = HashMap::new();
        map.insert("only".to_string(), "single_node".to_string());

        let ce = ConditionalFanOutEdge::new(
            "router",
            |_state: &Value| vec!["only".to_string()],
            map,
        );

        let targets = ce.resolve(&json!({}));
        assert_eq!(targets, vec!["single_node"]);
    }

    #[test]
    fn fan_out_edge_unknown_keys_ignored() {
        let mut map = HashMap::new();
        map.insert("known".to_string(), "target".to_string());

        let ce = ConditionalFanOutEdge::new(
            "router",
            |_state: &Value| vec!["known".to_string(), "unknown".to_string()],
            map,
        );

        let targets = ce.resolve(&json!({}));
        assert_eq!(targets, vec!["target"]);
    }

    #[test]
    fn fan_out_edge_empty_result() {
        let map = HashMap::new();
        let ce = ConditionalFanOutEdge::new(
            "router",
            |_state: &Value| vec![],
            map,
        );

        let targets = ce.resolve(&json!({}));
        assert!(targets.is_empty());
    }
}
