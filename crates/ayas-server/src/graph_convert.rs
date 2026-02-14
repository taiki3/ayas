use std::sync::Arc;

use serde_json::{json, Value};

use ayas_checkpoint::prelude::interrupt_output;
use ayas_core::error::Result;
use ayas_core::message::Message;
use ayas_core::model::{CallOptions, ChatModel};
use ayas_graph::channel::ChannelSpec;
use ayas_graph::compiled::CompiledStateGraph;
use ayas_graph::constants::END;
use ayas_graph::edge::ConditionalEdge;
use ayas_graph::node::NodeFn;
use ayas_graph::state_graph::StateGraph;
use ayas_llm::provider::Provider;

use crate::extractors::ApiKeys;
use crate::types::{GraphChannelDto, GraphEdgeDto, GraphNodeDto};

/// Factory function type for creating ChatModel instances in graph context.
pub type GraphModelFactory =
    Arc<dyn Fn(&Provider, String, String) -> Box<dyn ChatModel> + Send + Sync>;

/// Context for building a graph with real LLM execution.
pub struct GraphBuildContext {
    pub factory: GraphModelFactory,
    pub api_keys: ApiKeys,
}

/// Convert frontend graph DTOs into a compiled StateGraph (backward-compatible).
pub fn convert_to_state_graph(
    nodes: &[GraphNodeDto],
    edges: &[GraphEdgeDto],
    channels: &[GraphChannelDto],
) -> Result<CompiledStateGraph> {
    convert_to_state_graph_with_context(nodes, edges, channels, None)
}

