use std::collections::{HashMap, HashSet, VecDeque};

use ayas_core::error::{GraphError, Result};
use serde_json::Value;

use crate::channel::{AggregateOp, ChannelSpec};
use crate::compiled::CompiledStateGraph;
use crate::constants::{END, START};
use crate::edge::{ConditionalEdge, Edge};
use crate::node::NodeFn;

/// Builder for constructing a state graph.
///
/// Use `add_node`, `add_edge`, `add_conditional_edges`, etc. to define
/// the graph topology, then call `compile()` to validate and produce
/// a `CompiledStateGraph`.
pub struct StateGraph {
    channel_specs: HashMap<String, ChannelSpec>,
    nodes: HashMap<String, NodeFn>,
    edges: Vec<Edge>,
    conditional_edges: Vec<ConditionalEdge>,
    entry_point: Option<String>,
    finish_points: Vec<String>,
}

impl StateGraph {
    /// Create a new, empty state graph.
    pub fn new() -> Self {
        Self {
            channel_specs: HashMap::new(),
            nodes: HashMap::new(),
            edges: Vec::new(),
            conditional_edges: Vec::new(),
            entry_point: None,
            finish_points: Vec::new(),
        }
    }

    /// Add a channel spec for a state key.
    pub fn add_channel(
        &mut self,
        name: impl Into<String>,
        spec: ChannelSpec,
    ) -> &mut Self {
        self.channel_specs.insert(name.into(), spec);
        self
    }

    /// Convenience: add a `LastValue` channel with the given default.
    pub fn add_last_value_channel(
        &mut self,
        name: impl Into<String>,
        default: Value,
    ) -> &mut Self {
        self.add_channel(name, ChannelSpec::LastValue { default })
    }

    /// Convenience: add an `AppendChannel`.
    pub fn add_append_channel(&mut self, name: impl Into<String>) -> &mut Self {
        self.add_channel(name, ChannelSpec::Append)
    }

    /// Convenience: add a `BinaryOperatorAggregate` channel.
    pub fn add_binary_operator_channel(
        &mut self,
        name: impl Into<String>,
        default: Value,
        op: AggregateOp,
    ) -> &mut Self {
        self.add_channel(name, ChannelSpec::BinaryOperator { default, op })
    }

    /// Convenience: add an `EphemeralValue` channel.
    pub fn add_ephemeral_channel(&mut self, name: impl Into<String>) -> &mut Self {
        self.add_channel(name, ChannelSpec::Ephemeral)
    }

    /// Convenience: add a `TopicChannel`.
    pub fn add_topic_channel(
        &mut self,
        name: impl Into<String>,
        accumulate: bool,
    ) -> &mut Self {
        self.add_channel(name, ChannelSpec::Topic { accumulate })
    }

    /// Add a node to the graph.
    ///
    /// Returns an error if a node with the same name already exists
    /// or if the name is a reserved sentinel (`__start__` / `__end__`).
    pub fn add_node(&mut self, node: NodeFn) -> Result<&mut Self> {
        let name = node.name().to_string();

        if name == START || name == END {
            return Err(GraphError::InvalidGraph(format!(
                "Cannot add node with reserved name '{name}'"
            ))
            .into());
        }

        if self.nodes.contains_key(&name) {
            return Err(
                GraphError::InvalidGraph(format!("Duplicate node name: '{name}'")).into(),
            );
        }

        self.nodes.insert(name, node);
        Ok(self)
    }

    /// Add a static edge between two nodes.
    ///
    /// Both `from` and `to` can be node names or sentinels (`START` / `END`).
    pub fn add_edge(
        &mut self,
        from: impl Into<String>,
        to: impl Into<String>,
    ) -> &mut Self {
        self.edges.push(Edge::new(from, to));
        self
    }

    /// Add a conditional edge from a source node.
    pub fn add_conditional_edges(&mut self, edge: ConditionalEdge) -> &mut Self {
        self.conditional_edges.push(edge);
        self
    }

