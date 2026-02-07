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