/// Convert frontend graph DTOs into a compiled StateGraph, optionally with LLM execution context.
pub fn convert_to_state_graph_with_context(
    nodes: &[GraphNodeDto],
    edges: &[GraphEdgeDto],
    channels: &[GraphChannelDto],
    context: Option<GraphBuildContext>,
) -> Result<CompiledStateGraph> {
    let mut graph = StateGraph::new();

    // Add channels
    for ch in channels {
        let spec = match ch.channel_type.as_str() {
            "Append" | "append" => ChannelSpec::Append,
            _ => {
                let default = ch.default.clone().unwrap_or(Value::String(String::new()));
                ChannelSpec::LastValue { default }
            }
        };
        graph.add_channel(&ch.key, spec);
    }

    // If no channels provided, add a default "value" channel
    if channels.is_empty() {
        graph.add_channel(
            "value",
            ChannelSpec::LastValue {
                default: Value::String(String::new()),
            },
        );
    }

    // Determine entry and finish from edges
    let mut entry_node: Option<String> = None;
    let mut finish_nodes: Vec<String> = Vec::new();

    for edge in edges {
        if edge.from == "start" && edge.to != "end" {
            entry_node = Some(edge.to.clone());
        }
        if edge.to == "end" && edge.from != "start" {
            finish_nodes.push(edge.from.clone());
        }
    }

    // Handle start→end direct edge: need a synthetic passthrough node
    let direct_start_end = edges
        .iter()
        .any(|e| e.from == "start" && e.to == "end");

    if direct_start_end && entry_node.is_none() {
        let synthetic = NodeFn::new("__passthrough__", |state: Value, _cfg| async move {
            Ok(state)
        });
        graph.add_node(synthetic)?;
        entry_node = Some("__passthrough__".to_string());
        finish_nodes.push("__passthrough__".to_string());
    }

    // Wrap context in Arc so it can be shared across node closures
    let ctx = context.map(Arc::new);

    // Add nodes (skip start/end - they are virtual)
    for node in nodes {
        match node.node_type.as_str() {
            "start" | "end" => continue,
            "llm" => {
                let config = node.config.clone().unwrap_or(Value::Null);
                let id = node.id.clone();
                let ctx = ctx.clone();

                let node_fn = NodeFn::new(id, move |state: Value, _cfg| {
                    let config = config.clone();
                    let ctx = ctx.clone();
                    Box::pin(async move {
                        build_llm_node(state, &config, ctx.as_deref()).await
                    })
                });
                graph.add_node(node_fn)?;
            }
            "transform" => {
                let config = node.config.clone().unwrap_or(Value::Null);
                let id = node.id.clone();
                let node_fn = NodeFn::new(id, move |state: Value, _cfg| {
                    let config = config.clone();
                    Box::pin(async move { build_transform_node(state, &config) })
                });
                graph.add_node(node_fn)?;
            }
            "interrupt" => {
                let config = node.config.clone().unwrap_or(Value::Null);
                let id = node.id.clone();
                let node_fn = NodeFn::new(id, move |state: Value, _cfg| {
                    let config = config.clone();
                    Box::pin(async move {
                        let interrupt_val = config
                            .get("value")
                            .cloned()
                            .unwrap_or_else(|| json!({"prompt": "Human input needed"}));
                        let mut output = interrupt_output(interrupt_val);
                        // Merge state values through
                        if let Value::Object(ref state_map) = state {
                            if let Value::Object(ref mut out_map) = output {
                                for (k, v) in state_map {
                                    if !out_map.contains_key(k) {
                                        out_map.insert(k.clone(), v.clone());
                                    }
                                }
                            }
                        }
                        Ok(output)
                    })
                });
                graph.add_node(node_fn)?;
            }
            _ => {
                // passthrough / conditional / unknown → passthrough node
                let id = node.id.clone();
                let node_fn =
                    NodeFn::new(id, |state: Value, _cfg| async move { Ok(state) });
                graph.add_node(node_fn)?;
            }
        }
    }

    // Set entry point
    if let Some(ref entry) = entry_node {
        graph.set_entry_point(entry);
    }

    // Set finish points
    for fp in &finish_nodes {
        graph.set_finish_point(fp);
    }

    // Add edges (skip start→X and X→end, handled by entry/finish points)
    for edge in edges {
        if edge.from == "start" || edge.to == "end" {
            continue;
        }

        if let Some(condition) = &edge.condition {
            let condition = condition.clone();
            let to_target = edge.to.clone();

            let cond_edge = ConditionalEdge::new(
                &edge.from,
                move |state: &Value| {
                    if let Some(val) = state.get(&condition)
                        && (val.as_bool().unwrap_or(false)
                            || val.as_str().is_some_and(|s| !s.is_empty()))
                    {
                        return to_target.clone();
                    }
                    END.to_string()
                },
                None,
            );
            graph.add_conditional_edges(cond_edge);
        } else {
            graph.add_edge(&edge.from, &edge.to);
        }
    }

    graph.compile()
}

/// Build LLM node logic: calls a real ChatModel if context is available, otherwise dummy.
async fn build_llm_node(
    state: Value,
    config: &Value,
    context: Option<&GraphBuildContext>,
) -> Result<Value> {
    let prompt = config
        .get("prompt")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let input_channel = config
        .get("input_channel")
        .and_then(|v| v.as_str())
        .unwrap_or("value");
    let output_channel = config
        .get("output_channel")
        .and_then(|v| v.as_str())
        .unwrap_or("value");

    let Some(ctx) = context else {
        // No context: dummy behavior (backward-compatible)
        let mut state = state;
        if let Value::Object(ref mut map) = state {
            if !prompt.is_empty() {
                map.insert(
                    "last_prompt".to_string(),
                    Value::String(prompt.to_string()),
                );
            }
        }
        return Ok(state);
    };

    // Resolve provider and model from config
    let provider_str = config
        .get("provider")
        .and_then(|v| v.as_str())
        .unwrap_or("gemini");
    let provider = match provider_str {
        "claude" | "anthropic" => Provider::Claude,
        "openai" => Provider::OpenAI,
        _ => Provider::Gemini,
    };
    let model_id = config
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("gemini-2.5-flash")
        .to_string();

    let api_key = ctx.api_keys.get_key_for(&provider).map_err(|e| {
        ayas_core::error::AyasError::Other(format!("API key error: {e:?}"))
    })?;

    let model = (ctx.factory)(&provider, api_key, model_id);

    // Read user input from state
    let user_input = match state.get(input_channel) {
        Some(Value::String(s)) => s.clone(),
        Some(v) => v.to_string(),
        None => String::new(),
    };

    let mut messages = Vec::new();
    if !prompt.is_empty() {
        messages.push(Message::system(prompt));
    }
    messages.push(Message::user(user_input.as_str()));

    let temperature = config
        .get("temperature")
        .and_then(|v| v.as_f64());
    let options = CallOptions {
        temperature,
        ..Default::default()
    };

    let result = model.generate(&messages, &options).await?;
    let response_text = result.message.content().to_string();

    let mut state = state;
    if let Value::Object(ref mut map) = state {
        map.insert(output_channel.to_string(), Value::String(response_text));
    }

    Ok(state)
}