    /// Set the entry point (first node to execute after `START`).
    pub fn set_entry_point(&mut self, node: impl Into<String>) -> &mut Self {
        self.entry_point = Some(node.into());
        self
    }

    /// Add a finish point (node that leads to `END`).
    pub fn set_finish_point(&mut self, node: impl Into<String>) -> &mut Self {
        self.finish_points.push(node.into());
        self
    }

    /// Validate the graph and produce a `CompiledStateGraph`.
    pub fn compile(self) -> Result<CompiledStateGraph> {
        self.validate()?;

        let entry_point = self.entry_point.unwrap(); // safe: validate checks

        // Build adjacency list from static edges
        let mut adjacency: HashMap<String, Vec<String>> = HashMap::new();
        for edge in &self.edges {
            adjacency
                .entry(edge.from.clone())
                .or_default()
                .push(edge.to.clone());
        }

        // Add entry point edge: START -> entry_point
        adjacency
            .entry(START.to_string())
            .or_default()
            .push(entry_point.clone());

        // Add finish point edges: finish_point -> END
        for fp in &self.finish_points {
            adjacency
                .entry(fp.clone())
                .or_default()
                .push(END.to_string());
        }

        Ok(CompiledStateGraph {
            nodes: self.nodes,
            adjacency,
            conditional_edges: self.conditional_edges,
            channel_specs: self.channel_specs,
            entry_point,
            finish_points: self.finish_points,
        })
    }

    /// Validate the graph structure.
    fn validate(&self) -> Result<()> {
        // 1. Entry point must be set
        let entry = self.entry_point.as_deref().ok_or_else(|| {
            GraphError::InvalidGraph("Entry point not set".to_string())
        })?;

        // 2. Entry point node must exist
        if !self.nodes.contains_key(entry) {
            return Err(GraphError::InvalidGraph(format!(
                "Entry point node '{entry}' does not exist"
            ))
            .into());
        }

        // 3. All edges must reference existing nodes (or sentinels)
        for edge in &self.edges {
            self.validate_node_ref(&edge.from, "edge source")?;
            self.validate_node_ref(&edge.to, "edge target")?;
        }

        // 4. All conditional edges must reference existing source nodes
        for ce in &self.conditional_edges {
            self.validate_node_ref(&ce.from, "conditional edge source")?;
        }

        // 5. All finish points must reference existing nodes
        for fp in &self.finish_points {
            if !self.nodes.contains_key(fp) {
                return Err(GraphError::InvalidGraph(format!(
                    "Finish point node '{fp}' does not exist"
                ))
                .into());
            }
        }

        // 6. BFS reachability check from entry point (cycles are allowed)
        self.validate_reachability(entry)?;

        Ok(())
    }

    /// Check that a node reference is valid (exists as a node or is a sentinel).
    fn validate_node_ref(&self, name: &str, context: &str) -> Result<()> {
        if name == START || name == END {
            return Ok(());
        }
        if !self.nodes.contains_key(name) {
            return Err(GraphError::InvalidGraph(format!(
                "Unknown node '{name}' referenced as {context}"
            ))
            .into());
        }
        Ok(())
    }

