use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use ayas_core::config::RunnableConfig;
use ayas_core::error::{AyasError, Result, ToolError};
use ayas_core::message::{Message, ToolCall};
use ayas_core::model::{CallOptions, ChatModel, ChatResult};
use ayas_core::runnable::Runnable;
use ayas_core::tool::{Tool, ToolDefinition};

use ayas_agent::tool_calling::create_tool_calling_agent;

// ---------------------------------------------------------------------------
// Mock helpers
// ---------------------------------------------------------------------------

/// A mock chat model that returns a pre-configured sequence of responses.
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
    async fn generate(&self, _messages: &[Message], _options: &CallOptions) -> Result<ChatResult> {
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

/// A mock chat model that captures the messages it receives.
struct CapturingChatModel {
    responses: Vec<Message>,
    call_count: AtomicUsize,
    captured: std::sync::Mutex<Vec<Vec<Message>>>,
}

impl CapturingChatModel {
    fn new(responses: Vec<Message>) -> Self {
        Self {
            responses,
            call_count: AtomicUsize::new(0),
            captured: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn captured_calls(&self) -> Vec<Vec<Message>> {
        self.captured.lock().unwrap().clone()
    }
}

#[async_trait]
impl ChatModel for CapturingChatModel {
    async fn generate(&self, messages: &[Message], _options: &CallOptions) -> Result<ChatResult> {
        self.captured
            .lock()
            .unwrap()
            .push(messages.to_vec());
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
        "mock-capturing-model"
    }
}

/// A simple mock tool that always succeeds with a fixed result.
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

    async fn call(&self, _input: Value) -> Result<String> {
        Ok(self.result.clone())
    }
}

/// A mock tool that returns an error.
struct ErrorTool {
    name: String,
}

#[async_trait]
impl Tool for ErrorTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name.clone(),
            description: "Always fails".into(),
            parameters: json!({"type": "object", "properties": {}}),
        }
    }

    async fn call(&self, _input: Value) -> Result<String> {
        Err(AyasError::Tool(ToolError::ExecutionFailed(
            "tool exploded".into(),
        )))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// 1. Simple Q&A — agent responds directly without calling any tools.
#[tokio::test]
async fn simple_qa_no_tool_calls() {
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
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["type"], "user");
    assert_eq!(messages[1]["content"], "The answer is 42.");
}