/// Build Transform node logic: evaluates a Rhai expression against the state.
fn build_transform_node(state: Value, config: &Value) -> Result<Value> {
    let Some(expression) = config.get("expression").and_then(|v| v.as_str()) else {
        // No expression: dummy behavior (backward-compatible)
        let mut state = state;
        if let Value::Object(ref mut map) = state {
            map.insert(
                "transform_applied".to_string(),
                Value::String("(no expression)".to_string()),
            );
        }
        return Ok(state);
    };

    let output_channel = config
        .get("output_channel")
        .and_then(|v| v.as_str())
        .unwrap_or("value");

    // Rhai Engine is !Sync → create fresh per evaluation
    let mut engine = rhai::Engine::new();
    engine.set_max_operations(10_000);
    engine.set_max_call_levels(8);
    engine.set_max_expr_depths(32, 16);
    engine.set_max_string_size(4096);
    engine.set_max_array_size(256);
    engine.set_max_map_size(128);

    let dynamic_state = rhai::serde::to_dynamic(&state).map_err(|e| {
        ayas_core::error::AyasError::Other(format!(
            "Failed to convert state to Rhai dynamic: {e}"
        ))
    })?;

    let mut scope = rhai::Scope::new();
    scope.push_dynamic("state", dynamic_state);

    let result: rhai::Dynamic = engine.eval_with_scope(&mut scope, expression).map_err(|e| {
        ayas_core::error::AyasError::Other(format!("Rhai expression error: {e}"))
    })?;

    // Convert Rhai result back to serde_json::Value
    let result_value: Value = rhai::serde::from_dynamic(&result).unwrap_or_else(|_| {
        Value::String(result.to_string())
    });

    let mut state = state;
    if let Value::Object(ref mut map) = state {
        map.insert(output_channel.to_string(), result_value);
    }

    Ok(state)
}

/// Validate a graph structure without compiling.
pub fn validate_graph(
    nodes: &[GraphNodeDto],
    edges: &[GraphEdgeDto],
    _channels: &[GraphChannelDto],
) -> Vec<String> {
    let mut errors = Vec::new();

    // Check for start edge
    let has_start = edges.iter().any(|e| e.from == "start");
    if !has_start {
        errors.push("Graph must have an edge from 'start'".into());
    }

    // Check for end edge
    let has_end = edges.iter().any(|e| e.to == "end");
    if !has_end {
        errors.push("Graph must have an edge to 'end'".into());
    }

    // Check all edge targets exist as nodes
    let node_ids: std::collections::HashSet<&str> =
        nodes.iter().map(|n| n.id.as_str()).collect();
    for edge in edges {
        if edge.from != "start" && !node_ids.contains(edge.from.as_str()) {
            errors.push(format!("Edge from unknown node '{}'", edge.from));
        }
        if edge.to != "end" && !node_ids.contains(edge.to.as_str()) {
            errors.push(format!("Edge to unknown node '{}'", edge.to));
        }
    }

    // Check for unreachable nodes
    let mut reachable: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut queue: Vec<&str> = edges
        .iter()
        .filter(|e| e.from == "start")
        .map(|e| e.to.as_str())
        .collect();
    while let Some(node) = queue.pop() {
        if node == "end" || !reachable.insert(node) {
            continue;
        }
        for edge in edges {
            if edge.from == node {
                queue.push(&edge.to);
            }
        }
    }
    for node in nodes {
        if node.node_type != "start"
            && node.node_type != "end"
            && !reachable.contains(node.id.as_str())
        {
            errors.push(format!("Node '{}' is unreachable", node.id));
        }
    }

    errors
}