    /// BFS from entry point to check that all user-defined nodes are reachable.
    fn validate_reachability(&self, entry: &str) -> Result<()> {
        // Build a temporary adjacency for reachability analysis
        let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();

        // Add static edges
        for edge in &self.edges {
            adj.entry(edge.from.as_str())
                .or_default()
                .push(edge.to.as_str());
        }

        // Add entry point edge
        adj.entry(START).or_default().push(entry);

        // For conditional edges, we consider all *possible* targets as reachable
        // Since we can't statically evaluate the routing function, we add edges
        // to all nodes in the graph from the source of each conditional edge.
        // This is conservative: if you can reach the source, all its conditional
        // targets are considered reachable.
        let all_node_names: Vec<&str> = self.nodes.keys().map(|s| s.as_str()).collect();
        for ce in &self.conditional_edges {
            let targets: Vec<&str> = if let Some(pm) = ce.path_map() {
                pm.values().map(|s| s.as_str()).collect()
            } else {
                // Without a path_map, we can't know all targets statically.
                // Be conservative and assume all nodes + END are possible.
                let mut t = all_node_names.clone();
                t.push(END);
                t
            };
            for target in targets {
                adj.entry(ce.from.as_str()).or_default().push(target);
            }
        }

        // Add finish point edges
        for fp in &self.finish_points {
            adj.entry(fp.as_str()).or_default().push(END);
        }

        // BFS
        let mut visited: HashSet<&str> = HashSet::new();
        let mut queue: VecDeque<&str> = VecDeque::new();
        queue.push_back(entry);
        visited.insert(entry);

        while let Some(current) = queue.pop_front() {
            if let Some(neighbors) = adj.get(current) {
                for &next in neighbors {
                    if visited.insert(next) {
                        queue.push_back(next);
                    }
                }
            }
        }

        // Check that all user-defined nodes are reachable
        for name in self.nodes.keys() {
            if !visited.contains(name.as_str()) {
                return Err(GraphError::InvalidGraph(format!(
                    "Node '{name}' is not reachable from entry point '{entry}'"
                ))
                .into());
            }
        }

        Ok(())
    }
}

impl Default for StateGraph {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn noop_node(name: &str) -> NodeFn {
        let name = name.to_string();
        NodeFn::new(name, |state: Value, _config| async move { Ok(state) })
    }

    // --- Node addition tests ---

    #[test]
    fn add_node_success() {
        let mut graph = StateGraph::new();
        assert!(graph.add_node(noop_node("a")).is_ok());
    }

    #[test]
    fn add_duplicate_node_errors() {
        let mut graph = StateGraph::new();
        graph.add_node(noop_node("a")).unwrap();
        let result = graph.add_node(noop_node("a"));
        assert!(result.is_err());
        assert!(result.err().unwrap().to_string().contains("Duplicate"));
    }

    #[test]
    fn add_node_reserved_start_errors() {
        let mut graph = StateGraph::new();
        let result = graph.add_node(noop_node(START));
        assert!(result.is_err());
        assert!(result.err().unwrap().to_string().contains("reserved"));
    }

    #[test]
    fn add_node_reserved_end_errors() {
        let mut graph = StateGraph::new();
        let result = graph.add_node(noop_node(END));
        assert!(result.is_err());
        assert!(result.err().unwrap().to_string().contains("reserved"));
    }

    // --- Compile validation tests ---

    #[test]
    fn compile_no_entry_point() {
        let mut graph = StateGraph::new();
        graph.add_node(noop_node("a")).unwrap();
        let result = graph.compile();
        assert!(result.is_err());
        assert!(result.err().unwrap().to_string().contains("Entry point not set"));
    }

    #[test]
    fn compile_missing_entry_node() {
        let mut graph = StateGraph::new();
        graph.add_node(noop_node("a")).unwrap();
        graph.set_entry_point("nonexistent");
        let result = graph.compile();
        assert!(result.is_err());
        assert!(result.err().unwrap().to_string().contains("does not exist"));
    }

    #[test]
    fn compile_edge_unknown_source() {
        let mut graph = StateGraph::new();
        graph.add_node(noop_node("a")).unwrap();
        graph.set_entry_point("a");
        graph.set_finish_point("a");
        graph.add_edge("nonexistent", "a");
        let result = graph.compile();
        assert!(result.is_err());
        assert!(result.err().unwrap().to_string().contains("Unknown node"));
    }

