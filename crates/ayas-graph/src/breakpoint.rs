use std::collections::HashMap;

use chrono::Utc;
use serde_json::Value;
use uuid::Uuid;

use ayas_checkpoint::prelude::{
    extract_command, extract_interrupt_value, extract_sends, is_command, is_interrupt, is_send,
    Checkpoint, CheckpointConfigExt, CheckpointMetadata, CheckpointStore, GraphOutput,
    INTERRUPT_KEY, SEND_KEY,
};
use ayas_core::config::RunnableConfig;
use ayas_core::error::{GraphError, Result};

use crate::channel::{Channel, ChannelSpec};
use crate::compiled::CompiledStateGraph;
use crate::constants::END;

/// Configuration for dynamic breakpoints during graph execution.
///
/// Breakpoints pause execution at specified nodes, saving a checkpoint
/// that can be resumed later. This enables step-through debugging of
/// graph execution.
pub struct BreakpointConfig {
    /// Node names to break *before* execution (the node has not yet run).
    pub break_before: Vec<String>,
    /// Node names to break *after* execution (the node just finished).
    pub break_after: Vec<String>,
    /// Optional condition: breakpoint triggers only when this returns `true`.
    /// The function receives the current graph state.
    pub condition: Option<Box<dyn Fn(&Value) -> bool + Send + Sync>>,
}

impl BreakpointConfig {
    pub fn new() -> Self {
        Self {
            break_before: Vec::new(),
            break_after: Vec::new(),
            condition: None,
        }
    }

    /// Create a config that breaks before the given nodes.
    pub fn before(nodes: Vec<String>) -> Self {
        Self {
            break_before: nodes,
            break_after: Vec::new(),
            condition: None,
        }
    }

    /// Create a config that breaks after the given nodes.
    pub fn after(nodes: Vec<String>) -> Self {
        Self {
            break_before: Vec::new(),
            break_after: nodes,
            condition: None,
        }
    }

    /// Attach a condition function. Breakpoints only fire when this returns `true`.
    pub fn with_condition<F>(mut self, f: F) -> Self
    where
        F: Fn(&Value) -> bool + Send + Sync + 'static,
    {
        self.condition = Some(Box::new(f));
        self
    }

    fn should_break(&self, node: &str, before: bool, state: &Value) -> bool {
        let list = if before {
            &self.break_before
        } else {
            &self.break_after
        };
        if !list.iter().any(|n| n == node) {
            return false;
        }
        match &self.condition {
            Some(cond) => cond(state),
            None => true,
        }
    }
}

