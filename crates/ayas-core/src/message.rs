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

/// Binary content (image/file) source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentSource {
    /// Base64-encoded inline data.
    Base64 { media_type: String, data: String },
    /// URL reference (https://, data:, gs://, etc.).
    Url {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    /// Provider-specific file ID.
    FileId { file_id: String },
}

/// Multimodal message content part.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text { text: String },
    Image { source: ContentSource },
    File { source: ContentSource },
}

/// Message content: text-only or multimodal parts.
///
/// Serializes as a plain JSON string for `Text`, or a JSON array for `Parts`.
/// Uses `#[serde(untagged)]` — deserialization tries `Text` first, then `Parts`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Parts(Vec<ContentPart>),
}

impl MessageContent {
    /// Extract the text portions, joined together.
    pub fn text(&self) -> String {
        match self {
            MessageContent::Text(s) => s.clone(),
            MessageContent::Parts(parts) => parts
                .iter()
                .filter_map(|p| match p {
                    ContentPart::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(""),
        }
    }

    /// Returns true if this content contains non-text parts.
    pub fn is_multimodal(&self) -> bool {
        match self {
            MessageContent::Text(_) => false,
            MessageContent::Parts(parts) => {
                parts.iter().any(|p| !matches!(p, ContentPart::Text { .. }))
            }
        }
    }

    /// Get content as parts (text is converted to a single Text part).
    pub fn parts(&self) -> Vec<ContentPart> {
        match self {
            MessageContent::Text(s) => vec![ContentPart::Text { text: s.clone() }],
            MessageContent::Parts(parts) => parts.clone(),
        }
    }
}

impl From<String> for MessageContent {
    fn from(s: String) -> Self {
        MessageContent::Text(s)
    }
}

impl From<&str> for MessageContent {
    fn from(s: &str) -> Self {
        MessageContent::Text(s.to_string())
    }
}

/// A chat message in a conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Message {
    #[serde(rename = "system")]
    System { content: MessageContent },

    #[serde(rename = "user")]
    User { content: MessageContent },

    #[serde(rename = "ai")]
    AI(AIContent),

    #[serde(rename = "tool")]
    Tool {
        content: String,
        tool_call_id: String,
    },
}

impl Message {
    pub fn system(content: impl Into<MessageContent>) -> Self {
        Message::System {
            content: content.into(),
        }
    }

    pub fn user(content: impl Into<MessageContent>) -> Self {
        Message::User {
            content: content.into(),
        }
    }

    pub fn system_with_parts(parts: Vec<ContentPart>) -> Self {
        Message::System {
            content: MessageContent::Parts(parts),
        }
    }

