use std::sync::Arc;

use serde_json::{json, Value};

use ayas_checkpoint::prelude::interrupt_output;
use ayas_core::config::RunnableConfig;
use ayas_core::error::{AyasError, Result};
use ayas_core::message::{AIContent, ContentPart, ContentSource, Message};
use ayas_core::model::{CallOptions, ChatModel, ResponseFormat};
use ayas_core::runnable::Runnable;
use ayas_core::tool::Tool;
use ayas_deep_research::client::InteractionsClient;
use ayas_deep_research::runnable::{DeepResearchInput, DeepResearchRunnable};
use ayas_deep_research::types::ToolConfig;
use ayas_agent::react::create_react_agent;
use ayas_graph::channel::ChannelSpec;
use ayas_graph::compiled::CompiledStateGraph;
use ayas_graph::constants::END;
use ayas_graph::edge::{ConditionalEdge, ConditionalFanOutEdge};
use ayas_graph::node::NodeFn;
use ayas_graph::state_graph::StateGraph;
use ayas_llm::provider::Provider;

use crate::extractors::ApiKeys;
use crate::types::{GraphChannelDto, GraphEdgeDto, GraphNodeDto};

/// Factory function type for creating ChatModel instances in graph context.
pub type GraphModelFactory =
    Arc<dyn Fn(&Provider, String, String) -> Box<dyn ChatModel> + Send + Sync>;

/// Factory function type for creating InteractionsClient instances for Deep Research.
pub type GraphResearchFactory =
    Arc<dyn Fn(String) -> Arc<dyn InteractionsClient> + Send + Sync>;

/// Factory function type for creating tools by name.
pub type GraphToolsFactory =
    Arc<dyn Fn(&[String]) -> Vec<Arc<dyn Tool>> + Send + Sync>;

/// Context for building a graph with real LLM execution.
pub struct GraphBuildContext {
    pub factory: GraphModelFactory,
    pub api_keys: ApiKeys,
    pub research_factory: Option<GraphResearchFactory>,
    pub tools_factory: Option<GraphToolsFactory>,
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

    // Collect nodes that have on_error outgoing edges
    let mut error_edge_nodes: std::collections::HashSet<String> = std::collections::HashSet::new();
    for edge in edges {
        if edge.on_error {
            error_edge_nodes.insert(edge.from.clone());
        }
    }

    // If there are error edges, add a __error channel for routing
    if !error_edge_nodes.is_empty() {
        graph.add_channel("__error", ChannelSpec::LastValue { default: Value::Null });
    }

    // Wrap context in Arc so it can be shared across node closures
    let ctx = context.map(Arc::new);

    // Helper: wrap a node function with retry + error catching when needed
    // If the node has on_error edges, catch errors and set state.__error instead of propagating
    // If the node has max_retries in config, retry with exponential backoff
    fn wrap_node_fn<F>(
        id: String,
        has_error_edge: bool,
        max_retries: usize,
        inner: F,
    ) -> NodeFn
    where
        F: Fn(Value) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value>> + Send>>
            + Send
            + Sync
            + 'static,
    {
        let inner = Arc::new(inner);
        NodeFn::new(id, move |state: Value, _cfg| {
            let inner = inner.clone();
            let state_clone = state.clone();
            Box::pin(async move {
                let mut last_err = String::new();
                for attempt in 0..=max_retries {
                    if attempt > 0 {
                        let delay = std::time::Duration::from_millis(100 * (1 << (attempt - 1)));
                        tokio::time::sleep(delay).await;
                    }
                    match inner(state_clone.clone()).await {
                        Ok(val) => return Ok(val),
                        Err(e) => {
                            last_err = e.to_string();
                            if attempt == max_retries {
                                break;
                            }
                        }
                    }
                }
                // All retries exhausted
                if has_error_edge {
                    // Set __error in state and return successfully so error edges can route
                    let mut output = state_clone;
                    if let Value::Object(ref mut map) = output {
                        map.insert("__error".to_string(), Value::String(last_err));
                    }
                    Ok(output)
                } else {
                    Err(AyasError::Other(last_err))
                }
            })
        })
    }

