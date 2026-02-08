use std::collections::HashMap;

use async_trait::async_trait;
use serde_json::Value;

use ayas_core::config::RunnableConfig;
use ayas_core::error::{GraphError, Result};
use ayas_core::runnable::Runnable;

use crate::channel::{Channel, ChannelSpec};
use crate::constants::END;
use crate::edge::ConditionalEdge;
use crate::node::NodeFn;

/// Information about a single step in graph execution.
#[derive(Debug, Clone)]
pub struct StepInfo {
    pub step_number: usize,
    pub node_name: String,
    pub state_after: Value,
}

/// A compiled state graph ready for execution.
///
/// Created by `StateGraph::compile()`. Implements `Runnable<Input=Value, Output=Value>`
/// to execute the graph as a Pregel-style loop.
pub struct CompiledStateGraph {
    pub(crate) nodes: HashMap<String, NodeFn>,
    pub(crate) adjacency: HashMap<String, Vec<String>>,
    pub(crate) conditional_edges: Vec<ConditionalEdge>,
    pub(crate) channel_specs: HashMap<String, ChannelSpec>,
    pub(crate) entry_point: String,
    pub(crate) finish_points: Vec<String>,
}

impl CompiledStateGraph {
    /// Get the names of all nodes in the graph.
    pub fn node_names(&self) -> Vec<&str> {
        self.nodes.keys().map(|s| s.as_str()).collect()
    }

    /// Get the static edges from a given node.
    pub fn edges_from(&self, node: &str) -> &[String] {
        self.adjacency
            .get(node)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Get the entry point node name.
    pub fn entry_point(&self) -> &str {
        &self.entry_point
    }

    /// Get the finish point node names.
    pub fn finish_points(&self) -> &[String] {
        &self.finish_points
    }

    /// Get a node by name.
    pub fn node(&self, name: &str) -> Option<&NodeFn> {
        self.nodes.get(name)
    }

    /// Get the conditional edges.
    pub fn conditional_edges(&self) -> &[ConditionalEdge] {
        &self.conditional_edges
    }

    /// Check if a channel exists.
    pub fn has_channel(&self, name: &str) -> bool {
        self.channel_specs.contains_key(name)
    }

    /// Build a state Value from all channels.
    fn build_state(channels: &HashMap<String, Box<dyn Channel>>) -> Value {
        let mut map = serde_json::Map::new();
        for (key, ch) in channels {
            map.insert(key.clone(), ch.get().clone());
        }
        Value::Object(map)
    }

    /// Update channels from a node's partial output.
    fn update_channels(
        channels: &mut HashMap<String, Box<dyn Channel>>,
        output: &Value,
    ) -> Result<()> {
        if let Value::Object(map) = output {
            for (key, value) in map {
                if let Some(ch) = channels.get_mut(key) {
                    ch.update(vec![value.clone()])?;
                }
            }
        }
        Ok(())
    }

    /// Determine the next nodes to execute after a given node.
    fn next_nodes(&self, current: &str, state: &Value) -> Vec<String> {
        // Check conditional edges first (they take priority)
        for ce in &self.conditional_edges {
            if ce.from == current {
                let target = ce.resolve(state);
                if target == END {
                    return Vec::new();
                }
                return vec![target];
            }
        }

        // Then check static edges
        if let Some(targets) = self.adjacency.get(current) {
            return targets
                .iter()
                .filter(|t| *t != END)
                .cloned()
                .collect();
        }

        Vec::new()
    }

    /// Execute the graph like `invoke`, but call `observer` after each node execution.
    pub async fn invoke_with_observer<F>(
        &self,
        input: Value,
        config: &RunnableConfig,
        observer: F,
    ) -> Result<Value>
    where
        F: Fn(StepInfo) + Send,
    {
        // Create fresh channels for this invocation
        let mut channels: HashMap<String, Box<dyn Channel>> = self
            .channel_specs
            .iter()
            .map(|(k, spec)| (k.clone(), spec.create()))
            .collect();

        // Initialize channels from input
        if let Value::Object(map) = &input {
            for (key, value) in map {
                if let Some(ch) = channels.get_mut(key) {
                    ch.update(vec![value.clone()])?;
                }
            }
        }

        // Execute graph: super-step based loop
        let mut current_nodes = vec![self.entry_point.clone()];
        let mut step = 0;
        let mut node_step = 0;

        while !current_nodes.is_empty() {
            // Check recursion limit
            if step >= config.recursion_limit {
                return Err(
                    GraphError::RecursionLimit {
                        limit: config.recursion_limit,
                    }
                    .into(),
                );
            }

            let mut all_next: Vec<String> = Vec::new();

            for node_name in &current_nodes {
                // Build current state from channels
                let state = Self::build_state(&channels);

                let node = self.nodes.get(node_name).ok_or_else(|| {
                    GraphError::InvalidGraph(format!(
                        "Node '{node_name}' not found during execution"
                    ))
                })?;

                // Execute node
                let output = node.invoke(state.clone(), config).await.map_err(|e| {
                    GraphError::NodeExecution {
                        node: node_name.clone(),
                        source: Box::new(e),
                    }
                })?;

                // Update channels from partial output
                Self::update_channels(&mut channels, &output)?;

                // Build state after and call observer
                let state_after = Self::build_state(&channels);
                observer(StepInfo {
                    step_number: node_step,
                    node_name: node_name.clone(),
                    state_after: state_after.clone(),
                });

                // Determine next nodes
                let next = self.next_nodes(node_name, &state_after);
                all_next.extend(next);
                node_step += 1;
            }

            // Deduplicate
            all_next.sort();
            all_next.dedup();

            current_nodes = all_next;
            step += 1;
        }

        Ok(Self::build_state(&channels))
    }
}

#[async_trait]
impl Runnable for CompiledStateGraph {
    type Input = Value;
    type Output = Value;

