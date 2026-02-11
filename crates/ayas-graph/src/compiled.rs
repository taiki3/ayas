use std::collections::HashMap;

use async_trait::async_trait;
use chrono::Utc;
use serde_json::Value;
use uuid::Uuid;

use ayas_checkpoint::prelude::{
    extract_command, extract_interrupt_value, extract_sends, is_command, is_interrupt, is_send,
    Checkpoint, CheckpointConfigExt, CheckpointMetadata, CheckpointStore, GraphOutput,
    SendDirective, INTERRUPT_KEY, SEND_KEY,
};
use ayas_core::config::RunnableConfig;
use ayas_core::error::{AyasError, GraphError, Result};
use ayas_core::runnable::Runnable;

use tokio::sync::mpsc;

use crate::channel::{Channel, ChannelSpec};
use crate::constants::END;
use crate::edge::{ConditionalEdge, ConditionalFanOutEdge};
use crate::node::NodeFn;
use crate::stream::StreamEvent;

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
    pub(crate) fan_out_edges: Vec<ConditionalFanOutEdge>,
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
    pub(crate) fn build_state(channels: &HashMap<String, Box<dyn Channel>>) -> Value {
        let mut map = serde_json::Map::new();
        for (key, ch) in channels {
            map.insert(key.clone(), ch.get().clone());
        }
        Value::Object(map)
    }

    /// Update channels from a node's partial output.
    pub(crate) fn update_channels(
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
    pub(crate) fn next_nodes(&self, current: &str, state: &Value) -> Vec<String> {
        // Check fan-out conditional edges first (highest priority for multi-target)
        for fe in &self.fan_out_edges {
            if fe.from == current {
                let targets = fe.resolve(state);
                return targets.into_iter().filter(|t| *t != END).collect();
            }
        }

        // Check conditional edges (single target)
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

    /// Execute send directives in parallel using tokio::task::JoinSet.
    ///
    /// All sends receive the same base state snapshot (built from channels before
    /// sends start), with each send's private input merged on top. Results are
    /// applied to channels in the original send order for determinism.
    async fn execute_sends_parallel(
        nodes: &HashMap<String, NodeFn>,
        sends: Vec<SendDirective>,
        channels: &mut HashMap<String, Box<dyn Channel>>,
        config: &RunnableConfig,
    ) -> Result<()> {
        let base_state = Self::build_state(channels);
        let mut join_set = tokio::task::JoinSet::new();

        for (idx, send) in sends.into_iter().enumerate() {
            let node_name = send.node;
            let send_node = nodes
                .get(&node_name)
                .ok_or_else(|| {
                    GraphError::InvalidGraph(format!(
                        "Send target node '{node_name}' not found"
                    ))
                })?
                .clone();

            let mut send_state = base_state.clone();
            if let (Value::Object(state_map), Value::Object(input_map)) =
                (&mut send_state, send.input)
            {
                for (k, v) in input_map {
                    state_map.insert(k, v);
                }
            }

            let cfg = config.clone();
            join_set.spawn(async move {
                let output = send_node.invoke(send_state, &cfg).await.map_err(|e| {
                    GraphError::NodeExecution {
                        node: node_name,
                        source: Box::new(e),
                    }
                })?;
                Ok::<_, AyasError>((idx, output))
            });
        }

        let mut results: Vec<(usize, Value)> = Vec::new();
        while let Some(res) = join_set.join_next().await {
            let result = res.map_err(|e| {
                AyasError::Other(format!("Parallel send task panicked: {e}"))
            })??;
            results.push(result);
        }
        // Sort by original send index for deterministic channel updates
        results.sort_by_key(|(idx, _)| *idx);

        for (_, output) in results {
            Self::update_channels(channels, &output)?;
        }

        Ok(())
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

                // Priority: command → send → normal
                if is_command(&output) {
                    if let Some((update, goto)) = extract_command(&output) {
                        Self::update_channels(&mut channels, &update)?;
                        let state_after = Self::build_state(&channels);
                        observer(StepInfo {
                            step_number: node_step,
                            node_name: node_name.clone(),
                            state_after: state_after.clone(),
                        });
                        node_step += 1;
                        if goto != END {
                            all_next.push(goto);
                        }
                        continue;
                    }
                }

                if is_send(&output) {
                    if let Some(sends) = extract_sends(&output) {
                        let mut filtered = output.clone();
                        if let Value::Object(ref mut map) = filtered {
                            map.remove(SEND_KEY);
                        }
                        Self::update_channels(&mut channels, &filtered)?;

                        Self::execute_sends_parallel(
                            &self.nodes, sends, &mut channels, config,
                        )
                        .await?;

                        let state_after = Self::build_state(&channels);
                        observer(StepInfo {
                            step_number: node_step,
                            node_name: node_name.clone(),
                            state_after: state_after.clone(),
                        });
                        node_step += 1;

                        let next = self.next_nodes(node_name, &state_after);
                        all_next.extend(next);
                        continue;
                    }
                }

                // Normal flow: update channels + observer + determine next nodes
                Self::update_channels(&mut channels, &output)?;

                let state_after = Self::build_state(&channels);
                observer(StepInfo {
                    step_number: node_step,
                    node_name: node_name.clone(),
                    state_after: state_after.clone(),
                });

                let next = self.next_nodes(node_name, &state_after);
                all_next.extend(next);
                node_step += 1;
            }

            // Deduplicate
            all_next.sort();
            all_next.dedup();

            // Notify channels that a super-step has ended
            for ch in channels.values_mut() {
                ch.on_step_end();
            }

            current_nodes = all_next;
            step += 1;
        }

        Ok(Self::build_state(&channels))
    }

    /// Execute the graph, emitting stream events to the provided sender.
    ///
    /// Similar to `invoke_with_observer` but emits structured [`StreamEvent`]s
    /// via a tokio mpsc channel. The caller manages the receiver and can
    /// spawn its own task to consume events.
    ///
    /// Send errors (e.g. receiver dropped) are silently ignored so that the
    /// graph always runs to completion.
    pub async fn invoke_with_streaming(
        &self,
        input: Value,
        config: &RunnableConfig,
        tx: mpsc::Sender<StreamEvent>,
    ) -> Result<Value> {
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
                let err = GraphError::RecursionLimit {
                    limit: config.recursion_limit,
                };
                let _ = tx
                    .send(StreamEvent::Error {
                        message: err.to_string(),
                    })
                    .await;
                return Err(err.into());
            }

            let mut all_next: Vec<String> = Vec::new();

            for node_name in &current_nodes {
                // Emit NodeStart
                let _ = tx
                    .send(StreamEvent::NodeStart {
                        node_name: node_name.clone(),
                        step: node_step,
                    })
                    .await;

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

                // Priority: command → send → normal
                if is_command(&output) {
                    if let Some((update, goto)) = extract_command(&output) {
                        Self::update_channels(&mut channels, &update)?;
                        let state_after = Self::build_state(&channels);

                        let _ = tx
                            .send(StreamEvent::NodeEnd {
                                node_name: node_name.clone(),
                                step: node_step,
                                state: state_after,
                            })
                            .await;
                        node_step += 1;

                        if goto != END {
                            all_next.push(goto);
                        }
                        continue;
                    }
                }

                if is_send(&output) {
                    if let Some(sends) = extract_sends(&output) {
                        let mut filtered = output.clone();
                        if let Value::Object(ref mut map) = filtered {
                            map.remove(SEND_KEY);
                        }
                        Self::update_channels(&mut channels, &filtered)?;

                        Self::execute_sends_parallel(
                            &self.nodes, sends, &mut channels, config,
                        )
                        .await?;

                        let state_after = Self::build_state(&channels);

                        let _ = tx
                            .send(StreamEvent::NodeEnd {
                                node_name: node_name.clone(),
                                step: node_step,
                                state: state_after,
                            })
                            .await;
                        node_step += 1;

                        let next = self.next_nodes(node_name, &Self::build_state(&channels));
                        all_next.extend(next);
                        continue;
                    }
                }

                // Normal flow: update channels + emit NodeEnd + determine next nodes
                Self::update_channels(&mut channels, &output)?;

                let state_after = Self::build_state(&channels);

                let _ = tx
                    .send(StreamEvent::NodeEnd {
                        node_name: node_name.clone(),
                        step: node_step,
                        state: state_after.clone(),
                    })
                    .await;

                let next = self.next_nodes(node_name, &state_after);
                all_next.extend(next);
                node_step += 1;
            }

            // Deduplicate
            all_next.sort();
            all_next.dedup();

            // Notify channels that a super-step has ended
            for ch in channels.values_mut() {
                ch.on_step_end();
            }

            current_nodes = all_next;
            step += 1;
        }

        let final_state = Self::build_state(&channels);
        let _ = tx
            .send(StreamEvent::GraphComplete {
                output: final_state.clone(),
            })
            .await;
        Ok(final_state)
    }

    /// Execute the graph with checkpoint support for resumable execution.
    ///
    /// If `config.configurable` contains a `checkpoint_id`, the graph resumes
    /// from that checkpoint. Otherwise it starts fresh from the input.
    /// After each node execution a checkpoint is saved via `checkpointer`.
    /// If a node output contains `"__interrupt__"`, execution pauses and
    /// returns `GraphOutput::Interrupted` with a checkpoint that can be
    /// resumed later.
    pub async fn invoke_resumable(
        &self,
        input: Value,
        config: &RunnableConfig,
        checkpointer: &dyn CheckpointStore,
    ) -> Result<GraphOutput> {
        self.invoke_resumable_with_observer(input, config, checkpointer, |_| {})
            .await
    }

    /// Like `invoke_resumable`, but calls `observer` after each node execution.
    pub async fn invoke_resumable_with_observer<F>(
        &self,
        input: Value,
        config: &RunnableConfig,
        checkpointer: &dyn CheckpointStore,
        observer: F,
    ) -> Result<GraphOutput>
    where
        F: Fn(StepInfo) + Send,
    {
        let thread_id = config
            .thread_id()
            .unwrap_or_else(|| Uuid::new_v4().to_string());

        // Create fresh channels
        let mut channels: HashMap<String, Box<dyn Channel>> = self
            .channel_specs
            .iter()
            .map(|(k, spec)| (k.clone(), spec.create()))
            .collect();

        let mut current_nodes;
        let mut step = 0usize;
        let mut checkpoint_step = 0usize;
        let mut parent_checkpoint_id: Option<String> = None;

        // Check if resuming from a checkpoint
        if let Some(checkpoint_id) = config.checkpoint_id() {
            let checkpoint = checkpointer
                .get(&thread_id, &checkpoint_id)
                .await?
                .ok_or_else(|| {
                    GraphError::Checkpoint(format!(
                        "Checkpoint '{checkpoint_id}' not found for thread '{thread_id}'"
                    ))
                })?;

            // Restore channels from checkpoint
            for (key, value) in &checkpoint.channel_values {
                if let Some(ch) = channels.get_mut(key) {
                    ch.restore(value.clone());
                }
            }

            // Inject resume_value if provided
            if let Some(resume_val) = config.resume_value() {
                if let Some(ch) = channels.get_mut("resume_value") {
                    ch.update(vec![resume_val])?;
                } else {
                    let mut ch =
                        ChannelSpec::LastValue { default: Value::Null }.create();
                    ch.update(vec![resume_val])?;
                    channels.insert("resume_value".to_string(), ch);
                }
            }

            current_nodes = checkpoint.pending_nodes.clone();
            checkpoint_step = checkpoint.step + 1;
            parent_checkpoint_id = Some(checkpoint.id.clone());
        } else {
            // Initialize channels from input
            if let Value::Object(map) = &input {
                for (key, value) in map {
                    if let Some(ch) = channels.get_mut(key) {
                        ch.update(vec![value.clone()])?;
                    }
                }
            }
            current_nodes = vec![self.entry_point.clone()];
        }

        let mut node_step = 0usize;

        // Execute Pregel loop
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

                // Command: apply update + route directly, bypass edges
                if is_command(&output) {
                    if let Some((update, goto)) = extract_command(&output) {
                        Self::update_channels(&mut channels, &update)?;
                        let state_after = Self::build_state(&channels);
                        let next = if goto == END {
                            Vec::new()
                        } else {
                            vec![goto]
                        };

                        observer(StepInfo {
                            step_number: node_step,
                            node_name: node_name.clone(),
                            state_after: state_after.clone(),
                        });

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

                        all_next.extend(next);
                        node_step += 1;
                        continue;
                    }
                }

                // Interrupt: pause execution and return checkpoint
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

                    observer(StepInfo {
                        step_number: node_step,
                        node_name: node_name.clone(),
                        state_after: state_after.clone(),
                    });

                    return Ok(GraphOutput::Interrupted {
                        checkpoint_id: cp_id,
                        interrupt_value,
                        state: state_after,
                    });
                }

                // Send: execute send targets in parallel
                if is_send(&output) {
                    if let Some(sends) = extract_sends(&output) {
                        let mut filtered = output.clone();
                        if let Value::Object(ref mut map) = filtered {
                            map.remove(SEND_KEY);
                        }
                        Self::update_channels(&mut channels, &filtered)?;

                        Self::execute_sends_parallel(
                            &self.nodes, sends, &mut channels, config,
                        )
                        .await?;

                        let state_after = Self::build_state(&channels);
                        let next = self.next_nodes(node_name, &state_after);

                        observer(StepInfo {
                            step_number: node_step,
                            node_name: node_name.clone(),
                            state_after: state_after.clone(),
                        });

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

                        all_next.extend(next);
                        node_step += 1;
                        continue;
                    }
                }

                // Normal flow: update channels + observer + checkpoint + next nodes
                Self::update_channels(&mut channels, &output)?;

                let state_after = Self::build_state(&channels);
                let next = self.next_nodes(node_name, &state_after);

                observer(StepInfo {
                    step_number: node_step,
                    node_name: node_name.clone(),
                    state_after: state_after.clone(),
                });

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

                all_next.extend(next);
                node_step += 1;
            }

            // Deduplicate
            all_next.sort();
            all_next.dedup();

            // Notify channels that a super-step has ended
            for ch in channels.values_mut() {
                ch.on_step_end();
            }

            current_nodes = all_next;
            step += 1;
        }

        Ok(GraphOutput::Complete(Self::build_state(&channels)))
    }

    /// Execute the graph with multi-mode streaming.
    ///
    /// Emits [`CoreStreamEvent`] events filtered by the requested [`StreamMode`]s.
    /// Terminal events (`GraphComplete`, `Error`) are always emitted regardless
    /// of the requested modes.
    ///
    /// Multiple modes can be active simultaneously (e.g. `[Values, Debug]`).
    pub async fn stream_with_modes(
        &self,
        input: Value,
        config: &RunnableConfig,
        modes: &[ayas_core::stream::StreamMode],
        tx: mpsc::Sender<ayas_core::stream::StreamEvent>,
    ) -> Result<Value> {
        use ayas_core::stream::{StreamEvent as CoreEvent, StreamMode};

        let has = |m: StreamMode| modes.contains(&m);

        let mut channels: HashMap<String, Box<dyn Channel>> = self
            .channel_specs
            .iter()
            .map(|(k, spec)| (k.clone(), spec.create()))
            .collect();

        if let Value::Object(map) = &input {
            for (key, value) in map {
                if let Some(ch) = channels.get_mut(key) {
                    ch.update(vec![value.clone()])?;
                }
            }
        }

        let mut current_nodes = vec![self.entry_point.clone()];
        let mut step = 0;
        let mut node_step = 0;

        while !current_nodes.is_empty() {
            if step >= config.recursion_limit {
                let err = GraphError::RecursionLimit {
                    limit: config.recursion_limit,
                };
                let _ = tx.send(CoreEvent::Error { message: err.to_string() }).await;
                return Err(err.into());
            }

            let mut all_next: Vec<String> = Vec::new();

            for node_name in &current_nodes {
                if has(StreamMode::Debug) {
                    let _ = tx.send(CoreEvent::Debug {
                        event_type: "node_start".into(),
                        payload: serde_json::json!({
                            "node": node_name,
                            "step": node_step,
                        }),
                    }).await;
                }

                let state = Self::build_state(&channels);

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

                // Normal flow (command/send/interrupt handling omitted for
                // simplicity in this streaming method; use
                // invoke_with_streaming for the full feature set)
                Self::update_channels(&mut channels, &output)?;

                let state_after = Self::build_state(&channels);

                if has(StreamMode::Updates) {
                    let _ = tx.send(CoreEvent::Updates {
                        node: node_name.clone(),
                        data: output.clone(),
                    }).await;
                }

                if has(StreamMode::Values) {
                    let _ = tx.send(CoreEvent::Values {
                        state: state_after.clone(),
                    }).await;
                }

                if has(StreamMode::Debug) {
                    let _ = tx.send(CoreEvent::Debug {
                        event_type: "node_end".into(),
                        payload: serde_json::json!({
                            "node": node_name,
                            "step": node_step,
                        }),
                    }).await;
                }

                let next = self.next_nodes(node_name, &state_after);
                all_next.extend(next);
                node_step += 1;

                if has(StreamMode::Debug) && !all_next.is_empty() {
                    let _ = tx.send(CoreEvent::Debug {
                        event_type: "edge_transition".into(),
                        payload: serde_json::json!({
                            "from": node_name,
                            "to": &all_next,
                        }),
                    }).await;
                }
            }

            all_next.sort();
            all_next.dedup();

            for ch in channels.values_mut() {
                ch.on_step_end();
            }

            current_nodes = all_next;
            step += 1;
        }

        let final_state = Self::build_state(&channels);
        let _ = tx.send(CoreEvent::GraphComplete {
            output: final_state.clone(),
        }).await;
        Ok(final_state)
    }

    /// Like `invoke_resumable`, but emits structured [`StreamEvent`]s via a
    /// tokio mpsc channel instead of using an observer callback.
    pub async fn invoke_resumable_with_streaming(
        &self,
        input: Value,
        config: &RunnableConfig,
        checkpointer: &dyn CheckpointStore,
        tx: mpsc::Sender<StreamEvent>,
    ) -> Result<GraphOutput> {
        let thread_id = config
            .thread_id()
            .unwrap_or_else(|| Uuid::new_v4().to_string());

        // Create fresh channels
        let mut channels: HashMap<String, Box<dyn Channel>> = self
            .channel_specs
            .iter()
            .map(|(k, spec)| (k.clone(), spec.create()))
            .collect();

        let mut current_nodes;
        let mut step = 0usize;
        let mut checkpoint_step = 0usize;
        let mut parent_checkpoint_id: Option<String> = None;

        // Check if resuming from a checkpoint
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
                    let mut ch =
                        ChannelSpec::LastValue { default: Value::Null }.create();
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

        let mut node_step = 0usize;

        while !current_nodes.is_empty() {
            if step >= config.recursion_limit {
                let err = GraphError::RecursionLimit {
                    limit: config.recursion_limit,
                };
                let _ = tx
                    .send(StreamEvent::Error {
                        message: err.to_string(),
                    })
                    .await;
                return Err(err.into());
            }

            let mut all_next: Vec<String> = Vec::new();

            for node_name in &current_nodes {
                let _ = tx
                    .send(StreamEvent::NodeStart {
                        node_name: node_name.clone(),
                        step: node_step,
                    })
                    .await;

                let state = Self::build_state(&channels);

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

                        let _ = tx
                            .send(StreamEvent::NodeEnd {
                                node_name: node_name.clone(),
                                step: node_step,
                                state: state_after,
                            })
                            .await;

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

                        all_next.extend(next);
                        node_step += 1;
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

                    let _ = tx
                        .send(StreamEvent::NodeEnd {
                            node_name: node_name.clone(),
                            step: node_step,
                            state: state_after.clone(),
                        })
                        .await;

                    let _ = tx
                        .send(StreamEvent::Interrupted {
                            checkpoint_id: cp_id.clone(),
                            interrupt_value: interrupt_value.clone(),
                        })
                        .await;

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

                        Self::execute_sends_parallel(
                            &self.nodes, sends, &mut channels, config,
                        )
                        .await?;

                        let state_after = Self::build_state(&channels);
                        let next = self.next_nodes(node_name, &state_after);

                        let _ = tx
                            .send(StreamEvent::NodeEnd {
                                node_name: node_name.clone(),
                                step: node_step,
                                state: state_after,
                            })
                            .await;

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

                        all_next.extend(next);
                        node_step += 1;
                        continue;
                    }
                }

                // Normal flow
                Self::update_channels(&mut channels, &output)?;

                let state_after = Self::build_state(&channels);
                let next = self.next_nodes(node_name, &state_after);

                let _ = tx
                    .send(StreamEvent::NodeEnd {
                        node_name: node_name.clone(),
                        step: node_step,
                        state: state_after,
                    })
                    .await;

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

                all_next.extend(next);
                node_step += 1;
            }

            all_next.sort();
            all_next.dedup();

            for ch in channels.values_mut() {
                ch.on_step_end();
            }

            current_nodes = all_next;
            step += 1;
        }

        let final_state = Self::build_state(&channels);
        let _ = tx
            .send(StreamEvent::GraphComplete {
                output: final_state.clone(),
            })
            .await;
        Ok(GraphOutput::Complete(final_state))
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

                // Priority: command → send → normal
                if is_command(&output) {
                    if let Some((update, goto)) = extract_command(&output) {
                        Self::update_channels(&mut channels, &update)?;
                        if goto != END {
                            all_next.push(goto);
                        }
                        continue;
                    }
                }

                if is_send(&output) {
                    if let Some(sends) = extract_sends(&output) {
                        // Apply any non-__send__ updates from the output
                        let mut filtered = output.clone();
                        if let Value::Object(ref mut map) = filtered {
                            map.remove(SEND_KEY);
                        }
                        Self::update_channels(&mut channels, &filtered)?;

                        // Execute send targets in parallel
                        Self::execute_sends_parallel(
                            &self.nodes, sends, &mut channels, config,
                        )
                        .await?;

                        // After all sends, determine next nodes from the sender
                        let state_after = Self::build_state(&channels);
                        let next = self.next_nodes(node_name, &state_after);
                        all_next.extend(next);
                        continue;
                    }
                }

                // Normal flow: update channels + determine next nodes
                Self::update_channels(&mut channels, &output)?;
                let state_after = Self::build_state(&channels);
                let next = self.next_nodes(node_name, &state_after);
                all_next.extend(next);
            }

            // Deduplicate
            all_next.sort();
            all_next.dedup();

            // Notify channels that a super-step has ended
            for ch in channels.values_mut() {
                ch.on_step_end();
            }

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

    // ---- Resumable / checkpoint tests ----

    use ayas_checkpoint::prelude::MemoryCheckpointStore;

    #[tokio::test]
    async fn test_invoke_resumable_basic() {
        let graph = build_linear_graph();
        let store = MemoryCheckpointStore::new();
        let config = default_config().with_thread_id("thread-basic");

        let result = graph
            .invoke_resumable(json!({}), &config, &store)
            .await
            .unwrap();

        assert!(result.is_complete());
        assert_eq!(result.into_value()["count"], json!(3));

        // Verify checkpoints were saved (one per node: a, b, c)
        let checkpoints = store.list("thread-basic").await.unwrap();
        assert_eq!(checkpoints.len(), 3);
    }

    #[tokio::test]
    async fn test_invoke_resumable_resume() {
        let graph = build_linear_graph();
        let store = MemoryCheckpointStore::new();
        let config = default_config().with_thread_id("thread-resume");

        // Run graph fully
        let result = graph
            .invoke_resumable(json!({}), &config, &store)
            .await
            .unwrap();
        assert!(result.is_complete());
        assert_eq!(result.into_value()["count"], json!(3));

        let checkpoints = store.list("thread-resume").await.unwrap();
        assert_eq!(checkpoints.len(), 3);

        // Get checkpoint after first node (a), which has pending_nodes = ["b"]
        let after_a = checkpoints
            .iter()
            .find(|cp| cp.metadata.node_name.as_deref() == Some("a"))
            .unwrap();
        assert_eq!(after_a.channel_values["count"], json!(1));

        // Resume from after node a
        let resume_config = default_config()
            .with_thread_id("thread-resume")
            .with_checkpoint_id(&after_a.id);

        let result = graph
            .invoke_resumable(json!({}), &resume_config, &store)
            .await
            .unwrap();
        assert!(result.is_complete());
        // Resumed with count=1, b makes 2, c makes 3
        assert_eq!(result.into_value()["count"], json!(3));

        // Total checkpoints: 3 original + 2 from resume (b and c)
        let checkpoints = store.list("thread-resume").await.unwrap();
        assert_eq!(checkpoints.len(), 5);
    }

    #[tokio::test]
    async fn test_invoke_resumable_interrupt() {
        let mut g = StateGraph::new();
        g.add_last_value_channel("count", json!(0));

        g.add_node(NodeFn::new("a", |_state: Value, _cfg| async move {
            Ok(json!({"count": 1}))
        }))
        .unwrap();
        g.add_node(NodeFn::new(
            "interrupter",
            |state: Value, _cfg| async move {
                let c = state["count"].as_i64().unwrap_or(0);
                Ok(json!({
                    "count": c + 1,
                    "__interrupt__": {"value": "approve?"}
                }))
            },
        ))
        .unwrap();
        g.add_node(NodeFn::new("c", |state: Value, _cfg| async move {
            let c = state["count"].as_i64().unwrap_or(0);
            Ok(json!({"count": c + 1}))
        }))
        .unwrap();

        g.set_entry_point("a");
        g.add_edge("a", "interrupter");
        g.add_edge("interrupter", "c");
        g.set_finish_point("c");

        let graph = g.compile().unwrap();
        let store = MemoryCheckpointStore::new();
        let config = default_config().with_thread_id("thread-interrupt");

        let result = graph
            .invoke_resumable(json!({}), &config, &store)
            .await
            .unwrap();

        assert!(result.is_interrupted());
        match &result {
            GraphOutput::Interrupted {
                checkpoint_id,
                interrupt_value,
                state,
            } => {
                assert!(!checkpoint_id.is_empty());
                assert_eq!(interrupt_value, &json!("approve?"));
                assert_eq!(state["count"], json!(2));
            }
            _ => panic!("Expected Interrupted"),
        }
    }

    #[tokio::test]
    async fn test_invoke_resumable_interrupt_and_resume() {
        let mut g = StateGraph::new();
        g.add_last_value_channel("count", json!(0));
        g.add_last_value_channel("resume_value", json!(null));

        g.add_node(NodeFn::new("a", |_state: Value, _cfg| async move {
            Ok(json!({"count": 1}))
        }))
        .unwrap();
        g.add_node(NodeFn::new(
            "interrupter",
            |state: Value, _cfg| async move {
                let c = state["count"].as_i64().unwrap_or(0);
                Ok(json!({
                    "count": c + 1,
                    "__interrupt__": {"value": "approve?"}
                }))
            },
        ))
        .unwrap();
        g.add_node(NodeFn::new("c", |state: Value, _cfg| async move {
            let c = state["count"].as_i64().unwrap_or(0);
            Ok(json!({"count": c + 1}))
        }))
        .unwrap();

        g.set_entry_point("a");
        g.add_edge("a", "interrupter");
        g.add_edge("interrupter", "c");
        g.set_finish_point("c");

        let graph = g.compile().unwrap();
        let store = MemoryCheckpointStore::new();
        let config = default_config().with_thread_id("thread-full-cycle");

        // First run: should interrupt
        let result = graph
            .invoke_resumable(json!({}), &config, &store)
            .await
            .unwrap();
        let checkpoint_id = match &result {
            GraphOutput::Interrupted { checkpoint_id, .. } => checkpoint_id.clone(),
            _ => panic!("Expected Interrupted"),
        };

        // Resume with resume_value
        let resume_config = default_config()
            .with_thread_id("thread-full-cycle")
            .with_checkpoint_id(&checkpoint_id)
            .with_resume_value(json!("approved"));

        let result = graph
            .invoke_resumable(json!({}), &resume_config, &store)
            .await
            .unwrap();

        assert!(result.is_complete());
        let final_state = result.into_value();
        assert_eq!(final_state["count"], json!(3)); // a:1, interrupter:2, c:3
        assert_eq!(final_state["resume_value"], json!("approved"));
    }

    #[tokio::test]
    async fn test_invoke_resumable_thread_isolation() {
        let graph = build_linear_graph();
        let store = MemoryCheckpointStore::new();

        // Run with thread-1
        let config1 = default_config().with_thread_id("thread-1");
        graph
            .invoke_resumable(json!({"count": 10}), &config1, &store)
            .await
            .unwrap();

        // Run with thread-2
        let config2 = default_config().with_thread_id("thread-2");
        graph
            .invoke_resumable(json!({"count": 100}), &config2, &store)
            .await
            .unwrap();

        // Each thread should have its own checkpoints
        let cp1 = store.list("thread-1").await.unwrap();
        let cp2 = store.list("thread-2").await.unwrap();

        assert_eq!(cp1.len(), 3);
        assert_eq!(cp2.len(), 3);

        // Thread IDs are isolated
        assert!(cp1.iter().all(|cp| cp.thread_id == "thread-1"));
        assert!(cp2.iter().all(|cp| cp.thread_id == "thread-2"));

        // No overlapping checkpoint IDs
        let cp1_ids: Vec<_> = cp1.iter().map(|cp| cp.id.clone()).collect();
        let cp2_ids: Vec<_> = cp2.iter().map(|cp| cp.id.clone()).collect();
        for id in &cp1_ids {
            assert!(!cp2_ids.contains(id));
        }
    }

    // ---- Command API tests ----

    use ayas_checkpoint::prelude::{command_output, send_output, SendDirective};

    #[tokio::test]
    async fn test_command_basic() {
        // Graph: a → b → END, but "a" uses command to route to "b"
        let mut g = StateGraph::new();
        g.add_last_value_channel("count", json!(0));

        g.add_node(NodeFn::new("a", |_state: Value, _cfg| async move {
            Ok(command_output(json!({"count": 10}), "b"))
        }))
        .unwrap();
        g.add_node(NodeFn::new("b", |state: Value, _cfg| async move {
            let c = state["count"].as_i64().unwrap_or(0);
            Ok(json!({"count": c + 1}))
        }))
        .unwrap();

        g.set_entry_point("a");
        g.add_edge("a", "b"); // static edge exists but command should override
        g.set_finish_point("b");

        let graph = g.compile().unwrap();
        let config = default_config();
        let result = graph.invoke(json!({}), &config).await.unwrap();
        // a sets count=10 via command, b increments to 11
        assert_eq!(result["count"], json!(11));
    }

    #[tokio::test]
    async fn test_command_bypasses_edges() {
        // Graph: a →(conditional)→ c, but "a" uses command to route to "b"
        let mut g = StateGraph::new();
        g.add_last_value_channel("count", json!(0));

        g.add_node(NodeFn::new("a", |_state: Value, _cfg| async move {
            Ok(command_output(json!({"count": 5}), "b"))
        }))
        .unwrap();
        g.add_node(NodeFn::new("b", |state: Value, _cfg| async move {
            let c = state["count"].as_i64().unwrap_or(0);
            Ok(json!({"count": c + 100}))
        }))
        .unwrap();
        g.add_node(NodeFn::new("c", |state: Value, _cfg| async move {
            let c = state["count"].as_i64().unwrap_or(0);
            Ok(json!({"count": c + 1000}))
        }))
        .unwrap();

        g.set_entry_point("a");
        // Conditional edge that would route to "c"
        g.add_conditional_edges(ConditionalEdge::new(
            "a",
            |_state: &Value| "c".to_string(),
            None,
        ));
        g.set_finish_point("b");
        g.set_finish_point("c");

        let graph = g.compile().unwrap();
        let config = default_config();
        let result = graph.invoke(json!({}), &config).await.unwrap();
        // Command routes to "b", not "c"
        assert_eq!(result["count"], json!(105));
    }

    #[tokio::test]
    async fn test_command_to_end() {
        // Graph: a → b → END, but "a" uses command to go to END directly
        let mut g = StateGraph::new();
        g.add_last_value_channel("count", json!(0));

        g.add_node(NodeFn::new("a", |_state: Value, _cfg| async move {
            Ok(command_output(json!({"count": 42}), END))
        }))
        .unwrap();
        g.add_node(NodeFn::new("b", |state: Value, _cfg| async move {
            let c = state["count"].as_i64().unwrap_or(0);
            Ok(json!({"count": c + 1}))
        }))
        .unwrap();

        g.set_entry_point("a");
        g.add_edge("a", "b");
        g.set_finish_point("b");

        let graph = g.compile().unwrap();
        let config = default_config();
        let result = graph.invoke(json!({}), &config).await.unwrap();
        // "b" should NOT execute; command routes to END
        assert_eq!(result["count"], json!(42));
    }

    #[tokio::test]
    async fn test_command_with_state_update() {
        // Verify that the update portion of command is applied to channels
        let mut g = StateGraph::new();
        g.add_last_value_channel("count", json!(0));
        g.add_last_value_channel("name", json!(""));

        g.add_node(NodeFn::new("a", |_state: Value, _cfg| async move {
            Ok(command_output(
                json!({"count": 99, "name": "from_command"}),
                "b",
            ))
        }))
        .unwrap();
        g.add_node(NodeFn::new("b", |state: Value, _cfg| async move {
            // Just pass through; verify state was updated by command
            Ok(json!({"count": state["count"].as_i64().unwrap_or(0)}))
        }))
        .unwrap();

        g.set_entry_point("a");
        g.add_edge("a", "b");
        g.set_finish_point("b");

        let graph = g.compile().unwrap();
        let config = default_config();
        let result = graph.invoke(json!({}), &config).await.unwrap();
        assert_eq!(result["count"], json!(99));
        assert_eq!(result["name"], json!("from_command"));
    }

    #[tokio::test]
    async fn test_command_with_observer() {
        let mut g = StateGraph::new();
        g.add_last_value_channel("count", json!(0));

        g.add_node(NodeFn::new("a", |_state: Value, _cfg| async move {
            Ok(command_output(json!({"count": 10}), "b"))
        }))
        .unwrap();
        g.add_node(NodeFn::new("b", |state: Value, _cfg| async move {
            let c = state["count"].as_i64().unwrap_or(0);
            Ok(json!({"count": c + 1}))
        }))
        .unwrap();

        g.set_entry_point("a");
        g.add_edge("a", "b");
        g.set_finish_point("b");

        let graph = g.compile().unwrap();
        let config = default_config();
        let (steps, observer) = collect_observer();

        graph
            .invoke_with_observer(json!({}), &config, observer)
            .await
            .unwrap();

        let steps = steps.lock().unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].node_name, "a");
        assert_eq!(steps[0].state_after["count"], json!(10));
        assert_eq!(steps[1].node_name, "b");
        assert_eq!(steps[1].state_after["count"], json!(11));
    }

    #[tokio::test]
    async fn test_command_priority_over_interrupt() {
        // If both __command__ and __interrupt__ are present, command wins
        let mut g = StateGraph::new();
        g.add_last_value_channel("count", json!(0));

        g.add_node(NodeFn::new("a", |_state: Value, _cfg| async move {
            // Return both command and interrupt keys
            Ok(json!({
                "__command__": {
                    "update": {"count": 42},
                    "goto": "b"
                },
                "__interrupt__": {"value": "should be ignored"}
            }))
        }))
        .unwrap();
        g.add_node(NodeFn::new("b", |state: Value, _cfg| async move {
            let c = state["count"].as_i64().unwrap_or(0);
            Ok(json!({"count": c + 1}))
        }))
        .unwrap();

        g.set_entry_point("a");
        g.add_edge("a", "b");
        g.set_finish_point("b");

        let graph = g.compile().unwrap();
        let store = MemoryCheckpointStore::new();
        let config = default_config().with_thread_id("thread-cmd-int");

        let result = graph
            .invoke_resumable(json!({}), &config, &store)
            .await
            .unwrap();
        // Command should win: no interrupt, execution continues to b
        assert!(result.is_complete());
        assert_eq!(result.into_value()["count"], json!(43));
    }

    // ---- Send API tests ----

    #[tokio::test]
    async fn test_send_basic() {
        // Graph: dispatcher → collector → END
        // dispatcher sends to worker_a and worker_b
        let mut g = StateGraph::new();
        g.add_last_value_channel("count", json!(0));

        g.add_node(NodeFn::new(
            "dispatcher",
            |_state: Value, _cfg| async move {
                Ok(send_output(vec![
                    SendDirective::new("worker_a", json!({})),
                    SendDirective::new("worker_b", json!({})),
                ]))
            },
        ))
        .unwrap();
        g.add_node(NodeFn::new("worker_a", |state: Value, _cfg| async move {
            let c = state["count"].as_i64().unwrap_or(0);
            Ok(json!({"count": c + 10}))
        }))
        .unwrap();
        g.add_node(NodeFn::new("worker_b", |state: Value, _cfg| async move {
            let c = state["count"].as_i64().unwrap_or(0);
            Ok(json!({"count": c + 100}))
        }))
        .unwrap();
        g.add_node(NodeFn::new(
            "collector",
            |state: Value, _cfg| async move { Ok(state) },
        ))
        .unwrap();

        g.set_entry_point("dispatcher");
        g.add_edge("dispatcher", "collector");
        // workers are reachable via conditional edges (for validation)
        g.add_conditional_edges(ConditionalEdge::new(
            "collector",
            |_: &Value| END.to_string(),
            None,
        ));
        g.set_finish_point("collector");

        let graph = g.compile().unwrap();
        let config = default_config();
        let result = graph.invoke(json!({}), &config).await.unwrap();
        // Parallel sends: both workers see base count=0
        // worker_a outputs {"count": 10}, worker_b outputs {"count": 100}
        // Applied in send order: count=10, then count=100 → final 100
        assert_eq!(result["count"], json!(100));
    }

    #[tokio::test]
    async fn test_send_with_private_input() {
        // Each send gets its own private input merged into state
        let mut g = StateGraph::new();
        g.add_last_value_channel("result", json!(""));

        g.add_node(NodeFn::new(
            "dispatcher",
            |_state: Value, _cfg| async move {
                Ok(send_output(vec![SendDirective::new(
                    "worker",
                    json!({"task": "summarize"}),
                )]))
            },
        ))
        .unwrap();
        g.add_node(NodeFn::new("worker", |state: Value, _cfg| async move {
            // Worker receives the private input merged into state
            let task = state["task"].as_str().unwrap_or("unknown");
            Ok(json!({"result": format!("did_{task}")}))
        }))
        .unwrap();

        g.set_entry_point("dispatcher");
        g.add_conditional_edges(ConditionalEdge::new(
            "dispatcher",
            |_: &Value| END.to_string(),
            None,
        ));

        let graph = g.compile().unwrap();
        let config = default_config();
        let result = graph.invoke(json!({}), &config).await.unwrap();
        assert_eq!(result["result"], json!("did_summarize"));
    }

    #[tokio::test]
    async fn test_send_results_merge() {
        // Outputs from sent nodes are merged into channels
        let mut g = StateGraph::new();
        g.add_channel("log", crate::channel::ChannelSpec::Append);

        g.add_node(NodeFn::new(
            "dispatcher",
            |_state: Value, _cfg| async move {
                Ok(send_output(vec![
                    SendDirective::new("worker", json!({"id": "a"})),
                    SendDirective::new("worker", json!({"id": "b"})),
                ]))
            },
        ))
        .unwrap();
        g.add_node(NodeFn::new("worker", |state: Value, _cfg| async move {
            let id = state["id"].as_str().unwrap_or("?");
            Ok(json!({"log": format!("done_{id}")}))
        }))
        .unwrap();

        g.set_entry_point("dispatcher");
        g.add_conditional_edges(ConditionalEdge::new(
            "dispatcher",
            |_: &Value| END.to_string(),
            None,
        ));

        let graph = g.compile().unwrap();
        let config = default_config();
        let result = graph.invoke(json!({}), &config).await.unwrap();
        let log = result["log"].as_array().unwrap();
        assert_eq!(log.len(), 2);
        assert_eq!(log[0], json!("done_a"));
        assert_eq!(log[1], json!("done_b"));
    }

    #[tokio::test]
    async fn test_send_with_normal_output() {
        // Send output can contain both __send__ and normal channel updates
        let mut g = StateGraph::new();
        g.add_last_value_channel("count", json!(0));
        g.add_last_value_channel("status", json!(""));

        g.add_node(NodeFn::new(
            "dispatcher",
            |_state: Value, _cfg| async move {
                let mut output = send_output(vec![SendDirective::new("worker", json!({}))]);
                // Add normal channel updates alongside send
                if let Value::Object(ref mut map) = output {
                    map.insert("status".to_string(), json!("dispatched"));
                }
                Ok(output)
            },
        ))
        .unwrap();
        g.add_node(NodeFn::new("worker", |state: Value, _cfg| async move {
            let c = state["count"].as_i64().unwrap_or(0);
            Ok(json!({"count": c + 1}))
        }))
        .unwrap();

        g.set_entry_point("dispatcher");
        g.add_conditional_edges(ConditionalEdge::new(
            "dispatcher",
            |_: &Value| END.to_string(),
            None,
        ));

        let graph = g.compile().unwrap();
        let config = default_config();
        let result = graph.invoke(json!({}), &config).await.unwrap();
        assert_eq!(result["status"], json!("dispatched"));
        assert_eq!(result["count"], json!(1));
    }

    // ---- Fan-Out Conditional Edge tests ----

    use crate::edge::ConditionalFanOutEdge;

    #[tokio::test]
    async fn test_fan_out_conditional_edge() {
        // Graph: router → fan-out → [research, coding] → aggregator → END
        let mut g = StateGraph::new();
        g.add_channel("log", crate::channel::ChannelSpec::Append);

        g.add_node(NodeFn::new(
            "router",
            |_state: Value, _cfg| async move { Ok(json!({"log": "routed"})) },
        ))
        .unwrap();
        g.add_node(NodeFn::new(
            "research",
            |_state: Value, _cfg| async move { Ok(json!({"log": "researched"})) },
        ))
        .unwrap();
        g.add_node(NodeFn::new(
            "coding",
            |_state: Value, _cfg| async move { Ok(json!({"log": "coded"})) },
        ))
        .unwrap();
        g.add_node(NodeFn::new(
            "aggregator",
            |_state: Value, _cfg| async move { Ok(json!({"log": "aggregated"})) },
        ))
        .unwrap();

        g.set_entry_point("router");

        let mut target_map = HashMap::new();
        target_map.insert("research".to_string(), "research".to_string());
        target_map.insert("coding".to_string(), "coding".to_string());

        g.add_conditional_fan_out_edges(ConditionalFanOutEdge::new(
            "router",
            |_state: &Value| vec!["research".to_string(), "coding".to_string()],
            target_map,
        ));

        g.add_edge("research", "aggregator");
        g.add_edge("coding", "aggregator");
        g.set_finish_point("aggregator");

        let graph = g.compile().unwrap();
        let config = default_config();
        let result = graph.invoke(json!({}), &config).await.unwrap();

        let log = result["log"].as_array().unwrap();
        // router → research & coding (parallel in same super-step) → aggregator
        // Note: research and coding execute in the same super-step,
        // aggregator runs once (dedup) in the next step
        assert!(log.contains(&json!("routed")));
        assert!(log.contains(&json!("researched")));
        assert!(log.contains(&json!("coded")));
        assert!(log.contains(&json!("aggregated")));
        assert_eq!(log.len(), 4);
    }

    #[tokio::test]
    async fn test_send_parallel_execution_timing() {
        // Three sends with 100ms sleep each; parallel should complete in < 300ms
        let mut g = StateGraph::new();
        g.add_channel("log", crate::channel::ChannelSpec::Append);

        g.add_node(NodeFn::new(
            "dispatcher",
            |_state: Value, _cfg| async move {
                Ok(send_output(vec![
                    SendDirective::new("worker", json!({"id": "a"})),
                    SendDirective::new("worker", json!({"id": "b"})),
                    SendDirective::new("worker", json!({"id": "c"})),
                ]))
            },
        ))
        .unwrap();
        g.add_node(NodeFn::new("worker", |state: Value, _cfg| async move {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            let id = state["id"].as_str().unwrap_or("?");
            Ok(json!({"log": format!("done_{id}")}))
        }))
        .unwrap();

        g.set_entry_point("dispatcher");
        g.add_conditional_edges(ConditionalEdge::new(
            "dispatcher",
            |_: &Value| END.to_string(),
            None,
        ));

        let graph = g.compile().unwrap();
        let config = default_config();

        let start = std::time::Instant::now();
        let result = graph.invoke(json!({}), &config).await.unwrap();
        let elapsed = start.elapsed();

        let log = result["log"].as_array().unwrap();
        assert_eq!(log.len(), 3);

        // Parallel: 3 × 100ms should complete in < 300ms
        assert!(
            elapsed.as_millis() < 300,
            "Expected parallel send under 300ms, took {}ms",
            elapsed.as_millis()
        );
    }
}