/// 2. Single tool call — agent calls one tool, gets result, then responds.
#[tokio::test]
async fn single_tool_call_then_response() {
    let model = Arc::new(MockChatModel::new(vec![
        Message::ai_with_tool_calls(
            "",
            vec![ToolCall {
                id: "call_1".into(),
                name: "calculator".into(),
                arguments: json!({"expression": "2+2"}),
            }],
        ),
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

/// 3. Multi-tool sequential — agent calls tool A, then tool B, then responds.
#[tokio::test]
async fn multi_tool_sequential() {
    let model = Arc::new(MockChatModel::new(vec![
        Message::ai_with_tool_calls(
            "",
            vec![ToolCall {
                id: "call_1".into(),
                name: "search".into(),
                arguments: json!({"query": "weather"}),
            }],
        ),
        Message::ai_with_tool_calls(
            "",
            vec![ToolCall {
                id: "call_2".into(),
                name: "calculator".into(),
                arguments: json!({"expression": "25+7"}),
            }],
        ),
        Message::ai("The weather is sunny and the sum is 32."),
    ]));

    let tools: Vec<Arc<dyn Tool>> = vec![
        Arc::new(MockTool {
            name: "search".into(),
            result: "sunny, 25°C".into(),
        }),
        Arc::new(MockTool {
            name: "calculator".into(),
            result: "32".into(),
        }),
    ];

    let graph = create_tool_calling_agent(model, tools, None).unwrap();
    let config = RunnableConfig::default();
    let input = json!({"messages": [{"type": "user", "content": "Weather and math?"}]});
    let result = graph.invoke(input, &config).await.unwrap();

    let messages = result["messages"].as_array().unwrap();
    // user -> AI(tc1) -> tool1 -> AI(tc2) -> tool2 -> AI(final)
    assert_eq!(messages.len(), 6);
    assert_eq!(messages[1]["tool_calls"][0]["name"], "search");
    assert_eq!(messages[2]["content"], "sunny, 25°C");
    assert_eq!(messages[3]["tool_calls"][0]["name"], "calculator");
    assert_eq!(messages[4]["content"], "32");
    assert_eq!(
        messages[5]["content"],
        "The weather is sunny and the sum is 32."
    );
}

/// 4. Parallel tool calls — agent returns multiple tool_calls in one response.
#[tokio::test]
async fn parallel_tool_calls() {
    let model = Arc::new(MockChatModel::new(vec![
        Message::ai_with_tool_calls(
            "",
            vec![
                ToolCall {
                    id: "call_a".into(),
                    name: "search".into(),
                    arguments: json!({"query": "rust"}),
                },
                ToolCall {
                    id: "call_b".into(),
                    name: "calculator".into(),
                    arguments: json!({"expression": "1+1"}),
                },
            ],
        ),
        Message::ai("Both results are in."),
    ]));

    let tools: Vec<Arc<dyn Tool>> = vec![
        Arc::new(MockTool {
            name: "search".into(),
            result: "Rust is a language".into(),
        }),
        Arc::new(MockTool {
            name: "calculator".into(),
            result: "2".into(),
        }),
    ];

    let graph = create_tool_calling_agent(model, tools, None).unwrap();
    let config = RunnableConfig::default();
    let input = json!({"messages": [{"type": "user", "content": "Search and calculate"}]});
    let result = graph.invoke(input, &config).await.unwrap();

    let messages = result["messages"].as_array().unwrap();
    // user -> AI(2 tool calls) -> tool1 -> tool2 -> AI(final)
    assert_eq!(messages.len(), 5);
    assert_eq!(messages[2]["type"], "tool");
    assert_eq!(messages[3]["type"], "tool");
    assert_eq!(messages[4]["content"], "Both results are in.");
}

/// 5. Tool error handling — tool returns an error, graph propagates it.
#[tokio::test]
async fn tool_error_propagates() {
    let model = Arc::new(MockChatModel::new(vec![
        Message::ai_with_tool_calls(
            "",
            vec![ToolCall {
                id: "call_err".into(),
                name: "failing_tool".into(),
                arguments: json!({}),
            }],
        ),
    ]));

    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(ErrorTool {
        name: "failing_tool".into(),
    })];

    let graph = create_tool_calling_agent(model, tools, None).unwrap();
    let config = RunnableConfig::default();
    let input = json!({"messages": [{"type": "user", "content": "Do something"}]});
    let result = graph.invoke(input, &config).await;

    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("tool exploded"),
        "Expected error about tool failure, got: {err_msg}"
    );
}

/// 6. System prompt influence — verify system prompt is included in the first call.
#[tokio::test]
async fn system_prompt_is_prepended() {
    let model = Arc::new(CapturingChatModel::new(vec![Message::ai(
        "I am helpful.",
    )]));
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(MockTool {
        name: "noop".into(),
        result: "ok".into(),
    })];

    let graph = create_tool_calling_agent(
        model.clone(),
        tools,
        Some("You are a helpful assistant.".into()),
    )
    .unwrap();
    let config = RunnableConfig::default();
    let input = json!({"messages": [{"type": "user", "content": "Hello"}]});
    let result = graph.invoke(input, &config).await.unwrap();

    let messages = result["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[1]["content"], "I am helpful.");

    // Verify the model saw a system message
    let calls = model.captured_calls();
    assert!(!calls.is_empty());
    let first_call_messages = &calls[0];
    assert!(
        first_call_messages
            .iter()
            .any(|m| matches!(m, Message::System { .. })),
        "System message should be present in model input"
    );
}

/// 7. Large conversation — 10+ turn conversation with interleaved tool calls.
#[tokio::test]
async fn large_conversation_many_turns() {
    // Build 5 rounds of tool-call + final answer = 11 model responses
    let mut responses = Vec::new();
    for i in 0..5 {
        responses.push(Message::ai_with_tool_calls(
            "",
            vec![ToolCall {
                id: format!("call_{i}"),
                name: "search".into(),
                arguments: json!({"query": format!("query_{i}")}),
            }],
        ));
    }
    responses.push(Message::ai("All done after many rounds."));

    let model = Arc::new(MockChatModel::new(responses));
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(MockTool {
        name: "search".into(),
        result: "result".into(),
    })];

    let graph = create_tool_calling_agent(model, tools, None).unwrap();
    let config = RunnableConfig::default();
    let input = json!({"messages": [{"type": "user", "content": "Long conversation"}]});
    let result = graph.invoke(input, &config).await.unwrap();

    let messages = result["messages"].as_array().unwrap();
    // 1 user + 5*(AI+tool) + 1 AI(final) = 12
    assert_eq!(messages.len(), 12);
    assert_eq!(messages[11]["content"], "All done after many rounds.");

    // Verify all intermediate messages alternate correctly
    for i in 0..5 {
        let ai_idx = 1 + i * 2;
        let tool_idx = 2 + i * 2;
        assert!(
            !messages[ai_idx]["tool_calls"]
                .as_array()
                .unwrap()
                .is_empty(),
            "Turn {i}: AI message should have tool calls"
        );
        assert_eq!(
            messages[tool_idx]["type"], "tool",
            "Turn {i}: Should be a tool result"
        );
    }
}

/// 8. No tools available — agent with empty tool list responds directly.
#[tokio::test]
async fn no_tools_available() {
    let model = Arc::new(MockChatModel::new(vec![Message::ai(
        "No tools needed.",
    )]));
    let tools: Vec<Arc<dyn Tool>> = vec![];

    let graph = create_tool_calling_agent(model, tools, None).unwrap();
    let config = RunnableConfig::default();
    let input = json!({"messages": [{"type": "user", "content": "Hello"}]});
    let result = graph.invoke(input, &config).await.unwrap();

    let messages = result["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[1]["content"], "No tools needed.");
}

/// 9. Tool not found — agent calls a tool name not in the registry.
#[tokio::test]
async fn tool_not_found_error() {
    let model = Arc::new(MockChatModel::new(vec![
        Message::ai_with_tool_calls(
            "",
            vec![ToolCall {
                id: "call_ghost".into(),
                name: "nonexistent_tool".into(),
                arguments: json!({}),
            }],
        ),
    ]));

    // Only register "calculator", but the model calls "nonexistent_tool"
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(MockTool {
        name: "calculator".into(),
        result: "42".into(),
    })];

    let graph = create_tool_calling_agent(model, tools, None).unwrap();
    let config = RunnableConfig::default();
    let input = json!({"messages": [{"type": "user", "content": "Use a tool"}]});
    let result = graph.invoke(input, &config).await;

    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("nonexistent_tool") || err_msg.contains("not found"),
        "Expected tool-not-found error, got: {err_msg}"
    );
}

