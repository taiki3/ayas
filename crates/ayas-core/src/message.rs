use serde::{Deserialize, Serialize};

/// Metadata about token usage from a model call.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageMetadata {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
}

/// A request from the AI to call a tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// Content of an AI message, which may include tool call requests.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AIContent {
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageMetadata>,
}

/// A chat message in a conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Message {
    #[serde(rename = "system")]
    System { content: String },

    #[serde(rename = "user")]
    User { content: String },

    #[serde(rename = "ai")]
    AI(AIContent),

    #[serde(rename = "tool")]
    Tool {
        content: String,
        tool_call_id: String,
    },
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Message::System {
            content: content.into(),
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Message::User {
            content: content.into(),
        }
    }

    pub fn ai(content: impl Into<String>) -> Self {
        Message::AI(AIContent {
            content: content.into(),
            tool_calls: Vec::new(),
            usage: None,
        })
    }

    pub fn ai_with_tool_calls(content: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
        Message::AI(AIContent {
            content: content.into(),
            tool_calls,
            usage: None,
        })
    }

    pub fn tool(content: impl Into<String>, tool_call_id: impl Into<String>) -> Self {
        Message::Tool {
            content: content.into(),
            tool_call_id: tool_call_id.into(),
        }
    }

    /// Extract the text content from any message variant.
    pub fn content(&self) -> &str {
        match self {
            Message::System { content } => content,
            Message::User { content } => content,
            Message::AI(ai) => &ai.content,
            Message::Tool { content, .. } => content,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_message_serde_roundtrip() {
        let msg = Message::system("You are a helpful assistant.");
        let json = serde_json::to_string(&msg).unwrap();
        let deserialized: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, deserialized);
        assert!(json.contains(r#""type":"system"#));
    }

    #[test]
    fn user_message_serde_roundtrip() {
        let msg = Message::user("Hello!");
        let json = serde_json::to_string(&msg).unwrap();
        let deserialized: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, deserialized);
        assert!(json.contains(r#""type":"user"#));
    }

    #[test]
    fn ai_message_serde_roundtrip() {
        let msg = Message::ai("Hi there!");
        let json = serde_json::to_string(&msg).unwrap();
        let deserialized: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, deserialized);
        assert!(json.contains(r#""type":"ai"#));
    }

    #[test]
    fn ai_message_with_tool_calls_serde_roundtrip() {
        let msg = Message::ai_with_tool_calls(
            "",
            vec![
                ToolCall {
                    id: "call_1".into(),
                    name: "calculator".into(),
                    arguments: serde_json::json!({"expression": "2+2"}),
                },
                ToolCall {
                    id: "call_2".into(),
                    name: "web_search".into(),
                    arguments: serde_json::json!({"query": "rust lang"}),
                },
            ],
        );
        let json = serde_json::to_string(&msg).unwrap();
        let deserialized: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, deserialized);
    }

    #[test]
    fn ai_message_with_usage_serde_roundtrip() {
        let msg = Message::AI(AIContent {
            content: "response".into(),
            tool_calls: Vec::new(),
            usage: Some(UsageMetadata {
                input_tokens: 10,
                output_tokens: 20,
                total_tokens: 30,
            }),
        });
        let json = serde_json::to_string(&msg).unwrap();
        let deserialized: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, deserialized);
    }

    #[test]
    fn tool_message_serde_roundtrip() {
        let msg = Message::tool("4", "call_1");
        let json = serde_json::to_string(&msg).unwrap();
        let deserialized: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, deserialized);
        assert!(json.contains(r#""type":"tool"#));
    }

    #[test]
    fn empty_ai_content_omits_optional_fields() {
        let msg = Message::ai("");
        let json = serde_json::to_string(&msg).unwrap();
        assert!(!json.contains("tool_calls"));
        assert!(!json.contains("usage"));
    }

    #[test]
    fn content_accessor() {
        assert_eq!(Message::system("sys").content(), "sys");
        assert_eq!(Message::user("usr").content(), "usr");
        assert_eq!(Message::ai("ai_msg").content(), "ai_msg");
        assert_eq!(Message::tool("result", "id").content(), "result");
    }

    #[test]
    fn deserialize_from_json_string() {
        let json = r#"{"type":"user","content":"test message"}"#;
        let msg: Message = serde_json::from_str(json).unwrap();
        assert_eq!(msg.content(), "test message");
    }
}