#[cfg(test)]
mod tests {
    use super::*;
    use ayas_core::runnable::Runnable;
    use crate::types::{GraphChannelDto, GraphEdgeDto, GraphNodeDto};

    fn node(id: &str, node_type: &str) -> GraphNodeDto {
        GraphNodeDto {
            id: id.into(),
            node_type: node_type.into(),
            label: None,
            config: None,
        }
    }

    fn edge(from: &str, to: &str) -> GraphEdgeDto {
        GraphEdgeDto {
            from: from.into(),
            to: to.into(),
            condition: None,
        }
    }

    fn channel(key: &str, channel_type: &str) -> GraphChannelDto {
        GraphChannelDto {
            key: key.into(),
            channel_type: channel_type.into(),
            default: None,
        }
    }

    #[test]
    fn convert_start_end_only() {
        let nodes = vec![];
        let edges = vec![edge("start", "end")];
        let channels = vec![channel("value", "LastValue")];
        let result = convert_to_state_graph(&nodes, &edges, &channels);
        assert!(result.is_ok());
    }

    #[test]
    fn convert_linear_pipeline() {
        let nodes = vec![node("transform_1", "transform")];
        let edges = vec![edge("start", "transform_1"), edge("transform_1", "end")];
        let channels = vec![channel("value", "LastValue")];
        let result = convert_to_state_graph(&nodes, &edges, &channels);
        assert!(result.is_ok());
        let compiled = result.unwrap();
        assert!(compiled.node_names().contains(&"transform_1"));
    }

    #[test]
    fn convert_llm_node_with_config() {
        let mut n = node("llm_1", "llm");
        n.config = Some(serde_json::json!({
            "prompt": "Hello",
            "provider": "gemini",
            "model": "gemini-2.5-flash"
        }));
        let nodes = vec![n];
        let edges = vec![edge("start", "llm_1"), edge("llm_1", "end")];
        let channels = vec![channel("value", "LastValue")];
        let result = convert_to_state_graph(&nodes, &edges, &channels);
        assert!(result.is_ok());
    }

    #[test]
    fn convert_conditional_node() {
        let nodes = vec![
            node("check", "conditional"),
            node("branch_a", "passthrough"),
        ];
        let edges = vec![
            edge("start", "check"),
            GraphEdgeDto {
                from: "check".into(),
                to: "branch_a".into(),
                condition: Some("flag".into()),
            },
            edge("branch_a", "end"),
        ];
        let channels = vec![channel("value", "LastValue"), channel("flag", "LastValue")];
        let result = convert_to_state_graph(&nodes, &edges, &channels);
        assert!(result.is_ok());
    }

    #[test]
    fn convert_channels_last_value() {
        let channels = vec![channel("text", "LastValue")];
        let nodes = vec![node("n1", "passthrough")];
        let edges = vec![edge("start", "n1"), edge("n1", "end")];
        let result = convert_to_state_graph(&nodes, &edges, &channels);
        assert!(result.is_ok());
        let compiled = result.unwrap();
        assert!(compiled.has_channel("text"));
    }