    async fn invoke(&self, input: Self::Input, config: &RunnableConfig) -> Result<Self::Output> {
        // Create fresh channels for this invocation
        let mut channels: HashMap<String, Box<dyn Channel>> = self
            .channel_specs
            .iter()
            .map(|(k, spec)| (k.clone(), spec.create()))
            .collect();

        // Initialize channels from input
        if let Value::Object(map) = &input {
            for (key, value) in map {
                if let Some(ch) = channels.get_mut(key) {
                    ch.update(vec![value.clone()])?;
                }
            }
        }

        // Execute graph: super-step based loop
        let mut current_nodes = vec![self.entry_point.clone()];
        let mut step = 0;

        while !current_nodes.is_empty() {
            // Check recursion limit
            if step >= config.recursion_limit {
                return Err(
                    GraphError::RecursionLimit {
                        limit: config.recursion_limit,
                    }
                    .into(),
                );
            }

            let mut all_next: Vec<String> = Vec::new();

            for node_name in &current_nodes {
                // Build current state from channels
                let state = Self::build_state(&channels);

                let node = self.nodes.get(node_name).ok_or_else(|| {
                    GraphError::InvalidGraph(format!(
                        "Node '{node_name}' not found during execution"
                    ))
                })?;

                // Execute node
                let output = node.invoke(state.clone(), config).await.map_err(|e| {
                    GraphError::NodeExecution {
                        node: node_name.clone(),
                        source: Box::new(e),
                    }
                })?;

                // Update channels from partial output
                Self::update_channels(&mut channels, &output)?;

                // Determine next nodes
                let state_after = Self::build_state(&channels);
                let next = self.next_nodes(node_name, &state_after);
                all_next.extend(next);
            }

            // Deduplicate
            all_next.sort();
            all_next.dedup();

            current_nodes = all_next;
            step += 1;
        }

        Ok(Self::build_state(&channels))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use serde_json::json;

    use crate::edge::ConditionalEdge;
    use crate::node::NodeFn;
    use crate::state_graph::StateGraph;

    /// Helper: build a 3-node linear graph: a → b → c
    /// Channel "count" (LastValue, default 0); each node increments count by 1.
    fn build_linear_graph() -> CompiledStateGraph {
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

    fn collect_observer() -> (Arc<Mutex<Vec<StepInfo>>>, impl Fn(StepInfo) + Send + Clone) {
        let steps = Arc::new(Mutex::new(Vec::new()));
        let steps_clone = steps.clone();
        let observer = move |info: StepInfo| {
            steps_clone.lock().unwrap().push(info);
        };
        (steps, observer)
    }

    #[tokio::test]
    async fn observer_called_for_each_step() {
        let graph = build_linear_graph();
        let config = default_config();
        let (steps, observer) = collect_observer();

        graph
            .invoke_with_observer(json!({}), &config, observer)
            .await
            .unwrap();

        assert_eq!(steps.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn observer_receives_correct_node_names() {
        let graph = build_linear_graph();
        let config = default_config();
        let (steps, observer) = collect_observer();

        graph
            .invoke_with_observer(json!({}), &config, observer)
            .await
            .unwrap();

        let steps = steps.lock().unwrap();
        assert_eq!(steps[0].node_name, "a");
        assert_eq!(steps[1].node_name, "b");
        assert_eq!(steps[2].node_name, "c");
    }

    #[tokio::test]
    async fn observer_receives_correct_step_numbers() {
        let graph = build_linear_graph();
        let config = default_config();
        let (steps, observer) = collect_observer();

        graph
            .invoke_with_observer(json!({}), &config, observer)
            .await
            .unwrap();

        let steps = steps.lock().unwrap();
        assert_eq!(steps[0].step_number, 0);
        assert_eq!(steps[1].step_number, 1);
        assert_eq!(steps[2].step_number, 2);
    }

    #[tokio::test]
    async fn observer_receives_state_after() {
        let graph = build_linear_graph();
        let config = default_config();
        let (steps, observer) = collect_observer();

        graph
            .invoke_with_observer(json!({}), &config, observer)
            .await
            .unwrap();

        let steps = steps.lock().unwrap();
        // After node "a": count = 1
        assert_eq!(steps[0].state_after["count"], json!(1));
        // After node "b": count = 2
        assert_eq!(steps[1].state_after["count"], json!(2));
        // After node "c": count = 3
        assert_eq!(steps[2].state_after["count"], json!(3));
    }

    #[tokio::test]
    async fn observer_result_matches_invoke() {
        let graph = build_linear_graph();
        let config = default_config();
        let (_, observer) = collect_observer();

        let result_observer = graph
            .invoke_with_observer(json!({}), &config, observer)
            .await
            .unwrap();

        let result_invoke = graph.invoke(json!({}), &config).await.unwrap();

        assert_eq!(result_observer, result_invoke);
    }

    #[tokio::test]
    async fn observer_with_conditional_edges() {
        // Graph: a → (conditional) → b or c → END
        // Route to "b" when count == 1, otherwise "c"
        let mut g = StateGraph::new();
        g.add_last_value_channel("count", json!(0));

        g.add_node(NodeFn::new("a", |_state: Value, _cfg| async move {
            Ok(json!({"count": 1}))
        }))
        .unwrap();
        g.add_node(NodeFn::new("b", |state: Value, _cfg| async move {
            let c = state["count"].as_i64().unwrap_or(0);
            Ok(json!({"count": c + 10}))
        }))
        .unwrap();
        g.add_node(NodeFn::new("c", |state: Value, _cfg| async move {
            let c = state["count"].as_i64().unwrap_or(0);
            Ok(json!({"count": c + 100}))
        }))
        .unwrap();

        g.set_entry_point("a");

        let mut path_map = HashMap::new();
        path_map.insert("go_b".to_string(), "b".to_string());
        path_map.insert("go_c".to_string(), "c".to_string());
        g.add_conditional_edges(ConditionalEdge::new(
            "a",
            |state: &Value| {
                if state["count"] == json!(1) {
                    "go_b".to_string()
                } else {
                    "go_c".to_string()
                }
            },
            Some(path_map),
        ));

        g.set_finish_point("b");
        g.set_finish_point("c");

        let graph = g.compile().unwrap();
        let config = default_config();
        let (steps, observer) = collect_observer();

        graph
            .invoke_with_observer(json!({}), &config, observer)
            .await
            .unwrap();

        let steps = steps.lock().unwrap();
        // Only "a" and "b" should be observed (not "c")
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].node_name, "a");
        assert_eq!(steps[1].node_name, "b");
        assert_eq!(steps[1].state_after["count"], json!(11));
    }

    #[tokio::test]
    async fn observer_recursion_limit() {
        // Create a cycle: a → b → a (via conditional edge)
        let mut g = StateGraph::new();
        g.add_last_value_channel("count", json!(0));

        g.add_node(NodeFn::new("a", |state: Value, _cfg| async move {
            let c = state["count"].as_i64().unwrap_or(0);
            Ok(json!({"count": c + 1}))
        }))
        .unwrap();
        g.add_node(NodeFn::new("b", |state: Value, _cfg| async move {
            Ok(state)
        }))
        .unwrap();

        g.set_entry_point("a");
        g.add_edge("a", "b");
        // b always routes back to a
        g.add_conditional_edges(ConditionalEdge::new(
            "b",
            |_state: &Value| "a".to_string(),
            None,
        ));

        let graph = g.compile().unwrap();
        let mut config = default_config();
        config.recursion_limit = 3;

        let (_, observer) = collect_observer();
        let result = graph
            .invoke_with_observer(json!({}), &config, observer)
            .await;

        assert!(result.is_err());
        assert!(result.err().unwrap().to_string().contains("Recursion limit"));
    }

    #[tokio::test]
    async fn observer_empty_input() {
        let graph = build_linear_graph();
        let config = default_config();
        let (steps, observer) = collect_observer();

        let result = graph
            .invoke_with_observer(json!({}), &config, observer)
            .await;

        assert!(result.is_ok());
        let steps = steps.lock().unwrap();
        assert_eq!(steps.len(), 3);
        // count starts at 0 (default), node a sets it to 1
        assert_eq!(steps[0].state_after["count"], json!(1));
    }
}
