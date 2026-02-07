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
}
