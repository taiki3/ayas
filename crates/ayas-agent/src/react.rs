use std::collections::HashMap;
use std::sync::Arc;

use ayas_core::error::{AyasError, Result};
use ayas_core::message::{AIContent, Message, ToolCall};
use ayas_core::model::{CallOptions, ChatModel};
use ayas_core::tool::{Tool, ToolDefinition};
use ayas_graph::compiled::CompiledStateGraph;
use ayas_graph::constants::END;
use ayas_graph::edge::ConditionalEdge;
use ayas_graph::node::NodeFn;
use ayas_graph::state_graph::StateGraph;
use futures::stream::{self, StreamExt, TryStreamExt};
use serde_json::{json, Value};

/// Create a ReAct-style agent graph.
///
/// The graph follows the cycle: `agent` -> `tools` -> `agent` -> ... -> END
///
/// - The **agent** node calls the `ChatModel` with the conversation messages
///   and available tool definitions.
/// - If the model returns tool calls, they are routed to the **tools** node.
/// - The **tools** node executes each tool call and appends results to messages.
/// - The cycle continues until the model returns a response without tool calls.
///
/// # State schema
/// - `messages`: `AppendChannel` — conversation history (`Vec<Message>` as JSON)
///
/// # Example
/// ```ignore
/// let graph = create_react_agent(model, tools)?;
/// let result = graph.invoke(json!({"messages": [{"type":"user","content":"Hi"}]}), &config).await?;
/// ```
pub fn create_react_agent(
    model: Arc<dyn ChatModel>,
    tools: Vec<Arc<dyn Tool>>,
) -> Result<CompiledStateGraph> {
    let tool_defs: Vec<ToolDefinition> = tools.iter().map(|t| t.definition()).collect();

    // Build tool lookup map
    let tools_map: Arc<HashMap<String, Arc<dyn Tool>>> = Arc::new(
        tools
            .into_iter()
            .map(|t| (t.definition().name, t))
            .collect(),
    );

    let mut graph = StateGraph::new();
    graph.add_append_channel("messages");

    // Agent node: calls LLM with messages + tool definitions
    let model_clone = model.clone();
    let tool_defs_clone = tool_defs.clone();
    graph.add_node(NodeFn::new(
        "agent",
        move |state: Value, _config| {
            let model = model_clone.clone();
            let tool_defs = tool_defs_clone.clone();
            async move {
                let messages = parse_messages(&state["messages"])?;
                let options = CallOptions {
                    tools: tool_defs,
                    ..Default::default()
                };
                let result = model.generate(&messages, &options).await?;
                let msg_value = serde_json::to_value(&result.message)
                    .map_err(AyasError::Serialization)?;
                Ok(json!({"messages": msg_value}))
            }
        },
    ))?;

    // Tools node: executes tool calls from the last AI message in parallel
    let tools_map_clone = tools_map.clone();
    graph.add_node(NodeFn::new(
        "tools",
        move |state: Value, _config| {
            let tools_map = tools_map_clone.clone();
            async move {
                let messages = parse_messages(&state["messages"])?;
                let tool_calls = extract_tool_calls(&messages);
                let concurrency = tool_calls.len().max(1);

                let results: Vec<Value> = stream::iter(tool_calls.into_iter().map(
                    |tc| {
                        let tools_map = tools_map.clone();
                        async move {
                            let tool = tools_map.get(&tc.name).ok_or_else(|| {
                                AyasError::Tool(ayas_core::error::ToolError::NotFound(
                                    tc.name.clone(),
                                ))
                            })?;
                            let output = tool.call(tc.arguments.clone()).await?;
                            let tool_msg = Message::tool(output, &tc.id);
                            serde_json::to_value(&tool_msg).map_err(AyasError::Serialization)
                        }
                    },
                ))
                .buffered(concurrency)
                .try_collect()
                .await?;

                Ok(json!({"messages": results}))
            }
        },
    ))?;

    // Routing: agent -> tools (if tool_calls) or END
    graph.set_entry_point("agent");

    let mut path_map = HashMap::new();
    path_map.insert("tools".to_string(), "tools".to_string());
    path_map.insert("end".to_string(), END.to_string());

    graph.add_conditional_edges(ConditionalEdge::new(
        "agent",
        |state: &Value| {
            if last_message_has_tool_calls(state) {
                "tools".to_string()
            } else {
                "end".to_string()
            }
        },
        Some(path_map),
    ));

    // tools -> agent (cycle back)
    graph.add_edge("tools", "agent");

    graph.compile()
}

/// Parse messages from a JSON array value.
fn parse_messages(value: &Value) -> Result<Vec<Message>> {
    match value {
        Value::Array(arr) => {
            let mut messages = Vec::new();
            for item in arr {
                let msg: Message = serde_json::from_value(item.clone())
                    .map_err(AyasError::Serialization)?;
                messages.push(msg);
            }
            Ok(messages)
        }
        Value::Null => Ok(Vec::new()),
        _ => Err(AyasError::Other(
            "Expected messages to be a JSON array".into(),
        )),
    }
}

/// Extract tool calls from the last AI message.
fn extract_tool_calls(messages: &[Message]) -> Vec<ToolCall> {
    messages
        .last()
        .and_then(|msg| match msg {
            Message::AI(AIContent { tool_calls, .. }) => {
                if tool_calls.is_empty() {
                    None
                } else {
                    Some(tool_calls.clone())
                }
            }
            _ => None,
        })
        .unwrap_or_default()
}

/// Check if the last message in state has tool calls.
fn last_message_has_tool_calls(state: &Value) -> bool {
    let messages = match state.get("messages") {
        Some(Value::Array(arr)) => arr,
        _ => return false,
    };
    let last = match messages.last() {
        Some(v) => v,
        None => return false,
    };
    // Check for tool_calls array that is non-empty
    matches!(last.get("tool_calls"), Some(Value::Array(arr)) if !arr.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_messages_from_array() {
        let val = json!([
            {"type": "user", "content": "hello"},
            {"type": "ai", "content": "hi", "tool_calls": [], "usage": null}
        ]);
        let msgs = parse_messages(&val).unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].content(), "hello");
        assert_eq!(msgs[1].content(), "hi");
    }

    #[test]
    fn parse_messages_null() {
        let msgs = parse_messages(&Value::Null).unwrap();
        assert!(msgs.is_empty());
    }

    #[test]
    fn extract_tool_calls_from_ai_message() {
        let messages = vec![
            Message::user("hi"),
            Message::ai_with_tool_calls(
                "",
                vec![ToolCall {
                    id: "call_1".into(),
                    name: "search".into(),
                    arguments: json!({"q": "rust"}),
                }],
            ),
        ];
        let calls = extract_tool_calls(&messages);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "search");
    }

    #[test]
    fn extract_tool_calls_no_tools() {
        let messages = vec![Message::user("hi"), Message::ai("hello")];
        let calls = extract_tool_calls(&messages);
        assert!(calls.is_empty());
    }

    #[test]
    fn last_message_has_tool_calls_true() {
        let state = json!({
            "messages": [
                {"type": "user", "content": "hi"},
                {"type": "ai", "content": "", "tool_calls": [{"id": "1", "name": "search", "arguments": {}}]}
            ]
        });
        assert!(last_message_has_tool_calls(&state));
    }

    #[test]
    fn last_message_has_tool_calls_false() {
        let state = json!({
            "messages": [
                {"type": "user", "content": "hi"},
                {"type": "ai", "content": "hello"}
            ]
        });
        assert!(!last_message_has_tool_calls(&state));
    }
}
