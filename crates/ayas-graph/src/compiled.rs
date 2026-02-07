use std::collections::HashMap;

use crate::channel::Channel;
use crate::edge::ConditionalEdge;
use crate::node::NodeFn;

/// A compiled state graph ready for execution.
///
/// Created by `StateGraph::compile()`. Contains the validated graph
/// topology and all nodes/channels. The actual execution logic
/// (Pregel loop) will be implemented in Sprint 5.
pub struct CompiledStateGraph {
    pub(crate) nodes: HashMap<String, NodeFn>,
    pub(crate) adjacency: HashMap<String, Vec<String>>,
    pub(crate) conditional_edges: Vec<ConditionalEdge>,
    pub(crate) channels: HashMap<String, Box<dyn Channel>>,
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
        self.channels.contains_key(name)
    }
}