    pub fn user_with_parts(parts: Vec<ContentPart>) -> Self {
        Message::User {
            content: MessageContent::Parts(parts),
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
    ///
    /// For multimodal messages (System/User with `Parts`), returns an empty string.
    /// Use `message_content()` for full access to multimodal content.
    pub fn content(&self) -> &str {
        match self {
            Message::System { content } | Message::User { content } => match content {
                MessageContent::Text(s) => s,
                MessageContent::Parts(_) => "",
            },
            Message::AI(ai) => &ai.content,
            Message::Tool { content, .. } => content,
        }
    }

    /// Access the full `MessageContent` for System/User messages.
    pub fn message_content(&self) -> Option<&MessageContent> {
        match self {
            Message::System { content } | Message::User { content } => Some(content),
            _ => None,
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

    // --- New multimodal tests ---

    #[test]
    fn message_content_text_serde_roundtrip() {
        let mc = MessageContent::Text("hello".into());
        let json = serde_json::to_string(&mc).unwrap();
        assert_eq!(json, r#""hello""#);
        let deserialized: MessageContent = serde_json::from_str(&json).unwrap();
        assert_eq!(mc, deserialized);
    }

    #[test]
    fn message_content_parts_serde_roundtrip() {
        let mc = MessageContent::Parts(vec![
            ContentPart::Text {
                text: "describe this".into(),
            },
            ContentPart::Image {
                source: ContentSource::Url {
                    url: "https://example.com/img.png".into(),
                    detail: Some("high".into()),
                },
            },
        ]);
        let json = serde_json::to_string(&mc).unwrap();
        let deserialized: MessageContent = serde_json::from_str(&json).unwrap();
        assert_eq!(mc, deserialized);
    }

    #[test]
    fn content_source_base64_serde() {
        let src = ContentSource::Base64 {
            media_type: "image/png".into(),
            data: "iVBOR...".into(),
        };
        let json = serde_json::to_string(&src).unwrap();
        assert!(json.contains(r#""type":"base64""#));
        let deserialized: ContentSource = serde_json::from_str(&json).unwrap();
        assert_eq!(src, deserialized);
    }

    #[test]
    fn content_source_url_serde() {
        let src = ContentSource::Url {
            url: "https://example.com/img.png".into(),
            detail: None,
        };
        let json = serde_json::to_string(&src).unwrap();
        assert!(json.contains(r#""type":"url""#));
        assert!(!json.contains("detail"));
        let deserialized: ContentSource = serde_json::from_str(&json).unwrap();
        assert_eq!(src, deserialized);
    }

    #[test]
    fn content_source_file_id_serde() {
        let src = ContentSource::FileId {
            file_id: "file-abc123".into(),
        };
        let json = serde_json::to_string(&src).unwrap();
        assert!(json.contains(r#""type":"file_id""#));
        let deserialized: ContentSource = serde_json::from_str(&json).unwrap();
        assert_eq!(src, deserialized);
    }

    #[test]
    fn content_part_text_serde() {
        let part = ContentPart::Text {
            text: "hello".into(),
        };
        let json = serde_json::to_string(&part).unwrap();
        assert!(json.contains(r#""type":"text""#));
        let deserialized: ContentPart = serde_json::from_str(&json).unwrap();
        assert_eq!(part, deserialized);
    }

    #[test]
    fn content_part_image_serde() {
        let part = ContentPart::Image {
            source: ContentSource::Base64 {
                media_type: "image/jpeg".into(),
                data: "base64data".into(),
            },
        };
        let json = serde_json::to_string(&part).unwrap();
        assert!(json.contains(r#""type":"image""#));
        let deserialized: ContentPart = serde_json::from_str(&json).unwrap();
        assert_eq!(part, deserialized);
    }

    #[test]
    fn content_part_file_serde() {
        let part = ContentPart::File {
            source: ContentSource::FileId {
                file_id: "file-xyz".into(),
            },
        };
        let json = serde_json::to_string(&part).unwrap();
        assert!(json.contains(r#""type":"file""#));
        let deserialized: ContentPart = serde_json::from_str(&json).unwrap();
        assert_eq!(part, deserialized);
    }

    #[test]
    fn message_content_text_method() {
        let text = MessageContent::Text("hello".into());
        assert_eq!(text.text(), "hello");

        let parts = MessageContent::Parts(vec![
            ContentPart::Text {
                text: "hello ".into(),
            },
            ContentPart::Image {
                source: ContentSource::Url {
                    url: "img.png".into(),
                    detail: None,
                },
            },
            ContentPart::Text {
                text: "world".into(),
            },
        ]);
        assert_eq!(parts.text(), "hello world");
    }

    #[test]
    fn message_content_is_multimodal() {
        assert!(!MessageContent::Text("hello".into()).is_multimodal());

        let text_only_parts = MessageContent::Parts(vec![ContentPart::Text {
            text: "hello".into(),
        }]);
        assert!(!text_only_parts.is_multimodal());

        let multimodal = MessageContent::Parts(vec![
            ContentPart::Text {
                text: "hello".into(),
            },
            ContentPart::Image {
                source: ContentSource::Url {
                    url: "img.png".into(),
                    detail: None,
                },
            },
        ]);
        assert!(multimodal.is_multimodal());
    }

    #[test]
    fn message_content_parts_method() {
        let text = MessageContent::Text("hello".into());
        let parts = text.parts();
        assert_eq!(parts.len(), 1);
        assert!(matches!(&parts[0], ContentPart::Text { text } if text == "hello"));

        let multi = MessageContent::Parts(vec![
            ContentPart::Text {
                text: "hello".into(),
            },
            ContentPart::Image {
                source: ContentSource::Url {
                    url: "img.png".into(),
                    detail: None,
                },
            },
        ]);
        assert_eq!(multi.parts().len(), 2);
    }

    #[test]
    fn user_with_parts_construction_and_serde() {
        let msg = Message::user_with_parts(vec![
            ContentPart::Text {
                text: "What is this?".into(),
            },
            ContentPart::Image {
                source: ContentSource::Url {
                    url: "https://example.com/img.png".into(),
                    detail: Some("auto".into()),
                },
            },
        ]);
        let json = serde_json::to_string(&msg).unwrap();
        let deserialized: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, deserialized);
        // content() returns empty for multimodal
        assert_eq!(msg.content(), "");
        // message_content gives full access
        let mc = msg.message_content().unwrap();
        assert!(mc.is_multimodal());
        assert_eq!(mc.text(), "What is this?");
    }

    #[test]
    fn existing_json_compat_user_text() {
        let json = r#"{"type":"user","content":"hello"}"#;
        let msg: Message = serde_json::from_str(json).unwrap();
        assert_eq!(msg.content(), "hello");
        match msg {
            Message::User { content } => {
                assert_eq!(content, MessageContent::Text("hello".into()));
            }
            _ => panic!("expected User"),
        }
    }

    #[test]
    fn new_json_user_parts() {
        let json = r#"{"type":"user","content":[{"type":"text","text":"describe"},{"type":"image","source":{"type":"url","url":"https://example.com/img.png"}}]}"#;
        let msg: Message = serde_json::from_str(json).unwrap();
        assert_eq!(msg.content(), "");
        let mc = msg.message_content().unwrap();
        assert!(mc.is_multimodal());
        assert_eq!(mc.text(), "describe");
    }

    #[test]
    fn message_content_from_string() {
        let mc: MessageContent = "hello".into();
        assert_eq!(mc, MessageContent::Text("hello".into()));

        let mc: MessageContent = String::from("world").into();
        assert_eq!(mc, MessageContent::Text("world".into()));
    }

    #[test]
    fn message_content_accessor() {
        let msg = Message::system("sys");
        assert!(msg.message_content().is_some());

        let msg = Message::ai("ai_msg");
        assert!(msg.message_content().is_none());

        let msg = Message::tool("result", "id");
        assert!(msg.message_content().is_none());
    }
}
