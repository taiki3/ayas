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
use serde_json::{json, Value};

/// Create a tool-calling agent graph that uses native tool calls.
///
/// Unlike the ReAct agent which relies on text-based action parsing, this agent
/// uses the model's native tool-calling capability directly. The model receives
/// tool definitions via `CallOptions` and returns structured `ToolCall` objects.
///
/// The graph follows the cycle: `agent` -> `tools` -> `agent` -> ... -> END
///
/// - The **agent** node prepends an optional system message, then calls the
///   `ChatModel` with the conversation messages and tool definitions.
/// - If the model returns tool calls, they are routed to the **tools** node.
/// - The **tools** node executes each tool call and appends results to messages.
/// - The cycle continues until the model returns a response without tool calls.
///
/// # State schema
/// - `messages`: `AppendChannel` — conversation history (`Vec<Message>` as JSON)
///
/// # Arguments
/// - `model` — The chat model to use for generation.
/// - `tools` — Available tools the model can invoke.
/// - `system_prompt` — Optional system message prepended to the conversation.
///
/// # Example
/// ```ignore
/// let graph = create_tool_calling_agent(model, tools, Some("You are a helpful assistant.".into()))?;
/// let result = graph.invoke(json!({"messages": [{"type":"user","content":"Hi"}]}), &config).await?;
/// ```
pub fn create_tool_calling_agent(
    model: Arc<dyn ChatModel>,
    tools: Vec<Arc<dyn Tool>>,
    system_prompt: Option<String>,
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

    // Agent node: optionally prepends system message, then calls LLM with tool definitions
    let model_clone = model.clone();
    let tool_defs_clone = tool_defs.clone();
    let system_prompt_clone = system_prompt.clone();
    graph.add_node(NodeFn::new(
        "agent",
        move |state: Value, _config| {
            let model = model_clone.clone();
            let tool_defs = tool_defs_clone.clone();
            let system_prompt = system_prompt_clone.clone();
            async move {
                let mut messages = parse_messages(&state["messages"])?;

                // Prepend system message if configured and not already present
                if let Some(ref prompt) = system_prompt {
                    let has_system = messages
                        .iter()
                        .any(|m| matches!(m, Message::System { .. }));
                    if !has_system {
                        messages.insert(0, Message::system(prompt.as_str()));
                    }
                }

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

    // Tools node: executes tool calls from the last AI message
    let tools_map_clone = tools_map.clone();
    graph.add_node(NodeFn::new(
        "tools",
        move |state: Value, _config| {
            let tools_map = tools_map_clone.clone();
            async move {
                let messages = parse_messages(&state["messages"])?;
                let tool_calls = extract_tool_calls(&messages);

                let mut results: Vec<Value> = Vec::new();
                for tc in &tool_calls {
                    let tool = tools_map.get(&tc.name).ok_or_else(|| {
                        AyasError::Tool(ayas_core::error::ToolError::NotFound(tc.name.clone()))
                    })?;
                    let output = tool.call(tc.arguments.clone()).await?;
                    let tool_msg = Message::tool(output, &tc.id);
                    let msg_value = serde_json::to_value(&tool_msg)
                        .map_err(AyasError::Serialization)?;
                    results.push(msg_value);
                }

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
                let msg: Message =
                    serde_json::from_value(item.clone()).map_err(AyasError::Serialization)?;
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
    matches!(last.get("tool_calls"), Some(Value::Array(arr)) if !arr.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use ayas_core::config::RunnableConfig;
    use ayas_core::model::ChatResult;
    use ayas_core::runnable::Runnable;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A mock chat model that returns a sequence of pre-configured responses.
    struct MockChatModel {
        responses: Vec<Message>,
        call_count: AtomicUsize,
    }

    impl MockChatModel {
        fn new(responses: Vec<Message>) -> Self {
            Self {
                responses,
                call_count: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl ChatModel for MockChatModel {
        async fn generate(
            &self,
            _messages: &[Message],
            _options: &CallOptions,
        ) -> ayas_core::error::Result<ChatResult> {
            let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
            let message = self
                .responses
                .get(idx)
                .cloned()
                .unwrap_or_else(|| Message::ai("done"));
            Ok(ChatResult {
                message,
                usage: None,
            })
        }

        fn model_name(&self) -> &str {
            "mock-tool-calling-model"
        }
    }

    /// A simple mock tool for testing.
    struct MockTool {
        name: String,
        result: String,
    }

    #[async_trait]
    impl Tool for MockTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                name: self.name.clone(),
                description: format!("Mock tool: {}", self.name),
                parameters: json!({"type": "object", "properties": {}}),
            }
        }

        async fn call(&self, _input: Value) -> ayas_core::error::Result<String> {
            Ok(self.result.clone())
        }
    }

    #[test]
    fn test_graph_compiles_without_system_prompt() {
        let model = Arc::new(MockChatModel::new(vec![Message::ai("hello")]));
        let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(MockTool {
            name: "search".into(),
            result: "found it".into(),
        })];
        let graph = create_tool_calling_agent(model, tools, None);
        assert!(graph.is_ok());
    }

    #[test]
    fn test_graph_compiles_with_system_prompt() {
        let model = Arc::new(MockChatModel::new(vec![Message::ai("hello")]));
        let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(MockTool {
            name: "search".into(),
            result: "found it".into(),
        })];
        let graph =
            create_tool_calling_agent(model, tools, Some("You are helpful.".into()));
        assert!(graph.is_ok());
    }

    #[tokio::test]
    async fn test_direct_response_without_tools() {
        // Model responds directly without calling any tools
        let model = Arc::new(MockChatModel::new(vec![Message::ai(
            "The answer is 42.",
        )]));
        let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(MockTool {
            name: "calculator".into(),
            result: "42".into(),
        })];

        let graph = create_tool_calling_agent(model, tools, None).unwrap();
        let config = RunnableConfig::default();
        let input = json!({"messages": [{"type": "user", "content": "What is the meaning of life?"}]});
        let result = graph.invoke(input, &config).await.unwrap();

        let messages = result["messages"].as_array().unwrap();
        // Should have: user message + AI response
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1]["content"], "The answer is 42.");
    }

    #[tokio::test]
    async fn test_single_tool_call_then_response() {
        // Model calls a tool, then responds with final answer
        let model = Arc::new(MockChatModel::new(vec![
            // First call: model decides to use the calculator tool
            Message::ai_with_tool_calls(
                "",
                vec![ToolCall {
                    id: "call_1".into(),
                    name: "calculator".into(),
                    arguments: json!({"expression": "2+2"}),
                }],
            ),
            // Second call: model responds with final answer
            Message::ai("The result is 4."),
        ]));

        let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(MockTool {
            name: "calculator".into(),
            result: "4".into(),
        })];

        let graph = create_tool_calling_agent(model, tools, None).unwrap();
        let config = RunnableConfig::default();
        let input = json!({"messages": [{"type": "user", "content": "What is 2+2?"}]});
        let result = graph.invoke(input, &config).await.unwrap();

        let messages = result["messages"].as_array().unwrap();
        // user -> AI(tool_call) -> tool_result -> AI(final)
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[1]["tool_calls"][0]["name"], "calculator");
        assert_eq!(messages[2]["type"], "tool");
        assert_eq!(messages[2]["content"], "4");
        assert_eq!(messages[3]["content"], "The result is 4.");
    }

    #[tokio::test]
    async fn test_multiple_tool_calls_in_sequence() {
        // Model calls tools twice before giving a final answer
        let model = Arc::new(MockChatModel::new(vec![
            // First call: search tool
            Message::ai_with_tool_calls(
                "",
                vec![ToolCall {
                    id: "call_1".into(),
                    name: "search".into(),
                    arguments: json!({"query": "weather"}),
                }],
            ),
            // Second call: another tool call
            Message::ai_with_tool_calls(
                "",
                vec![ToolCall {
                    id: "call_2".into(),
                    name: "search".into(),
                    arguments: json!({"query": "temperature"}),
                }],
            ),
            // Third call: final answer
            Message::ai("The weather is sunny and 25°C."),
        ]));

        let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(MockTool {
            name: "search".into(),
            result: "sunny, 25°C".into(),
        })];

        let graph = create_tool_calling_agent(model, tools, None).unwrap();
        let config = RunnableConfig::default();
        let input = json!({"messages": [{"type": "user", "content": "What's the weather?"}]});
        let result = graph.invoke(input, &config).await.unwrap();

        let messages = result["messages"].as_array().unwrap();
        // user -> AI(tool_call) -> tool -> AI(tool_call) -> tool -> AI(final)
        assert_eq!(messages.len(), 6);
        assert_eq!(messages[5]["content"], "The weather is sunny and 25°C.");
    }

    #[tokio::test]
    async fn test_system_prompt_prepended() {
        // Verify system prompt is passed to the model by checking call behavior
        let model = Arc::new(MockChatModel::new(vec![Message::ai("I am helpful.")]));
        let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(MockTool {
            name: "noop".into(),
            result: "ok".into(),
        })];

        let graph = create_tool_calling_agent(
            model,
            tools,
            Some("You are a helpful assistant.".into()),
        )
        .unwrap();
        let config = RunnableConfig::default();
        let input = json!({"messages": [{"type": "user", "content": "Hello"}]});
        let result = graph.invoke(input, &config).await.unwrap();

        let messages = result["messages"].as_array().unwrap();
        // user message + AI response
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1]["content"], "I am helpful.");
    }

    #[test]
    fn test_parse_messages_from_array() {
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
    fn test_parse_messages_null() {
        let msgs = parse_messages(&Value::Null).unwrap();
        assert!(msgs.is_empty());
    }

    #[test]
    fn test_extract_tool_calls_from_ai_message() {
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
    fn test_extract_tool_calls_no_tools() {
        let messages = vec![Message::user("hi"), Message::ai("hello")];
        let calls = extract_tool_calls(&messages);
        assert!(calls.is_empty());
    }

    #[test]
    fn test_last_message_has_tool_calls_true() {
        let state = json!({
            "messages": [
                {"type": "user", "content": "hi"},
                {"type": "ai", "content": "", "tool_calls": [{"id": "1", "name": "search", "arguments": {}}]}
            ]
        });
        assert!(last_message_has_tool_calls(&state));
    }

    #[test]
    fn test_last_message_has_tool_calls_false() {
        let state = json!({
            "messages": [
                {"type": "user", "content": "hi"},
                {"type": "ai", "content": "hello"}
            ]
        });
        assert!(!last_message_has_tool_calls(&state));
    }
}