    #[test]
    fn convert_channels_append() {
        let channels = vec![channel("messages", "Append")];
        let nodes = vec![node("n1", "passthrough")];
        let edges = vec![edge("start", "n1"), edge("n1", "end")];
        let result = convert_to_state_graph(&nodes, &edges, &channels);
        assert!(result.is_ok());
        let compiled = result.unwrap();
        assert!(compiled.has_channel("messages"));
    }

    #[test]
    fn convert_complex_graph() {
        let nodes = vec![
            node("preprocessor", "transform"),
            node("llm_1", "llm"),
            node("router", "conditional"),
            node("handler_a", "passthrough"),
            node("handler_b", "passthrough"),
        ];
        let edges = vec![
            edge("start", "preprocessor"),
            edge("preprocessor", "llm_1"),
            edge("llm_1", "router"),
            GraphEdgeDto {
                from: "router".into(),
                to: "handler_a".into(),
                condition: Some("route_a".into()),
            },
            edge("handler_a", "end"),
            edge("handler_b", "end"),
        ];
        let channels = vec![
            channel("value", "LastValue"),
            channel("route_a", "LastValue"),
        ];
        let result = convert_to_state_graph(&nodes, &edges, &channels);
        assert!(result.is_ok());
    }

    #[test]
    fn convert_and_compile_succeeds() {
        let nodes = vec![node("step1", "passthrough"), node("step2", "transform")];
        let edges = vec![
            edge("start", "step1"),
            edge("step1", "step2"),
            edge("step2", "end"),
        ];
        let channels = vec![channel("value", "LastValue")];
        let result = convert_to_state_graph(&nodes, &edges, &channels);
        assert!(result.is_ok());
        let compiled = result.unwrap();
        assert_eq!(compiled.node_names().len(), 2);
    }

    // --- New tests for LLM node with context ---

    use async_trait::async_trait;
    use ayas_core::message::AIContent;
    use ayas_core::model::ChatResult;

    struct MockChatModel {
        response: String,
    }

    #[async_trait]
    impl ChatModel for MockChatModel {
        async fn generate(
            &self,
            _messages: &[ayas_core::message::Message],
            _options: &CallOptions,
        ) -> ayas_core::error::Result<ChatResult> {
            Ok(ChatResult {
                message: Message::AI(AIContent {
                    content: self.response.clone(),
                    tool_calls: Vec::new(),
                    usage: None,
                }),
                usage: None,
            })
        }

        fn model_name(&self) -> &str {
            "mock-model"
        }
    }

    fn mock_factory(response: &str) -> GraphModelFactory {
        let resp = response.to_string();
        Arc::new(move |_provider, _key, _model| {
            Box::new(MockChatModel {
                response: resp.clone(),
            })
        })
    }

    #[tokio::test]
    async fn test_llm_node_with_mock_model() {
        let mut n = node("llm_1", "llm");
        n.config = Some(json!({
            "prompt": "You are a helpful assistant",
            "provider": "gemini",
            "model": "gemini-2.5-flash"
        }));
        let nodes = vec![n];
        let edges = vec![edge("start", "llm_1"), edge("llm_1", "end")];
        let channels = vec![channel("value", "LastValue")];

        let context = GraphBuildContext {
            factory: mock_factory("Hello from LLM!"),
            api_keys: ApiKeys {
                gemini_key: Some("test-key".into()),
                ..Default::default()
            },
        };

        let compiled = convert_to_state_graph_with_context(
            &nodes, &edges, &channels, Some(context),
        )
        .unwrap();

        let config = ayas_core::config::RunnableConfig::default();
        let input = json!({"value": "What is Rust?"});
        let output = compiled.invoke(input, &config).await.unwrap();
        assert_eq!(output["value"], "Hello from LLM!");
    }