    #[test]
    fn compile_edge_unknown_target() {
        let mut graph = StateGraph::new();
        graph.add_node(noop_node("a")).unwrap();
        graph.set_entry_point("a");
        graph.set_finish_point("a");
        graph.add_edge("a", "nonexistent");
        let result = graph.compile();
        assert!(result.is_err());
        assert!(result.err().unwrap().to_string().contains("Unknown node"));
    }

    #[test]
    fn compile_conditional_edge_unknown_source() {
        let mut graph = StateGraph::new();
        graph.add_node(noop_node("a")).unwrap();
        graph.set_entry_point("a");
        graph.set_finish_point("a");
        graph.add_conditional_edges(ConditionalEdge::new(
            "nonexistent",
            |_state: &Value| "a".to_string(),
            None,
        ));
        let result = graph.compile();
        assert!(result.is_err());
    }

    #[test]
    fn compile_missing_finish_node() {
        let mut graph = StateGraph::new();
        graph.add_node(noop_node("a")).unwrap();
        graph.set_entry_point("a");
        graph.set_finish_point("nonexistent");
        let result = graph.compile();
        assert!(result.is_err());
        assert!(result.err().unwrap().to_string().contains("Finish point"));
    }

    #[test]
    fn compile_unreachable_node() {
        let mut graph = StateGraph::new();
        graph.add_node(noop_node("a")).unwrap();
        graph.add_node(noop_node("b")).unwrap();
        // b is not reachable from a
        graph.set_entry_point("a");
        graph.set_finish_point("a");
        let result = graph.compile();
        assert!(result.is_err());
        assert!(result.err().unwrap().to_string().contains("not reachable"));
    }

    #[test]
    fn compile_success_linear() {
        let mut graph = StateGraph::new();
        graph.add_node(noop_node("a")).unwrap();
        graph.add_node(noop_node("b")).unwrap();
        graph.set_entry_point("a");
        graph.add_edge("a", "b");
        graph.set_finish_point("b");
        let compiled = graph.compile().unwrap();
        assert_eq!(compiled.entry_point(), "a");
        assert!(compiled.node_names().contains(&"a"));
        assert!(compiled.node_names().contains(&"b"));
    }

    #[test]
    fn compile_success_with_channels() {
        let mut graph = StateGraph::new();
        graph.add_last_value_channel("count", json!(0));
        graph.add_node(noop_node("a")).unwrap();
        graph.set_entry_point("a");
        graph.set_finish_point("a");
        let compiled = graph.compile().unwrap();
        assert!(compiled.has_channel("count"));
    }

    #[test]
    fn compile_edge_to_end_sentinel() {
        let mut graph = StateGraph::new();
        graph.add_node(noop_node("a")).unwrap();
        graph.set_entry_point("a");
        // Edge to END is valid
        graph.add_edge("a", END);
        let compiled = graph.compile().unwrap();
        assert!(compiled.edges_from("a").contains(&END.to_string()));
    }

    #[test]
    fn compile_edge_from_start_sentinel() {
        let mut graph = StateGraph::new();
        graph.add_node(noop_node("a")).unwrap();
        graph.set_entry_point("a");
        graph.set_finish_point("a");
        // Edge from START is valid (though entry_point already creates one)
        graph.add_edge(START, "a");
        let compiled = graph.compile().unwrap();
        let start_edges = compiled.edges_from(START);
        assert!(start_edges.contains(&"a".to_string()));
    }

    #[test]
    fn compile_reachable_via_conditional_edge() {
        let mut graph = StateGraph::new();
        graph.add_node(noop_node("a")).unwrap();
        graph.add_node(noop_node("b")).unwrap();
        graph.set_entry_point("a");
        graph.set_finish_point("b");

        let mut path_map = std::collections::HashMap::new();
        path_map.insert("go".to_string(), "b".to_string());
        graph.add_conditional_edges(ConditionalEdge::new(
            "a",
            |_state: &Value| "go".to_string(),
            Some(path_map),
        ));

        let compiled = graph.compile();
        assert!(compiled.is_ok());
    }
}