impl Default for BreakpointConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl CompiledStateGraph {
    /// Execute the graph with checkpoint support and dynamic breakpoints.
    ///
    /// Behaves like `invoke_resumable`, but additionally checks the
    /// `breakpoints` configuration before and after each node execution.
    /// When a breakpoint triggers, a checkpoint is saved and execution
    /// returns `GraphOutput::Interrupted` with an interrupt value describing
    /// the breakpoint.
    pub async fn invoke_with_breakpoints(
        &self,
        input: Value,
        config: &RunnableConfig,
        checkpointer: &dyn CheckpointStore,
        breakpoints: &BreakpointConfig,
    ) -> Result<GraphOutput> {
        let thread_id = config
            .thread_id()
            .unwrap_or_else(|| Uuid::new_v4().to_string());

        let mut channels: HashMap<String, Box<dyn Channel>> = self
            .channel_specs
            .iter()
            .map(|(k, spec)| (k.clone(), spec.create()))
            .collect();

        let mut current_nodes;
        let mut step = 0usize;
        let mut checkpoint_step = 0usize;
        let mut parent_checkpoint_id: Option<String> = None;

        if let Some(checkpoint_id) = config.checkpoint_id() {
            let checkpoint = checkpointer
                .get(&thread_id, &checkpoint_id)
                .await?
                .ok_or_else(|| {
                    GraphError::Checkpoint(format!(
                        "Checkpoint '{checkpoint_id}' not found for thread '{thread_id}'"
                    ))
                })?;

            for (key, value) in &checkpoint.channel_values {
                if let Some(ch) = channels.get_mut(key) {
                    ch.restore(value.clone());
                }
            }

            if let Some(resume_val) = config.resume_value() {
                if let Some(ch) = channels.get_mut("resume_value") {
                    ch.update(vec![resume_val])?;
                } else {
                    let mut ch = ChannelSpec::LastValue { default: Value::Null }.create();
                    ch.update(vec![resume_val])?;
                    channels.insert("resume_value".to_string(), ch);
                }
            }

            current_nodes = checkpoint.pending_nodes.clone();
            checkpoint_step = checkpoint.step + 1;
            parent_checkpoint_id = Some(checkpoint.id.clone());
        } else {
            if let Value::Object(map) = &input {
                for (key, value) in map {
                    if let Some(ch) = channels.get_mut(key) {
                        ch.update(vec![value.clone()])?;
                    }
                }
            }
            current_nodes = vec![self.entry_point.clone()];
        }

        let mut _node_step = 0usize;

        while !current_nodes.is_empty() {
            if step >= config.recursion_limit {
                return Err(GraphError::RecursionLimit {
                    limit: config.recursion_limit,
                }
                .into());
            }

            let mut all_next: Vec<String> = Vec::new();

            for node_name in &current_nodes {
                let state = Self::build_state(&channels);

                // --- break_before check ---
                if breakpoints.should_break(node_name, true, &state) {
                    let cp_id = Uuid::new_v4().to_string();
                    let channel_values: HashMap<String, Value> = channels
                        .iter()
                        .map(|(k, ch)| (k.clone(), ch.checkpoint()))
                        .collect();

                    // Pending nodes: this node and any remaining in current_nodes
                    let pending = vec![node_name.clone()];

                    let checkpoint = Checkpoint {
                        id: cp_id.clone(),
                        thread_id: thread_id.clone(),
                        parent_id: parent_checkpoint_id.clone(),
                        step: checkpoint_step,
                        channel_values,
                        pending_nodes: pending,
                        metadata: CheckpointMetadata {
                            source: "breakpoint_before".into(),
                            step: checkpoint_step,
                            node_name: Some(node_name.clone()),
                        },
                        created_at: Utc::now(),
                    };

                    checkpointer.put(checkpoint).await?;

                    return Ok(GraphOutput::Interrupted {
                        checkpoint_id: cp_id,
                        interrupt_value: serde_json::json!({
                            "breakpoint": "before",
                            "node": node_name,
                        }),
                        state,
                    });
                }

                let node = self.nodes.get(node_name).ok_or_else(|| {
                    GraphError::InvalidGraph(format!(
                        "Node '{node_name}' not found during execution"
                    ))
                })?;

                let output = node.invoke(state.clone(), config).await.map_err(|e| {
                    GraphError::NodeExecution {
                        node: node_name.clone(),
                        source: Box::new(e),
                    }
                })?;

                // Priority: command → interrupt → send → normal
                if is_command(&output) {
                    if let Some((update, goto)) = extract_command(&output) {
                        Self::update_channels(&mut channels, &update)?;
                        let state_after = Self::build_state(&channels);
                        let next = if goto == END {
                            Vec::new()
                        } else {
                            vec![goto]
                        };

                        let cp_id = Uuid::new_v4().to_string();
                        let channel_values: HashMap<String, Value> = channels
                            .iter()
                            .map(|(k, ch)| (k.clone(), ch.checkpoint()))
                            .collect();

                        let checkpoint = Checkpoint {
                            id: cp_id.clone(),
                            thread_id: thread_id.clone(),
                            parent_id: parent_checkpoint_id.clone(),
                            step: checkpoint_step,
                            channel_values,
                            pending_nodes: next.clone(),
                            metadata: CheckpointMetadata {
                                source: "command".into(),
                                step: checkpoint_step,
                                node_name: Some(node_name.clone()),
                            },
                            created_at: Utc::now(),
                        };

                        checkpointer.put(checkpoint).await?;
                        parent_checkpoint_id = Some(cp_id);
                        checkpoint_step += 1;

                        // break_after check for command nodes
                        if breakpoints.should_break(node_name, false, &state_after) {
                            return Ok(GraphOutput::Interrupted {
                                checkpoint_id: parent_checkpoint_id.clone().unwrap(),
                                interrupt_value: serde_json::json!({
                                    "breakpoint": "after",
                                    "node": node_name,
                                }),
                                state: state_after,
                            });
                        }

                        all_next.extend(next);
                        _node_step += 1;
                        continue;
                    }
                }

                if is_interrupt(&output) {
                    let raw_interrupt =
                        output.get(INTERRUPT_KEY).cloned().unwrap_or(Value::Null);
                    let interrupt_value =
                        extract_interrupt_value(&output).unwrap_or(raw_interrupt);

                    let mut filtered = output.clone();
                    if let Value::Object(ref mut map) = filtered {
                        map.remove(INTERRUPT_KEY);
                    }
                    Self::update_channels(&mut channels, &filtered)?;

                    let state_after = Self::build_state(&channels);
                    let next = self.next_nodes(node_name, &state_after);

                    let cp_id = Uuid::new_v4().to_string();
                    let channel_values: HashMap<String, Value> = channels
                        .iter()
                        .map(|(k, ch)| (k.clone(), ch.checkpoint()))
                        .collect();

                    let checkpoint = Checkpoint {
                        id: cp_id.clone(),
                        thread_id: thread_id.clone(),
                        parent_id: parent_checkpoint_id,
                        step: checkpoint_step,
                        channel_values,
                        pending_nodes: next,
                        metadata: CheckpointMetadata {
                            source: "interrupt".into(),
                            step: checkpoint_step,
                            node_name: Some(node_name.clone()),
                        },
                        created_at: Utc::now(),
                    };

                    checkpointer.put(checkpoint).await?;

                    return Ok(GraphOutput::Interrupted {
                        checkpoint_id: cp_id,
                        interrupt_value,
                        state: state_after,
                    });
                }

                if is_send(&output) {
                    if let Some(sends) = extract_sends(&output) {
                        let mut filtered = output.clone();
                        if let Value::Object(ref mut map) = filtered {
                            map.remove(SEND_KEY);
                        }
                        Self::update_channels(&mut channels, &filtered)?;

                        for send in sends {
                            let send_node =
                                self.nodes.get(&send.node).ok_or_else(|| {
                                    GraphError::InvalidGraph(format!(
                                        "Send target node '{}' not found",
                                        send.node
                                    ))
                                })?;
                            let mut send_state = Self::build_state(&channels);
                            if let (Value::Object(state_map), Value::Object(input_map)) =
                                (&mut send_state, send.input)
                            {
                                for (k, v) in input_map {
                                    state_map.insert(k, v);
                                }
                            }
                            let send_output =
                                send_node.invoke(send_state, config).await.map_err(|e| {
                                    GraphError::NodeExecution {
                                        node: send.node.clone(),
                                        source: Box::new(e),
                                    }
                                })?;
                            Self::update_channels(&mut channels, &send_output)?;
                        }

                        let state_after = Self::build_state(&channels);
                        let next = self.next_nodes(node_name, &state_after);

                        let cp_id = Uuid::new_v4().to_string();
                        let channel_values: HashMap<String, Value> = channels
                            .iter()
                            .map(|(k, ch)| (k.clone(), ch.checkpoint()))
                            .collect();

                        let checkpoint = Checkpoint {
                            id: cp_id.clone(),
                            thread_id: thread_id.clone(),
                            parent_id: parent_checkpoint_id.clone(),
                            step: checkpoint_step,
                            channel_values,
                            pending_nodes: next.clone(),
                            metadata: CheckpointMetadata {
                                source: "send".into(),
                                step: checkpoint_step,
                                node_name: Some(node_name.clone()),
                            },
                            created_at: Utc::now(),
                        };

                        checkpointer.put(checkpoint).await?;
                        parent_checkpoint_id = Some(cp_id);
                        checkpoint_step += 1;

                        // break_after check for send nodes
                        if breakpoints.should_break(node_name, false, &state_after) {
                            return Ok(GraphOutput::Interrupted {
                                checkpoint_id: parent_checkpoint_id.clone().unwrap(),
                                interrupt_value: serde_json::json!({
                                    "breakpoint": "after",
                                    "node": node_name,
                                }),
                                state: state_after,
                            });
                        }

                        all_next.extend(next);
                        _node_step += 1;
                        continue;
                    }
                }

                // Normal flow
                Self::update_channels(&mut channels, &output)?;

                let state_after = Self::build_state(&channels);
                let next = self.next_nodes(node_name, &state_after);

                let cp_id = Uuid::new_v4().to_string();
                let channel_values: HashMap<String, Value> = channels
                    .iter()
                    .map(|(k, ch)| (k.clone(), ch.checkpoint()))
                    .collect();

                let checkpoint = Checkpoint {
                    id: cp_id.clone(),
                    thread_id: thread_id.clone(),
                    parent_id: parent_checkpoint_id.clone(),
                    step: checkpoint_step,
                    channel_values,
                    pending_nodes: next.clone(),
                    metadata: CheckpointMetadata {
                        source: "loop".into(),
                        step: checkpoint_step,
                        node_name: Some(node_name.clone()),
                    },
                    created_at: Utc::now(),
                };

                checkpointer.put(checkpoint).await?;
                parent_checkpoint_id = Some(cp_id);
                checkpoint_step += 1;

                // --- break_after check ---
                if breakpoints.should_break(node_name, false, &state_after) {
                    return Ok(GraphOutput::Interrupted {
                        checkpoint_id: parent_checkpoint_id.clone().unwrap(),
                        interrupt_value: serde_json::json!({
                            "breakpoint": "after",
                            "node": node_name,
                        }),
                        state: state_after,
                    });
                }

                all_next.extend(next);
                _node_step += 1;
            }

            all_next.sort();
            all_next.dedup();

            for ch in channels.values_mut() {
                ch.on_step_end();
            }

            current_nodes = all_next;
            step += 1;
        }

        Ok(GraphOutput::Complete(Self::build_state(&channels)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ayas_checkpoint::prelude::MemoryCheckpointStore;
    use ayas_core::config::RunnableConfig;
    use serde_json::json;

    use crate::node::NodeFn;
    use crate::state_graph::StateGraph;

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

    #[tokio::test]
    async fn test_break_before_node() {
        let graph = build_linear_graph();
        let store = MemoryCheckpointStore::new();
        let config = default_config().with_thread_id("bp-before");

        let bp = BreakpointConfig::before(vec!["b".into()]);

        let result = graph
            .invoke_with_breakpoints(json!({}), &config, &store, &bp)
            .await
            .unwrap();

        // Should interrupt before "b" executes
        assert!(result.is_interrupted());
        match &result {
            GraphOutput::Interrupted {
                interrupt_value,
                state,
                ..
            } => {
                assert_eq!(interrupt_value["breakpoint"], json!("before"));
                assert_eq!(interrupt_value["node"], json!("b"));
                // "a" executed (count=1), "b" has NOT executed yet
                assert_eq!(state["count"], json!(1));
            }
            _ => panic!("Expected Interrupted"),
        }
    }

    #[tokio::test]
    async fn test_break_after_node() {
        let graph = build_linear_graph();
        let store = MemoryCheckpointStore::new();
        let config = default_config().with_thread_id("bp-after");

        let bp = BreakpointConfig::after(vec!["b".into()]);

        let result = graph
            .invoke_with_breakpoints(json!({}), &config, &store, &bp)
            .await
            .unwrap();

        assert!(result.is_interrupted());
        match &result {
            GraphOutput::Interrupted {
                interrupt_value,
                state,
                ..
            } => {
                assert_eq!(interrupt_value["breakpoint"], json!("after"));
                assert_eq!(interrupt_value["node"], json!("b"));
                // "a" executed (count=1), "b" executed (count=2)
                assert_eq!(state["count"], json!(2));
            }
            _ => panic!("Expected Interrupted"),
        }
    }

    #[tokio::test]
    async fn test_no_breakpoint_runs_to_completion() {
        let graph = build_linear_graph();
        let store = MemoryCheckpointStore::new();
        let config = default_config().with_thread_id("bp-none");

        let bp = BreakpointConfig::new();

        let result = graph
            .invoke_with_breakpoints(json!({}), &config, &store, &bp)
            .await
            .unwrap();

        assert!(result.is_complete());
        assert_eq!(result.into_value()["count"], json!(3));
    }

    #[tokio::test]
    async fn test_breakpoint_with_condition() {
        let graph = build_linear_graph();
        let store = MemoryCheckpointStore::new();
        let config = default_config().with_thread_id("bp-cond");

        // Break after "b" only when count > 5 — since count will be 2, should NOT trigger
        let bp = BreakpointConfig::after(vec!["b".into()])
            .with_condition(|state| state["count"].as_i64().unwrap_or(0) > 5);

        let result = graph
            .invoke_with_breakpoints(json!({}), &config, &store, &bp)
            .await
            .unwrap();

        // Condition not met, so graph runs to completion
        assert!(result.is_complete());
        assert_eq!(result.into_value()["count"], json!(3));
    }

    #[tokio::test]
    async fn test_breakpoint_condition_triggers() {
        let graph = build_linear_graph();
        let store = MemoryCheckpointStore::new();
        let config = default_config().with_thread_id("bp-cond-yes");

        // Break after "b" when count >= 2 — this WILL trigger (count=2 after b)
        let bp = BreakpointConfig::after(vec!["b".into()])
            .with_condition(|state| state["count"].as_i64().unwrap_or(0) >= 2);

        let result = graph
            .invoke_with_breakpoints(json!({}), &config, &store, &bp)
            .await
            .unwrap();

        assert!(result.is_interrupted());
        match &result {
            GraphOutput::Interrupted { state, .. } => {
                assert_eq!(state["count"], json!(2));
            }
            _ => panic!("Expected Interrupted"),
        }
    }

    #[tokio::test]
    async fn test_resume_after_breakpoint() {
        let graph = build_linear_graph();
        let store = MemoryCheckpointStore::new();
        let config = default_config().with_thread_id("bp-resume");

        // Break before "b"
        let bp = BreakpointConfig::before(vec!["b".into()]);

        let result = graph
            .invoke_with_breakpoints(json!({}), &config, &store, &bp)
            .await
            .unwrap();

        let checkpoint_id = match &result {
            GraphOutput::Interrupted { checkpoint_id, .. } => checkpoint_id.clone(),
            _ => panic!("Expected Interrupted"),
        };

        // Resume from the breakpoint with no breakpoints set
        let resume_config = default_config()
            .with_thread_id("bp-resume")
            .with_checkpoint_id(&checkpoint_id);
        let bp_empty = BreakpointConfig::new();

        let result = graph
            .invoke_with_breakpoints(json!({}), &resume_config, &store, &bp_empty)
            .await
            .unwrap();

        assert!(result.is_complete());
        // a: count=1 (from first run), resumed at b: count=2, c: count=3
        assert_eq!(result.into_value()["count"], json!(3));
    }
}
