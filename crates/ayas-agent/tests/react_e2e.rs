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
        "mock-react-model"
    }
}

/// A simple mock tool.
struct MockTool {
    name: String,
    description: String,
    result: String,
}

impl MockTool {
    fn new(name: &str, description: &str, result: &str) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            result: result.into(),
        }
    }
}

#[async_trait]
impl Tool for MockTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name.clone(),
            description: self.description.clone(),
            parameters: json!({
                "type": "object",
                "properties": {},
            }),
        }
    }

    async fn call(&self, _input: Value) -> Result<String> {
        Ok(self.result.clone())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// ReAct basic loop — think-act-observe cycle.
/// The model first issues a tool call (act), the tool returns a result (observe),
/// then the model produces a final answer.
#[tokio::test]
async fn react_basic_think_act_observe() {
    let model = Arc::new(MockChatModel::new(vec![
        // Agent thinks and decides to call calculator
        Message::ai_with_tool_calls(
            "I need to calculate 6+7.",
            vec![ToolCall {
                id: "call_1".into(),
                name: "calculator".into(),
                arguments: json!({"expression": "6 + 7"}),
            }],
        ),
        // After observing the result, agent produces final answer
        Message::ai("The answer is 13."),
    ]));

    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(MockTool::new(
        "calculator",
        "Evaluates arithmetic expressions",
        "Result: 6 + 7 = 13",
    ))];

    let graph = create_react_agent(model, tools).unwrap();
    let config = RunnableConfig::default();
    let input = json!({"messages": [{"type": "user", "content": "What is 6 + 7?"}]});
    let result = graph.invoke(input, &config).await.unwrap();

    let messages = result["messages"].as_array().unwrap();
    // user -> AI(think+tool_call) -> tool_result -> AI(final)
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[0]["content"], "What is 6 + 7?");
    assert!(!messages[1]["tool_calls"].as_array().unwrap().is_empty());
    assert_eq!(messages[2]["type"], "tool");
    assert!(messages[2]["content"].as_str().unwrap().contains("13"));
    assert_eq!(messages[3]["content"], "The answer is 13.");
}

/// ReAct direct answer — model skips tool calling entirely.
#[tokio::test]
async fn react_direct_answer_no_tools() {
    let model = Arc::new(MockChatModel::new(vec![Message::ai(
        "Hello! How can I help you today?",
    )]));

    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(MockTool::new(
        "calculator",
        "Evaluates arithmetic expressions",
        "42",
    ))];

    let graph = create_react_agent(model, tools).unwrap();
    let config = RunnableConfig::default();
    let input = json!({"messages": [{"type": "user", "content": "Hello!"}]});
    let result = graph.invoke(input, &config).await.unwrap();

    let messages = result["messages"].as_array().unwrap();
    // user -> AI(direct answer)
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["type"], "user");
    assert_eq!(
        messages[1]["content"],
        "Hello! How can I help you today?"
    );
}

/// ReAct with multiple tools — model chooses between different tools.
#[tokio::test]
async fn react_multiple_tools_selection() {
    let model = Arc::new(MockChatModel::new(vec![
        // First, the agent uses the search tool
        Message::ai_with_tool_calls(
            "Let me search for that.",
            vec![ToolCall {
                id: "call_1".into(),
                name: "search".into(),
                arguments: json!({"query": "population of Tokyo"}),
            }],
        ),
        // Then, the agent uses the calculator tool
        Message::ai_with_tool_calls(
            "Now let me calculate the percentage.",
            vec![ToolCall {
                id: "call_2".into(),
                name: "calculator".into(),
                arguments: json!({"expression": "14000000 / 126000000 * 100"}),
            }],
        ),
        // Final answer incorporating both results
        Message::ai("Tokyo has about 14 million people, which is roughly 11.1% of Japan's population."),
    ]));

    let tools: Vec<Arc<dyn Tool>> = vec![
        Arc::new(MockTool::new(
            "search",
            "Search the web",
            "Tokyo population: approximately 14 million",
        )),
        Arc::new(MockTool::new(
            "calculator",
            "Evaluates arithmetic expressions",
            "11.111",
        )),
    ];

    let graph = create_react_agent(model, tools).unwrap();
    let config = RunnableConfig::default();
    let input = json!({
        "messages": [{"type": "user", "content": "What percentage of Japan's population lives in Tokyo?"}]
    });
    let result = graph.invoke(input, &config).await.unwrap();

    let messages = result["messages"].as_array().unwrap();
    // user -> AI(search call) -> tool(search) -> AI(calc call) -> tool(calc) -> AI(final)
    assert_eq!(messages.len(), 6);

    // Verify the agent called search first
    assert_eq!(messages[1]["tool_calls"][0]["name"], "search");
    assert_eq!(messages[2]["type"], "tool");
    assert!(messages[2]["content"]
        .as_str()
        .unwrap()
        .contains("14 million"));

    // Verify the agent called calculator second
    assert_eq!(messages[3]["tool_calls"][0]["name"], "calculator");
    assert_eq!(messages[4]["type"], "tool");
    assert!(messages[4]["content"].as_str().unwrap().contains("11.111"));

    // Final answer
    assert!(messages[5]["content"]
        .as_str()
        .unwrap()
        .contains("11.1%"));
}

/// ReAct with parallel tool calls in a single turn.
#[tokio::test]
async fn react_parallel_tool_calls() {
    let model = Arc::new(MockChatModel::new(vec![
        Message::ai_with_tool_calls(
            "I'll search for both topics at once.",
            vec![
                ToolCall {
                    id: "call_1".into(),
                    name: "search".into(),
                    arguments: json!({"query": "Rust"}),
                },
                ToolCall {
                    id: "call_2".into(),
                    name: "search".into(),
                    arguments: json!({"query": "Python"}),
                },
            ],
        ),
        Message::ai("Rust is a systems language, Python is a scripting language."),
    ]));

    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(MockTool::new(
        "search",
        "Search the web",
        "Language info",
    ))];

    let graph = create_react_agent(model, tools).unwrap();
    let config = RunnableConfig::default();
    let input = json!({"messages": [{"type": "user", "content": "Compare Rust and Python"}]});
    let result = graph.invoke(input, &config).await.unwrap();

    let messages = result["messages"].as_array().unwrap();
    // user -> AI(2 tool calls) -> tool1 -> tool2 -> AI(final)
    assert_eq!(messages.len(), 5);
    assert_eq!(
        messages[1]["tool_calls"].as_array().unwrap().len(),
        2,
        "Should have 2 parallel tool calls"
    );
    assert_eq!(messages[2]["type"], "tool");
    assert_eq!(messages[3]["type"], "tool");
    assert!(messages[4]["content"]
        .as_str()
        .unwrap()
        .contains("Rust"));
}