/// 10. Max iterations — agent loops tool calls and hits recursion limit.
#[tokio::test]
async fn recursion_limit_exceeded() {
    // Model always returns tool calls (never gives a final answer)
    let mut responses = Vec::new();
    for i in 0..50 {
        responses.push(Message::ai_with_tool_calls(
            "",
            vec![ToolCall {
                id: format!("call_{i}"),
                name: "search".into(),
                arguments: json!({"query": "infinite"}),
            }],
        ));
    }

    let model = Arc::new(MockChatModel::new(responses));
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(MockTool {
        name: "search".into(),
        result: "found".into(),
    })];

    let graph = create_tool_calling_agent(model, tools, None).unwrap();
    // Set a low recursion limit to trigger the guard
    let config = RunnableConfig::default().with_recursion_limit(5);
    let input = json!({"messages": [{"type": "user", "content": "Loop forever"}]});
    let result = graph.invoke(input, &config).await;

    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("Recursion limit") || err_msg.contains("recursion"),
        "Expected recursion limit error, got: {err_msg}"
    );
}

/// 11. Multiple user messages in initial input.
#[tokio::test]
async fn multiple_user_messages_in_input() {
    let model = Arc::new(MockChatModel::new(vec![Message::ai(
        "Got both messages.",
    )]));
    let tools: Vec<Arc<dyn Tool>> = vec![];

    let graph = create_tool_calling_agent(model, tools, None).unwrap();
    let config = RunnableConfig::default();
    let input = json!({
        "messages": [
            {"type": "user", "content": "First question"},
            {"type": "ai", "content": "First answer", "tool_calls": [], "usage": null},
            {"type": "user", "content": "Follow-up question"}
        ]
    });
    let result = graph.invoke(input, &config).await.unwrap();

    let messages = result["messages"].as_array().unwrap();
    // 3 initial + 1 AI response
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[3]["content"], "Got both messages.");
}

/// 12. System prompt is not duplicated on second agent call.
#[tokio::test]
async fn system_prompt_not_duplicated() {
    let model = Arc::new(CapturingChatModel::new(vec![
        Message::ai_with_tool_calls(
            "",
            vec![ToolCall {
                id: "call_1".into(),
                name: "search".into(),
                arguments: json!({}),
            }],
        ),
        Message::ai("Final answer."),
    ]));

    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(MockTool {
        name: "search".into(),
        result: "data".into(),
    })];

    let graph = create_tool_calling_agent(
        model.clone(),
        tools,
        Some("System prompt here.".into()),
    )
    .unwrap();
    let config = RunnableConfig::default();
    let input = json!({"messages": [{"type": "user", "content": "Hi"}]});
    let _result = graph.invoke(input, &config).await.unwrap();

    // The model is called twice (once for tool call, once for final answer).
    // On the second call, the system message should still appear exactly once.
    let calls = model.captured_calls();
    assert_eq!(calls.len(), 2);

    for (call_idx, call_msgs) in calls.iter().enumerate() {
        let system_count = call_msgs
            .iter()
            .filter(|m| matches!(m, Message::System { .. }))
            .count();
        assert_eq!(
            system_count, 1,
            "Call {call_idx}: expected exactly 1 system message, got {system_count}"
        );
    }
}
