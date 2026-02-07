use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use ayas_core::config::RunnableConfig;
use ayas_core::error::Result;
use ayas_core::message::{Message, ToolCall};
use ayas_core::model::{CallOptions, ChatModel, ChatResult};
use ayas_core::runnable::Runnable;
use ayas_core::tool::{Tool, ToolDefinition};

use ayas_agent::react::create_react_agent;

// --- Mock ChatModel that returns tool calls on first call, final answer on second ---

struct MockReActModel {
    call_count: AtomicUsize,
}

impl MockReActModel {
    fn new() -> Self {
        Self {
            call_count: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl ChatModel for MockReActModel {
    async fn generate(&self, _messages: &[Message], _options: &CallOptions) -> Result<ChatResult> {
        let count = self.call_count.fetch_add(1, Ordering::Relaxed);
        if count == 0 {
            // First call: request tool use
            Ok(ChatResult {
                message: Message::ai_with_tool_calls(
                    "",
                    vec![ToolCall {
                        id: "call_1".into(),
                        name: "calculator".into(),
                        arguments: json!({"expression": "6 + 7"}),
                    }],
                ),
                usage: None,
            })
        } else {
            // Second call: final answer
            Ok(ChatResult {
                message: Message::ai("The answer is 13."),
                usage: None,
            })
        }
    }

    fn model_name(&self) -> &str {
        "mock-react-model"
    }
}

// --- Mock ChatModel that returns final answer immediately (no tool calls) ---

struct MockDirectModel;

#[async_trait]
impl ChatModel for MockDirectModel {
    async fn generate(&self, _messages: &[Message], _options: &CallOptions) -> Result<ChatResult> {
        Ok(ChatResult {
            message: Message::ai("Direct answer without tools."),
            usage: None,
        })
    }

    fn model_name(&self) -> &str {
        "mock-direct-model"
    }
}

// --- Mock tool ---

struct MockCalculator;

#[async_trait]
impl Tool for MockCalculator {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "calculator".into(),
            description: "Evaluates arithmetic expressions".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "expression": {"type": "string"}
                },
                "required": ["expression"]
            }),
        }
    }

    async fn call(&self, input: Value) -> Result<String> {
        let expr = input["expression"].as_str().unwrap_or("?");
        // Simple mock: always return "13"
        Ok(format!("Result: {expr} = 13"))
    }
}

/// Full ReAct cycle: user -> agent (tool call) -> tool -> agent (final answer) -> END
#[tokio::test]
async fn react_agent_full_cycle() {
    let model: Arc<dyn ChatModel> = Arc::new(MockReActModel::new());
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(MockCalculator)];

    let graph = create_react_agent(model, tools).unwrap();
    let config = RunnableConfig::default();

    let input = json!({
        "messages": [
            {"type": "user", "content": "What is 6 + 7?"}
        ]
    });

    let result = graph.invoke(input, &config).await.unwrap();

    let messages = result["messages"].as_array().unwrap();
    // 1: user message
    // 2: AI message with tool_calls
    // 3: tool result message
    // 4: AI final answer
    assert_eq!(messages.len(), 4);

    // First message is user
    assert_eq!(messages[0]["content"], "What is 6 + 7?");

    // Second message is AI with tool calls
    assert!(!messages[1]["tool_calls"].as_array().unwrap().is_empty());

    // Third message is tool result
    assert_eq!(messages[2]["type"], "tool");
    assert!(messages[2]["content"].as_str().unwrap().contains("13"));

    // Fourth message is final AI answer
    assert_eq!(messages[3]["type"], "ai");
    assert!(messages[3]["content"].as_str().unwrap().contains("13"));
}

/// Direct answer without tool calls: user -> agent (final) -> END
#[tokio::test]
async fn react_agent_direct_answer() {
    let model: Arc<dyn ChatModel> = Arc::new(MockDirectModel);
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(MockCalculator)];

    let graph = create_react_agent(model, tools).unwrap();
    let config = RunnableConfig::default();

    let input = json!({
        "messages": [
            {"type": "user", "content": "Hello!"}
        ]
    });

    let result = graph.invoke(input, &config).await.unwrap();

    let messages = result["messages"].as_array().unwrap();
    // 1: user message
    // 2: AI direct answer
    assert_eq!(messages.len(), 2);
    assert!(messages[1]["content"]
        .as_str()
        .unwrap()
        .contains("Direct answer"));
}

/// Multiple tool calls in a single turn.
#[tokio::test]
async fn react_agent_multiple_tool_calls() {
    struct MultiToolModel(AtomicUsize);

    #[async_trait]
    impl ChatModel for MultiToolModel {
        async fn generate(
            &self,
            _messages: &[Message],
            _options: &CallOptions,
        ) -> Result<ChatResult> {
            let count = self.0.fetch_add(1, Ordering::Relaxed);
            if count == 0 {
                Ok(ChatResult {
                    message: Message::ai_with_tool_calls(
                        "",
                        vec![
                            ToolCall {
                                id: "call_1".into(),
                                name: "calculator".into(),
                                arguments: json!({"expression": "1+1"}),
                            },
                            ToolCall {
                                id: "call_2".into(),
                                name: "calculator".into(),
                                arguments: json!({"expression": "2+2"}),
                            },
                        ],
                    ),
                    usage: None,
                })
            } else {
                Ok(ChatResult {
                    message: Message::ai("Both results received."),
                    usage: None,
                })
            }
        }

        fn model_name(&self) -> &str {
            "multi-tool-model"
        }
    }

    let model: Arc<dyn ChatModel> = Arc::new(MultiToolModel(AtomicUsize::new(0)));
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(MockCalculator)];

    let graph = create_react_agent(model, tools).unwrap();
    let config = RunnableConfig::default();

    let input = json!({
        "messages": [{"type": "user", "content": "Calculate both"}]
    });

    let result = graph.invoke(input, &config).await.unwrap();
    let messages = result["messages"].as_array().unwrap();

    // 1: user, 2: AI (2 tool calls), 3: tool result 1, 4: tool result 2, 5: AI final
    assert_eq!(messages.len(), 5);
    assert_eq!(messages[2]["type"], "tool");
    assert_eq!(messages[3]["type"], "tool");
}
