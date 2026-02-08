use ayas_core::message::Message;
use ayas_core::model::{CallOptions, ChatModel};
use ayas_core::tool::ToolDefinition;
use ayas_llm::claude::ClaudeChatModel;
use ayas_llm::gemini::GeminiChatModel;
use ayas_llm::openai::OpenAIChatModel;

fn calculator_tool() -> ToolDefinition {
    ToolDefinition {
        name: "calculator".into(),
        description: "Calculate math expressions".into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "expression": {
                    "type": "string",
                    "description": "The math expression to calculate"
                }
            },
            "required": ["expression"]
        }),
    }
}

// ---------------------------------------------------------------------------
// Gemini integration tests
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn gemini_chat_basic() {
    let key = std::env::var("GEMINI_API_KEY").expect("GEMINI_API_KEY required");
    let model = GeminiChatModel::new(key, "gemini-2.0-flash".into());
    let msgs = vec![Message::user("Say 'hello' and nothing else")];
    let result = model
        .generate(&msgs, &CallOptions::default())
        .await
        .unwrap();
    assert!(!result.message.content().is_empty());
}

#[tokio::test]
#[ignore]
async fn gemini_with_tools() {
    let key = std::env::var("GEMINI_API_KEY").expect("GEMINI_API_KEY required");
    let model = GeminiChatModel::new(key, "gemini-2.0-flash".into());
    let msgs = vec![Message::user("What is 2+2? Use the calculator tool.")];
    let options = CallOptions {
        tools: vec![calculator_tool()],
        ..Default::default()
    };
    let result = model.generate(&msgs, &options).await.unwrap();
    match &result.message {
        Message::AI(ai) => {
            assert!(
                !ai.tool_calls.is_empty(),
                "Expected tool_calls but got none. Content: {}",
                ai.content
            );
            assert_eq!(ai.tool_calls[0].name, "calculator");
        }
        other => panic!("Expected AI message, got: {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Claude integration tests
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn claude_chat_basic() {
    let key = std::env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY required");
    let model = ClaudeChatModel::new(key, "claude-sonnet-4-5-20250929".into());
    let msgs = vec![Message::user("Say 'hello' and nothing else")];
    let result = model
        .generate(&msgs, &CallOptions::default())
        .await
        .unwrap();
    assert!(!result.message.content().is_empty());
}

#[tokio::test]
#[ignore]
async fn claude_with_tools() {
    let key = std::env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY required");
    let model = ClaudeChatModel::new(key, "claude-sonnet-4-5-20250929".into());
    let msgs = vec![Message::user("What is 2+2? Use the calculator tool.")];
    let options = CallOptions {
        tools: vec![calculator_tool()],
        ..Default::default()
    };
    let result = model.generate(&msgs, &options).await.unwrap();
    match &result.message {
        Message::AI(ai) => {
            assert!(
                !ai.tool_calls.is_empty(),
                "Expected tool_calls but got none. Content: {}",
                ai.content
            );
            assert_eq!(ai.tool_calls[0].name, "calculator");
        }
        other => panic!("Expected AI message, got: {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// OpenAI integration tests
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn openai_chat_basic() {
    let key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY required");
    let model = OpenAIChatModel::new(key, "gpt-4o-mini".into());
    let msgs = vec![Message::user("Say 'hello' and nothing else")];
    let result = model
        .generate(&msgs, &CallOptions::default())
        .await
        .unwrap();
    assert!(!result.message.content().is_empty());
}

#[tokio::test]
#[ignore]
async fn openai_with_tools() {
    let key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY required");
    let model = OpenAIChatModel::new(key, "gpt-4o-mini".into());
    let msgs = vec![Message::user("What is 2+2? Use the calculator tool.")];
    let options = CallOptions {
        tools: vec![calculator_tool()],
        ..Default::default()
    };
    let result = model.generate(&msgs, &options).await.unwrap();
    match &result.message {
        Message::AI(ai) => {
            assert!(
                !ai.tool_calls.is_empty(),
                "Expected tool_calls but got none. Content: {}",
                ai.content
            );
            assert_eq!(ai.tool_calls[0].name, "calculator");
        }
        other => panic!("Expected AI message, got: {:?}", other),
    }
}