    #[tokio::test]
    async fn test_llm_node_without_context_backward_compat() {
        let mut n = node("llm_1", "llm");
        n.config = Some(json!({
            "prompt": "Hello"
        }));
        let nodes = vec![n];
        let edges = vec![edge("start", "llm_1"), edge("llm_1", "end")];
        let channels = vec![
            channel("value", "LastValue"),
            channel("last_prompt", "LastValue"),
        ];

        // No context → dummy behavior
        let compiled = convert_to_state_graph_with_context(
            &nodes, &edges, &channels, None,
        )
        .unwrap();

        let config = ayas_core::config::RunnableConfig::default();
        let input = json!({"value": "test", "last_prompt": ""});
        let output = compiled.invoke(input, &config).await.unwrap();
        // Dummy: stores last_prompt
        assert_eq!(output["last_prompt"], "Hello");
        assert_eq!(output["value"], "test");
    }

    #[tokio::test]
    async fn test_llm_node_custom_channels() {
        let mut n = node("llm_1", "llm");
        n.config = Some(json!({
            "prompt": "Summarize",
            "provider": "gemini",
            "model": "gemini-2.5-flash",
            "input_channel": "input_text",
            "output_channel": "result"
        }));
        let nodes = vec![n];
        let edges = vec![edge("start", "llm_1"), edge("llm_1", "end")];
        let channels = vec![
            channel("input_text", "LastValue"),
            channel("result", "LastValue"),
        ];

        let context = GraphBuildContext {
            factory: mock_factory("Summary result"),
            api_keys: ApiKeys {
                gemini_key: Some("test-key".into()),
                ..Default::default()
            },
        };

        let compiled = convert_to_state_graph_with_context(
            &nodes, &edges, &channels, Some(context),
        )
        .unwrap();

        let config = ayas_core::config::RunnableConfig::default();
        let input = json!({"input_text": "Long text...", "result": ""});
        let output = compiled.invoke(input, &config).await.unwrap();
        assert_eq!(output["result"], "Summary result");
        assert_eq!(output["input_text"], "Long text...");
    }

    #[tokio::test]
    async fn test_transform_node_rhai_expression() {
        let mut n = node("transform_1", "transform");
        n.config = Some(json!({
            "expression": "state.value + \" world\"",
            "output_channel": "value"
        }));
        let nodes = vec![n];
        let edges = vec![edge("start", "transform_1"), edge("transform_1", "end")];
        let channels = vec![channel("value", "LastValue")];

        let compiled = convert_to_state_graph(&nodes, &edges, &channels).unwrap();

        let config = ayas_core::config::RunnableConfig::default();
        let input = json!({"value": "hello"});
        let output = compiled.invoke(input, &config).await.unwrap();
        assert_eq!(output["value"], "hello world");
    }

    #[tokio::test]
    async fn test_transform_node_numeric_expression() {
        let mut n = node("calc", "transform");
        n.config = Some(json!({
            "expression": "state.count + 10",
            "output_channel": "count"
        }));
        let nodes = vec![n];
        let edges = vec![edge("start", "calc"), edge("calc", "end")];
        let channels = vec![channel("count", "LastValue")];

        let compiled = convert_to_state_graph(&nodes, &edges, &channels).unwrap();

        let config = ayas_core::config::RunnableConfig::default();
        let input = json!({"count": 5});
        let output = compiled.invoke(input, &config).await.unwrap();
        assert_eq!(output["count"], 15);
    }

    #[tokio::test]
    async fn test_transform_node_no_expression_backward_compat() {
        let n = node("transform_1", "transform");
        let nodes = vec![n];
        let edges = vec![edge("start", "transform_1"), edge("transform_1", "end")];
        let channels = vec![
            channel("value", "LastValue"),
            channel("transform_applied", "LastValue"),
        ];

        let compiled = convert_to_state_graph(&nodes, &edges, &channels).unwrap();

        let config = ayas_core::config::RunnableConfig::default();
        let input = json!({"value": "test", "transform_applied": ""});
        let output = compiled.invoke(input, &config).await.unwrap();
        assert_eq!(output["value"], "test");
        assert_eq!(output["transform_applied"], "(no expression)");
    }
}