    // Add nodes (skip start/end - they are virtual)
    for node in nodes {
        let has_error_edge = error_edge_nodes.contains(&node.id);
        let node_config = node.config.clone().unwrap_or(Value::Null);
        let max_retries = node_config
            .get("max_retries")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;

        match node.node_type.as_str() {
            "start" | "end" => continue,
            "llm" => {
                let config = node_config;
                let id = node.id.clone();
                let ctx = ctx.clone();

                let node_fn = wrap_node_fn(
                    id,
                    has_error_edge,
                    max_retries,
                    move |state: Value| {
                        let config = config.clone();
                        let ctx = ctx.clone();
                        Box::pin(async move {
                            build_llm_node(state, &config, ctx.as_deref()).await
                        })
                    },
                );
                graph.add_node(node_fn)?;
            }
            "transform" => {
                let config = node_config;
                let id = node.id.clone();
                let node_fn = wrap_node_fn(
                    id,
                    has_error_edge,
                    max_retries,
                    move |state: Value| {
                        let config = config.clone();
                        Box::pin(async move { build_transform_node(state, &config) })
                    },
                );
                graph.add_node(node_fn)?;
            }
            "deep_research" => {
                let config = node_config;
                let id = node.id.clone();
                let ctx = ctx.clone();

                let node_fn = wrap_node_fn(
                    id,
                    has_error_edge,
                    max_retries,
                    move |state: Value| {
                        let config = config.clone();
                        let ctx = ctx.clone();
                        Box::pin(async move {
                            build_deep_research_node(state, &config, ctx.as_deref()).await
                        })
                    },
                );
                graph.add_node(node_fn)?;
            }
            "agent" => {
                let config = node_config;
                let id = node.id.clone();
                let ctx = ctx.clone();

                let node_fn = wrap_node_fn(
                    id,
                    has_error_edge,
                    max_retries,
                    move |state: Value| {
                        let config = config.clone();
                        let ctx = ctx.clone();
                        Box::pin(async move {
                            build_agent_node(state, &config, ctx.as_deref()).await
                        })
                    },
                );
                graph.add_node(node_fn)?;
            }
            "interrupt" => {
                let config = node_config;
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

    // Collect fan-out edges grouped by source node
    let mut fan_out_groups: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();

    // Collect on_error routing: source → (error_target, normal_target)
    // normal_target is the non-error, non-fan-out outgoing edge target (or END)
    let mut on_error_targets: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for edge in edges {
        if edge.on_error && edge.from != "start" {
            on_error_targets.insert(edge.from.clone(), edge.to.clone());
        }
    }
    // For error-edge sources, find their normal targets
    let mut error_node_normal_targets: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for edge in edges {
        if !edge.on_error && !edge.fan_out && on_error_targets.contains_key(&edge.from) {
            error_node_normal_targets.insert(edge.from.clone(), edge.to.clone());
        }
    }
    // Remove error-edge sources from finish_nodes (we'll handle routing via conditional edges)
    for src in on_error_targets.keys() {
        finish_nodes.retain(|fp| fp != src);
    }

    // Add edges (skip start→X and X→end, handled by entry/finish points)
    for edge in edges {
        if edge.from == "start" || edge.to == "end" {
            continue;
        }

        // Skip on_error edges (handled via conditional edges below)
        if edge.on_error {
            continue;
        }

        // Skip normal edges from error-source nodes (handled via conditional edges below)
        if on_error_targets.contains_key(&edge.from) && !edge.fan_out {
            continue;
        }

        // Fan-out edges: group by source and add as ConditionalFanOutEdge later
        if edge.fan_out {
            fan_out_groups
                .entry(edge.from.clone())
                .or_default()
                .push(edge.to.clone());
            continue;
        }

        if let Some(condition) = &edge.condition {
            let condition = condition.clone();
            let to_target = edge.to.clone();

            // Detect if the condition is a Rhai expression (contains operators) or simple key
            let is_expression = condition.contains('>')
                || condition.contains('<')
                || condition.contains("==")
                || condition.contains("!=")
                || condition.contains("&&")
                || condition.contains("||")
                || condition.contains('(')
                || condition.contains('!');

            let cond_edge = ConditionalEdge::new(
                &edge.from,
                move |state: &Value| {
                    if is_expression {
                        // Rhai expression evaluation
                        let mut engine = rhai::Engine::new();
                        engine.set_max_operations(10_000);
                        engine.set_max_call_levels(8);
                        engine.set_max_expr_depths(32, 16);

                        if let Ok(dynamic_state) = rhai::serde::to_dynamic(state) {
                            let mut scope = rhai::Scope::new();
                            scope.push_dynamic("state", dynamic_state);
                            if let Ok(result) = engine.eval_with_scope::<bool>(&mut scope, &condition) {
                                if result {
                                    return to_target.clone();
                                }
                            }
                        }
                        END.to_string()
                    } else {
                        // Simple key check (legacy behavior)
                        if let Some(val) = state.get(&condition)
                            && (val.as_bool().unwrap_or(false)
                                || val.as_str().is_some_and(|s| !s.is_empty()))
                        {
                            return to_target.clone();
                        }
                        END.to_string()
                    }
                },
                None,
            );
            graph.add_conditional_edges(cond_edge);
        } else {
            graph.add_edge(&edge.from, &edge.to);
        }
    }

    // Add fan-out edges: each group becomes a ConditionalFanOutEdge
    for (from, targets) in fan_out_groups {
        let mut target_map = std::collections::HashMap::new();
        for t in &targets {
            target_map.insert(t.clone(), t.clone());
        }
        let targets_clone = targets.clone();
        let fan_out = ConditionalFanOutEdge::new(
            &from,
            move |_state: &Value| targets_clone.clone(),
            target_map,
        );
        graph.add_conditional_fan_out_edges(fan_out);
    }

    // Add on_error routing: conditional edges that route to error_target if __error is set
    for (from, error_target) in &on_error_targets {
        let raw_target = error_node_normal_targets
            .get(from)
            .cloned()
            .unwrap_or_else(|| "end".to_string());
        // Map "end" to the graph framework's END constant
        let to_normal = if raw_target == "end" { END.to_string() } else { raw_target };
        let to_error = if error_target == "end" { END.to_string() } else { error_target.clone() };
        let cond_edge = ConditionalEdge::new(
            from,
            move |state: &Value| {
                if state.get("__error").is_some_and(|v| !v.is_null()) {
                    to_error.clone()
                } else {
                    to_normal.clone()
                }
            },
            None,
        );
        graph.add_conditional_edges(cond_edge);
    }

    graph.compile()
}

/// Parse `config.attachments` array into ContentPart items.
///
/// Each attachment is `{ data: "<base64>", media_type: "image/png", name?: "..." }`.
/// Images (image/*) → `ContentPart::Image`, others → `ContentPart::File`.
fn parse_attachments(config: &Value) -> Vec<ContentPart> {
    let Some(arr) = config.get("attachments").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|a| {
            let data = a.get("data")?.as_str()?.to_string();
            let media_type = a.get("media_type")?.as_str()?.to_string();
            let source = ContentSource::Base64 { media_type: media_type.clone(), data };
            if media_type.starts_with("image/") {
                Some(ContentPart::Image { source })
            } else {
                Some(ContentPart::File { source })
            }
        })
        .collect()
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

    // Build user message: multimodal if attachments present
    let attachment_parts = parse_attachments(config);
    if attachment_parts.is_empty() {
        messages.push(Message::user(user_input.as_str()));
    } else {
        let mut parts = vec![ContentPart::Text { text: user_input }];
        parts.extend(attachment_parts);
        messages.push(Message::user_with_parts(parts));
    }

    let temperature = config
        .get("temperature")
        .and_then(|v| v.as_f64());
    let response_format = config
        .get("response_format")
        .and_then(|v| serde_json::from_value::<ResponseFormat>(v.clone()).ok());

    // Build tools if specified
    let tool_names: Vec<String> = config
        .get("tools")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();

    let tools: Vec<Arc<dyn Tool>> = if !tool_names.is_empty() {
        ctx.tools_factory
            .as_ref()
            .map(|f| f(&tool_names))
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    let tool_defs: Vec<ayas_core::tool::ToolDefinition> =
        tools.iter().map(|t| t.definition()).collect();
    let tools_map: std::collections::HashMap<String, Arc<dyn Tool>> =
        tools.into_iter().map(|t| (t.definition().name, t)).collect();

    let max_tool_iterations = config
        .get("max_tool_iterations")
        .and_then(|v| v.as_u64())
        .unwrap_or(5) as usize;

    let options = CallOptions {
        temperature,
        response_format,
        tools: tool_defs,
        ..Default::default()
    };

    // Tool-calling loop: LLM → tool execution → LLM → ... until no tool calls or limit
    let mut iteration = 0;
    loop {
        let result = model.generate(&messages, &options).await?;

        // Check for tool calls
        let tool_calls = match &result.message {
            Message::AI(AIContent { tool_calls, .. }) if !tool_calls.is_empty() => {
                tool_calls.clone()
            }
            _ => {
                // No tool calls — we're done
                let response_text = result.message.content().to_string();
                let mut state = state;
                if let Value::Object(ref mut map) = state {
                    map.insert(output_channel.to_string(), Value::String(response_text));
                }
                return Ok(state);
            }
        };

        iteration += 1;
        if iteration > max_tool_iterations {
            // Exceeded iteration limit — return last content
            let response_text = result.message.content().to_string();
            let mut state = state;
            if let Value::Object(ref mut map) = state {
                map.insert(output_channel.to_string(), Value::String(response_text));
            }
            return Ok(state);
        }

        // Add AI message with tool calls to conversation
        messages.push(result.message);

        // Execute tool calls
        for tc in tool_calls {
            let output = if let Some(tool) = tools_map.get(&tc.name) {
                tool.call(tc.arguments.clone()).await.unwrap_or_else(|e| format!("Tool error: {e}"))
            } else {
                format!("Unknown tool: {}", tc.name)
            };
            messages.push(Message::tool(output, &tc.id));
        }
    }
}

/// Build Deep Research node logic: calls the Interactions API if context is available.
async fn build_deep_research_node(
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
    let agent = config
        .get("agent")
        .and_then(|v| v.as_str())
        .unwrap_or("deep-research-pro-preview-12-2025");
    let attachments_channel = config
        .get("attachments_channel")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());

    let Some(ctx) = context else {
        // No context: dummy behavior (backward-compatible)
        let mut state = state;
        if let Value::Object(ref mut map) = state {
            map.insert(
                output_channel.to_string(),
                Value::String("(deep research: no context)".to_string()),
            );
        }
        return Ok(state);
    };

    let research_factory = ctx.research_factory.as_ref().ok_or_else(|| {
        AyasError::Other("Deep Research node requires research_factory in context".to_string())
    })?;

    let api_key = ctx.api_keys.get_key_for(&Provider::Gemini).map_err(|e| {
        AyasError::Other(format!("API key error: {e:?}"))
    })?;

    let client = (research_factory)(api_key);

    // Read user input from state
    let user_input = match state.get(input_channel) {
        Some(Value::String(s)) => s.clone(),
        Some(v) => v.to_string(),
        None => String::new(),
    };

    // Build query: apply prompt template if provided
    let query = if prompt.is_empty() {
        user_input
    } else {
        prompt.replace("{INPUT}", &user_input)
    };

    // Build attachments: first from config.attachments_text (direct text), then from state channel
    let mut attachments: Vec<ayas_core::message::ContentPart> = Vec::new();

    // Direct text attachment from node config
    if let Some(text) = config
        .get("attachments_text")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        attachments.push(ayas_core::message::ContentPart::Text {
            text: text.to_string(),
        });
    }

    // Attachments from state channel (for chaining with previous nodes)
    if let Some(att_ch) = attachments_channel {
        match state.get(att_ch) {
            Some(Value::String(s)) if !s.is_empty() => {
                attachments.push(ayas_core::message::ContentPart::Text {
                    text: s.clone(),
                });
            }
            Some(Value::Array(arr)) => {
                attachments.extend(arr.iter().filter_map(|v| {
                    v.as_str()
                        .filter(|s| !s.is_empty())
                        .map(|s| ayas_core::message::ContentPart::Text {
                            text: s.to_string(),
                        })
                }));
            }
            _ => {}
        }
    }

    // Check for file_search_store_names in config
    let file_search_store_names: Option<Vec<String>> = config
        .get("file_search_store_names")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .filter(|v: &Vec<String>| !v.is_empty());

    let runnable = DeepResearchRunnable::new(client)
        .with_agent(agent)
        .with_poll_interval(std::time::Duration::from_secs(5));

    let mut input = DeepResearchInput::new(query).with_attachments(attachments);
    if let Some(names) = file_search_store_names {
        input = input.with_tools(vec![ToolConfig::FileSearch {
            file_search_store_names: names,
        }]);
    }
    let runnable_config = RunnableConfig::default();

    let output = runnable.invoke(input, &runnable_config).await?;

    let mut state = state;
    if let Value::Object(ref mut map) = state {
        map.insert(output_channel.to_string(), Value::String(output.text));
    }

    Ok(state)
}

/// Build Agent node logic: wraps create_react_agent as a single graph node.
async fn build_agent_node(
    state: Value,
    config: &Value,
    context: Option<&GraphBuildContext>,
) -> Result<Value> {
    let input_channel = config
        .get("input_channel")
        .and_then(|v| v.as_str())
        .unwrap_or("value");
    let output_channel = config
        .get("output_channel")
        .and_then(|v| v.as_str())
        .unwrap_or("value");
    let system_prompt = config
        .get("system_prompt")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let Some(ctx) = context else {
        // No context: dummy behavior
        let mut state = state;
        if let Value::Object(ref mut map) = state {
            map.insert(
                output_channel.to_string(),
                Value::String("(agent: no context)".to_string()),
            );
        }
        return Ok(state);
    };

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
        AyasError::Other(format!("API key error: {e:?}"))
    })?;

    let model: Arc<dyn ChatModel> = Arc::from((ctx.factory)(&provider, api_key, model_id));

    // Build tools
    let tool_names: Vec<String> = config
        .get("tools")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();

    let tools: Vec<Arc<dyn Tool>> = if !tool_names.is_empty() {
        ctx.tools_factory
            .as_ref()
            .map(|f| f(&tool_names))
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    // Read user input from state
    let user_input = match state.get(input_channel) {
        Some(Value::String(s)) => s.clone(),
        Some(v) => v.to_string(),
        None => String::new(),
    };

    // Build initial messages (multimodal if attachments present)
    let mut initial_messages: Vec<Value> = Vec::new();
    if !system_prompt.is_empty() {
        initial_messages.push(serde_json::to_value(&Message::system(system_prompt))
            .map_err(AyasError::Serialization)?);
    }
    let attachment_parts = parse_attachments(config);
    let user_msg = if attachment_parts.is_empty() {
        Message::user(user_input.as_str())
    } else {
        let mut parts = vec![ContentPart::Text { text: user_input }];
        parts.extend(attachment_parts);
        Message::user_with_parts(parts)
    };
    initial_messages.push(serde_json::to_value(&user_msg)
        .map_err(AyasError::Serialization)?);

    // Create and run the ReAct agent
    let agent_graph = create_react_agent(model, tools)?;

    let recursion_limit = config
        .get("recursion_limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(25) as usize;
    let agent_config = RunnableConfig {
        recursion_limit,
        ..Default::default()
    };

    let agent_input = json!({"messages": initial_messages});
    let agent_output = agent_graph.invoke(agent_input, &agent_config).await?;

    // Extract the last AI message content from the agent output
    let response_text = agent_output
        .get("messages")
        .and_then(|v| v.as_array())
        .and_then(|arr| {
            arr.iter()
                .rev()
                .find(|m| m.get("type").and_then(|t| t.as_str()) == Some("ai"))
        })
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();

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
            fan_out: false,
            on_error: false,
        }
    }

    fn fan_out_edge(from: &str, to: &str) -> GraphEdgeDto {
        GraphEdgeDto {
            from: from.into(),
            to: to.into(),
            condition: None,
            fan_out: true,
            on_error: false,
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
                fan_out: false,
                on_error: false,
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
                fan_out: false,
                on_error: false,
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
            research_factory: None,
            tools_factory: None,
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
            research_factory: None,
            tools_factory: None,
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

    // --- Deep Research node tests ---

    use ayas_deep_research::mock::MockInteractionsClient;

    fn mock_research_factory(text: &str) -> GraphResearchFactory {
        let t = text.to_string();
        Arc::new(move |_| {
            Arc::new(MockInteractionsClient::completed(t.clone())) as Arc<dyn InteractionsClient>
        })
    }

    #[tokio::test]
    async fn test_deep_research_node_without_context() {
        let mut n = node("dr_1", "deep_research");
        n.config = Some(json!({
            "prompt": "Research this",
            "output_channel": "result"
        }));
        let nodes = vec![n];
        let edges = vec![edge("start", "dr_1"), edge("dr_1", "end")];
        let channels = vec![
            channel("value", "LastValue"),
            channel("result", "LastValue"),
        ];

        // No context → dummy behavior
        let compiled =
            convert_to_state_graph_with_context(&nodes, &edges, &channels, None).unwrap();

        let config = ayas_core::config::RunnableConfig::default();
        let input = json!({"value": "test", "result": ""});
        let output = compiled.invoke(input, &config).await.unwrap();
        assert_eq!(output["result"], "(deep research: no context)");
        assert_eq!(output["value"], "test");
    }

    #[tokio::test]
    async fn test_deep_research_node_with_mock() {
        let mut n = node("dr_1", "deep_research");
        n.config = Some(json!({
            "agent": "deep-research-pro-preview-12-2025",
            "input_channel": "value",
            "output_channel": "result"
        }));
        let nodes = vec![n];
        let edges = vec![edge("start", "dr_1"), edge("dr_1", "end")];
        let channels = vec![
            channel("value", "LastValue"),
            channel("result", "LastValue"),
        ];

        let context = GraphBuildContext {
            factory: mock_factory("unused"),
            api_keys: ApiKeys {
                gemini_key: Some("test-key".into()),
                ..Default::default()
            },
            research_factory: Some(mock_research_factory("Deep research output")),
            tools_factory: None,
        };

        let compiled =
            convert_to_state_graph_with_context(&nodes, &edges, &channels, Some(context)).unwrap();

        let config = ayas_core::config::RunnableConfig::default();
        let input = json!({"value": "Explain quantum computing", "result": ""});
        let output = compiled.invoke(input, &config).await.unwrap();
        assert_eq!(output["result"], "Deep research output");
    }

    #[tokio::test]
    async fn test_deep_research_node_prompt_template() {
        let mut n = node("dr_1", "deep_research");
        n.config = Some(json!({
            "prompt": "Please research: {INPUT}",
            "input_channel": "value",
            "output_channel": "result"
        }));
        let nodes = vec![n];
        let edges = vec![edge("start", "dr_1"), edge("dr_1", "end")];
        let channels = vec![
            channel("value", "LastValue"),
            channel("result", "LastValue"),
        ];

        let context = GraphBuildContext {
            factory: mock_factory("unused"),
            api_keys: ApiKeys {
                gemini_key: Some("test-key".into()),
                ..Default::default()
            },
            research_factory: Some(mock_research_factory("Template research result")),
            tools_factory: None,
        };

        let compiled =
            convert_to_state_graph_with_context(&nodes, &edges, &channels, Some(context)).unwrap();

        let config = ayas_core::config::RunnableConfig::default();
        let input = json!({"value": "quantum computing", "result": ""});
        let output = compiled.invoke(input, &config).await.unwrap();
        // Mock returns fixed text regardless of query, but confirms the node ran
        assert_eq!(output["result"], "Template research result");
    }

    #[tokio::test]
    async fn test_deep_research_node_attachments() {
        let mut n = node("dr_1", "deep_research");
        n.config = Some(json!({
            "input_channel": "value",
            "output_channel": "result",
            "attachments_channel": "context_text"
        }));
        let nodes = vec![n];
        let edges = vec![edge("start", "dr_1"), edge("dr_1", "end")];
        let channels = vec![
            channel("value", "LastValue"),
            channel("result", "LastValue"),
            channel("context_text", "LastValue"),
        ];

        let context = GraphBuildContext {
            factory: mock_factory("unused"),
            api_keys: ApiKeys {
                gemini_key: Some("test-key".into()),
                ..Default::default()
            },
            research_factory: Some(mock_research_factory("Research with attachments")),
            tools_factory: None,
        };

        let compiled =
            convert_to_state_graph_with_context(&nodes, &edges, &channels, Some(context)).unwrap();

        let config = ayas_core::config::RunnableConfig::default();
        let input = json!({
            "value": "research topic",
            "result": "",
            "context_text": "Some additional context"
        });
        let output = compiled.invoke(input, &config).await.unwrap();
        assert_eq!(output["result"], "Research with attachments");
    }

    #[tokio::test]
    async fn test_deep_research_node_with_file_search_stores() {
        let mut n = node("dr_1", "deep_research");
        n.config = Some(json!({
            "input_channel": "value",
            "output_channel": "result",
            "file_search_store_names": ["fileSearchStores/store-abc", "fileSearchStores/store-def"]
        }));
        let nodes = vec![n];
        let edges = vec![edge("start", "dr_1"), edge("dr_1", "end")];
        let channels = vec![
            channel("value", "LastValue"),
            channel("result", "LastValue"),
        ];

        let context = GraphBuildContext {
            factory: mock_factory("unused"),
            api_keys: ApiKeys {
                gemini_key: Some("test-key".into()),
                ..Default::default()
            },
            research_factory: Some(mock_research_factory("File search research result")),
            tools_factory: None,
        };

        let compiled =
            convert_to_state_graph_with_context(&nodes, &edges, &channels, Some(context)).unwrap();

        let config = ayas_core::config::RunnableConfig::default();
        let input = json!({"value": "topic", "result": ""});
        let output = compiled.invoke(input, &config).await.unwrap();
        assert_eq!(output["result"], "File search research result");
    }

    #[tokio::test]
    async fn test_deep_research_node_direct_text_attachment() {
        let mut n = node("dr_1", "deep_research");
        n.config = Some(json!({
            "input_channel": "value",
            "output_channel": "result",
            "attachments_text": "Reference material pasted directly"
        }));
        let nodes = vec![n];
        let edges = vec![edge("start", "dr_1"), edge("dr_1", "end")];
        let channels = vec![
            channel("value", "LastValue"),
            channel("result", "LastValue"),
        ];

        let context = GraphBuildContext {
            factory: mock_factory("unused"),
            api_keys: ApiKeys {
                gemini_key: Some("test-key".into()),
                ..Default::default()
            },
            research_factory: Some(mock_research_factory("Research with direct text")),
            tools_factory: None,
        };

        let compiled =
            convert_to_state_graph_with_context(&nodes, &edges, &channels, Some(context)).unwrap();

        let config = ayas_core::config::RunnableConfig::default();
        let input = json!({"value": "topic", "result": ""});
        let output = compiled.invoke(input, &config).await.unwrap();
        assert_eq!(output["result"], "Research with direct text");
    }

    // --- Fan-out edge tests ---

    #[test]
    fn convert_fan_out_edges() {
        // splitter → [branch_a, branch_b] (parallel) → aggregator → END
        let nodes = vec![
            node("splitter", "transform"),
            node("branch_a", "passthrough"),
            node("branch_b", "passthrough"),
            node("aggregator", "passthrough"),
        ];
        let edges = vec![
            edge("start", "splitter"),
            fan_out_edge("splitter", "branch_a"),
            fan_out_edge("splitter", "branch_b"),
            edge("branch_a", "aggregator"),
            edge("branch_b", "aggregator"),
            edge("aggregator", "end"),
        ];
        let channels = vec![channel("log", "Append")];
        let result = convert_to_state_graph(&nodes, &edges, &channels);
        assert!(result.is_ok(), "Fan-out graph should compile: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_fan_out_parallel_execution() {
        // splitter → [a, b] (parallel fan-out) → aggregator → END
        // Uses Append channel to verify both branches ran
        let mut splitter = node("splitter", "transform");
        splitter.config = Some(json!({
            "expression": "\"split\"",
            "output_channel": "log"
        }));
        let nodes = vec![
            splitter,
            node("branch_a", "transform"),
            node("branch_b", "transform"),
            node("aggregator", "passthrough"),
        ];
        // Configure branch nodes
        let nodes: Vec<GraphNodeDto> = nodes.into_iter().map(|mut n| {
            if n.id == "branch_a" {
                n.config = Some(json!({ "expression": "\"ran_a\"", "output_channel": "log" }));
            }
            if n.id == "branch_b" {
                n.config = Some(json!({ "expression": "\"ran_b\"", "output_channel": "log" }));
            }
            n
        }).collect();

        let edges = vec![
            edge("start", "splitter"),
            fan_out_edge("splitter", "branch_a"),
            fan_out_edge("splitter", "branch_b"),
            edge("branch_a", "aggregator"),
            edge("branch_b", "aggregator"),
            edge("aggregator", "end"),
        ];
        let channels = vec![channel("log", "Append")];

        let compiled = convert_to_state_graph(&nodes, &edges, &channels).unwrap();
        let config = ayas_core::config::RunnableConfig::default();
        let result = compiled.invoke(json!({}), &config).await.unwrap();

        let log = result["log"].as_array().unwrap();
        assert!(log.contains(&json!("split")), "splitter should have run");
        assert!(log.contains(&json!("ran_a")), "branch_a should have run");
        assert!(log.contains(&json!("ran_b")), "branch_b should have run");
    }

    #[test]
    fn validate_fan_out_graph() {
        let nodes = vec![
            node("splitter", "transform"),
            node("a", "passthrough"),
            node("b", "passthrough"),
        ];
        let edges = vec![
            edge("start", "splitter"),
            fan_out_edge("splitter", "a"),
            fan_out_edge("splitter", "b"),
            edge("a", "end"),
            edge("b", "end"),
        ];
        let channels = vec![channel("value", "LastValue")];
        let errors = validate_graph(&nodes, &edges, &channels);
        assert!(errors.is_empty(), "Validation should pass: {:?}", errors);
    }

    // --- Rhai expression condition tests ---

    #[tokio::test]
    async fn test_conditional_edge_rhai_expression() {
        let nodes = vec![
            node("check", "passthrough"),
            node("high", "transform"),
            node("low", "passthrough"),
        ];
        let mut high = nodes[1].clone();
        high.config = Some(json!({ "expression": "\"high_path\"", "output_channel": "result" }));
        let nodes = vec![nodes[0].clone(), high, nodes[2].clone()];

        let edges = vec![
            edge("start", "check"),
            GraphEdgeDto {
                from: "check".into(),
                to: "high".into(),
                condition: Some("state.score > 50".into()),
                fan_out: false,
                on_error: false,
            },
            edge("high", "end"),
            edge("low", "end"),
        ];
        let channels = vec![
            channel("score", "LastValue"),
            channel("result", "LastValue"),
            channel("value", "LastValue"),
        ];

        let compiled = convert_to_state_graph(&nodes, &edges, &channels).unwrap();
        let config = ayas_core::config::RunnableConfig::default();

        // Score > 50 → should route to "high"
        let input = json!({"score": 75, "result": "", "value": ""});
        let output = compiled.invoke(input, &config).await.unwrap();
        assert_eq!(output["result"], "high_path");
    }

    #[tokio::test]
    async fn test_conditional_edge_rhai_expression_false() {
        let nodes = vec![
            node("check", "passthrough"),
            node("high", "transform"),
        ];
        let mut high = nodes[1].clone();
        high.config = Some(json!({ "expression": "\"high_path\"", "output_channel": "result" }));
        let nodes = vec![nodes[0].clone(), high];

        let edges = vec![
            edge("start", "check"),
            GraphEdgeDto {
                from: "check".into(),
                to: "high".into(),
                condition: Some("state.score > 50".into()),
                fan_out: false,
                on_error: false,
            },
            edge("high", "end"),
        ];
        let channels = vec![
            channel("score", "LastValue"),
            channel("result", "LastValue"),
            channel("value", "LastValue"),
        ];

        let compiled = convert_to_state_graph(&nodes, &edges, &channels).unwrap();
        let config = ayas_core::config::RunnableConfig::default();

        // Score <= 50 → should route to END (expression returns false)
        let input = json!({"score": 30, "result": "", "value": ""});
        let output = compiled.invoke(input, &config).await.unwrap();
        // high node never ran, so result stays empty
        assert_eq!(output["result"], "");
    }

    // --- LLM tool-calling tests ---

    use ayas_core::message::ToolCall;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Mock model that returns tool_calls on first call, then a final text response.
    struct MockToolCallingModel {
        call_count: Arc<AtomicUsize>,
        final_response: String,
    }

    #[async_trait]
    impl ChatModel for MockToolCallingModel {
        async fn generate(
            &self,
            _messages: &[Message],
            _options: &CallOptions,
        ) -> ayas_core::error::Result<ChatResult> {
            let count = self.call_count.fetch_add(1, Ordering::Relaxed);
            if count == 0 {
                // First call: return tool call
                Ok(ChatResult {
                    message: Message::AI(AIContent {
                        content: String::new(),
                        tool_calls: vec![ToolCall {
                            id: "call_1".into(),
                            name: "calculator".into(),
                            arguments: json!({"expression": "2+3"}),
                        }],
                        usage: None,
                    }),
                    usage: None,
                })
            } else {
                // Second call: return final text
                Ok(ChatResult {
                    message: Message::AI(AIContent {
                        content: self.final_response.clone(),
                        tool_calls: Vec::new(),
                        usage: None,
                    }),
                    usage: None,
                })
            }
        }

        fn model_name(&self) -> &str {
            "mock-tool-calling-model"
        }
    }

    fn mock_tool_calling_factory(response: &str) -> (GraphModelFactory, Arc<AtomicUsize>) {
        let resp = response.to_string();
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = counter.clone();
        let factory: GraphModelFactory = Arc::new(move |_p, _k, _m| {
            Box::new(MockToolCallingModel {
                call_count: counter_clone.clone(),
                final_response: resp.clone(),
            })
        });
        (factory, counter)
    }

    fn mock_tools_factory() -> GraphToolsFactory {
        Arc::new(|names: &[String]| {
            let mut tools: Vec<Arc<dyn Tool>> = Vec::new();
            if names.contains(&"calculator".to_string()) {
                tools.push(Arc::new(crate::tools::calculator::CalculatorTool));
            }
            if names.contains(&"datetime".to_string()) {
                tools.push(Arc::new(crate::tools::datetime::DateTimeTool));
            }
            tools
        })
    }

    #[tokio::test]
    async fn test_llm_node_tool_calling_loop() {
        let (factory, call_count) = mock_tool_calling_factory("The answer is 5");
        let mut n = node("llm_1", "llm");
        n.config = Some(json!({
            "prompt": "You are a calculator assistant",
            "provider": "gemini",
            "model": "gemini-2.5-flash",
            "tools": ["calculator"],
            "max_tool_iterations": 3
        }));
        let nodes = vec![n];
        let edges = vec![edge("start", "llm_1"), edge("llm_1", "end")];
        let channels = vec![channel("value", "LastValue")];

        let context = GraphBuildContext {
            factory,
            api_keys: ApiKeys {
                gemini_key: Some("test-key".into()),
                ..Default::default()
            },
            research_factory: None,
            tools_factory: Some(mock_tools_factory()),
        };

        let compiled = convert_to_state_graph_with_context(
            &nodes, &edges, &channels, Some(context),
        )
        .unwrap();

        let config = ayas_core::config::RunnableConfig::default();
        let input = json!({"value": "What is 2+3?"});
        let output = compiled.invoke(input, &config).await.unwrap();
        assert_eq!(output["value"], "The answer is 5");
        // Model should have been called twice: once with tool call, once with final response
        assert_eq!(call_count.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn test_llm_node_no_tools_no_loop() {
        let mut n = node("llm_1", "llm");
        n.config = Some(json!({
            "prompt": "Hello",
            "provider": "gemini",
            "model": "gemini-2.5-flash"
        }));
        let nodes = vec![n];
        let edges = vec![edge("start", "llm_1"), edge("llm_1", "end")];
        let channels = vec![channel("value", "LastValue")];

        let context = GraphBuildContext {
            factory: mock_factory("Direct response"),
            api_keys: ApiKeys {
                gemini_key: Some("test-key".into()),
                ..Default::default()
            },
            research_factory: None,
            tools_factory: Some(mock_tools_factory()),
        };

        let compiled = convert_to_state_graph_with_context(
            &nodes, &edges, &channels, Some(context),
        )
        .unwrap();

        let config = ayas_core::config::RunnableConfig::default();
        let input = json!({"value": "Hello"});
        let output = compiled.invoke(input, &config).await.unwrap();
        assert_eq!(output["value"], "Direct response");
    }

    // --- Agent node tests ---

    #[tokio::test]
    async fn test_agent_node_without_context() {
        let mut n = node("agent_1", "agent");
        n.config = Some(json!({
            "system_prompt": "You are a helper",
            "tools": ["calculator"],
            "output_channel": "result"
        }));
        let nodes = vec![n];
        let edges = vec![edge("start", "agent_1"), edge("agent_1", "end")];
        let channels = vec![
            channel("value", "LastValue"),
            channel("result", "LastValue"),
        ];

        let compiled =
            convert_to_state_graph_with_context(&nodes, &edges, &channels, None).unwrap();

        let config = ayas_core::config::RunnableConfig::default();
        let input = json!({"value": "test", "result": ""});
        let output = compiled.invoke(input, &config).await.unwrap();
        assert_eq!(output["result"], "(agent: no context)");
    }

    #[tokio::test]
    async fn test_agent_node_with_mock() {
        let mut n = node("agent_1", "agent");
        n.config = Some(json!({
            "provider": "gemini",
            "model": "gemini-2.5-flash",
            "system_prompt": "You are a helpful assistant",
            "tools": ["calculator"],
            "recursion_limit": 10,
            "input_channel": "value",
            "output_channel": "result"
        }));
        let nodes = vec![n];
        let edges = vec![edge("start", "agent_1"), edge("agent_1", "end")];
        let channels = vec![
            channel("value", "LastValue"),
            channel("result", "LastValue"),
        ];

        // Use the tool-calling factory so agent runs: tool call → tool execution → final response
        let (factory, call_count) = mock_tool_calling_factory("Agent final answer");
        let context = GraphBuildContext {
            factory,
            api_keys: ApiKeys {
                gemini_key: Some("test-key".into()),
                ..Default::default()
            },
            research_factory: None,
            tools_factory: Some(mock_tools_factory()),
        };

        let compiled = convert_to_state_graph_with_context(
            &nodes, &edges, &channels, Some(context),
        )
        .unwrap();

        let config = ayas_core::config::RunnableConfig::default();
        let input = json!({"value": "What is 2+3?", "result": ""});
        let output = compiled.invoke(input, &config).await.unwrap();
        assert_eq!(output["result"], "Agent final answer");
        // Agent graph internally calls model: agent(1st) → tools → agent(2nd: final)
        assert_eq!(call_count.load(Ordering::Relaxed), 2);
    }

    // --- Error edge tests ---

    #[tokio::test]
    async fn test_error_edge_routes_to_error_handler() {
        // Use an LLM node with a failing mock model and on_error edge
        let call_count = Arc::new(AtomicUsize::new(0));
        let cc = call_count.clone();
        let factory: GraphModelFactory = Arc::new(move |_p, _k, _m| {
            Box::new(MockFailingModel {
                fail_count: 100, // always fails
                call_count: cc.clone(),
            })
        });

        let mut fail_node = node("fail_node", "llm");
        fail_node.config = Some(json!({
            "prompt": "test",
            "provider": "gemini",
            "model": "gemini-2.5-flash"
        }));
        let handler = node("handler", "passthrough");

        let nodes = vec![fail_node, handler];
        let edges = vec![
            edge("start", "fail_node"),
            GraphEdgeDto {
                from: "fail_node".into(),
                to: "end".into(),
                condition: None,
                fan_out: false,
                on_error: false,
            },
            GraphEdgeDto {
                from: "fail_node".into(),
                to: "handler".into(),
                condition: None,
                fan_out: false,
                on_error: true,
            },
            edge("handler", "end"),
        ];
        let channels = vec![channel("value", "LastValue")];

        let context = GraphBuildContext {
            factory,
            api_keys: ApiKeys {
                gemini_key: Some("test-key".into()),
                ..Default::default()
            },
            research_factory: None,
            tools_factory: None,
        };

        let compiled = convert_to_state_graph_with_context(&nodes, &edges, &channels, Some(context)).unwrap();
        let config = ayas_core::config::RunnableConfig::default();
        let input = json!({"value": "test"});
        let output = compiled.invoke(input, &config).await.unwrap();
        // The error should be captured in __error and routed to handler (passthrough)
        assert!(output["__error"].as_str().is_some_and(|s| !s.is_empty()));
    }

    #[tokio::test]
    async fn test_no_error_edge_propagates_error() {
        // Without on_error edge, the error should propagate
        let mut fail_node = node("fail_node", "transform");
        fail_node.config = Some(json!({ "expression": "undefined_var.crash()", "output_channel": "value" }));

        let nodes = vec![fail_node];
        let edges = vec![edge("start", "fail_node"), edge("fail_node", "end")];
        let channels = vec![channel("value", "LastValue")];

        let compiled = convert_to_state_graph(&nodes, &edges, &channels).unwrap();
        let config = ayas_core::config::RunnableConfig::default();
        let input = json!({"value": "test"});
        let result = compiled.invoke(input, &config).await;
        assert!(result.is_err());
    }

    // --- Retry tests ---

    /// Mock model that fails N times then succeeds.
    struct MockFailingModel {
        fail_count: usize,
        call_count: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl ChatModel for MockFailingModel {
        async fn generate(
            &self,
            _messages: &[Message],
            _options: &CallOptions,
        ) -> ayas_core::error::Result<ChatResult> {
            let count = self.call_count.fetch_add(1, Ordering::Relaxed);
            if count < self.fail_count {
                Err(ayas_core::error::AyasError::Other(format!(
                    "Intentional failure #{count}"
                )))
            } else {
                Ok(ChatResult {
                    message: Message::AI(AIContent {
                        content: "recovered".into(),
                        tool_calls: Vec::new(),
                        usage: None,
                    }),
                    usage: None,
                })
            }
        }

        fn model_name(&self) -> &str {
            "mock-failing-model"
        }
    }

    #[tokio::test]
    async fn test_retry_succeeds_after_failures() {
        let call_count = Arc::new(AtomicUsize::new(0));
        let cc = call_count.clone();
        let factory: GraphModelFactory = Arc::new(move |_p, _k, _m| {
            Box::new(MockFailingModel {
                fail_count: 2,
                call_count: cc.clone(),
            })
        });

        let mut n = node("llm_1", "llm");
        n.config = Some(json!({
            "prompt": "test",
            "provider": "gemini",
            "model": "gemini-2.5-flash",
            "max_retries": 3
        }));
        let nodes = vec![n];
        let edges = vec![edge("start", "llm_1"), edge("llm_1", "end")];
        let channels = vec![channel("value", "LastValue")];

        let context = GraphBuildContext {
            factory,
            api_keys: ApiKeys {
                gemini_key: Some("test-key".into()),
                ..Default::default()
            },
            research_factory: None,
            tools_factory: None,
        };

        let compiled = convert_to_state_graph_with_context(
            &nodes, &edges, &channels, Some(context),
        )
        .unwrap();

        let config = ayas_core::config::RunnableConfig::default();
        let input = json!({"value": "test"});
        let output = compiled.invoke(input, &config).await.unwrap();
        assert_eq!(output["value"], "recovered");
        // Should have been called 3 times: 2 failures + 1 success
        assert_eq!(call_count.load(Ordering::Relaxed), 3);
    }

    #[tokio::test]
    async fn test_retry_exhausted_returns_error() {
        let call_count = Arc::new(AtomicUsize::new(0));
        let cc = call_count.clone();
        let factory: GraphModelFactory = Arc::new(move |_p, _k, _m| {
            Box::new(MockFailingModel {
                fail_count: 10,
                call_count: cc.clone(),
            })
        });

        let mut n = node("llm_1", "llm");
        n.config = Some(json!({
            "prompt": "test",
            "provider": "gemini",
            "model": "gemini-2.5-flash",
            "max_retries": 2
        }));
        let nodes = vec![n];
        let edges = vec![edge("start", "llm_1"), edge("llm_1", "end")];
        let channels = vec![channel("value", "LastValue")];

        let context = GraphBuildContext {
            factory,
            api_keys: ApiKeys {
                gemini_key: Some("test-key".into()),
                ..Default::default()
            },
            research_factory: None,
            tools_factory: None,
        };

        let compiled = convert_to_state_graph_with_context(
            &nodes, &edges, &channels, Some(context),
        )
        .unwrap();

        let config = ayas_core::config::RunnableConfig::default();
        let input = json!({"value": "test"});
        let result = compiled.invoke(input, &config).await;
        assert!(result.is_err());
        // 1 initial + 2 retries = 3 total attempts
        assert_eq!(call_count.load(Ordering::Relaxed), 3);
    }
}
