//! OpenAI Chat Completions API integration.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use ayas_core::error::{AyasError, ModelError, Result};
use ayas_core::message::{
    AIContent, ContentPart, ContentSource, Message, MessageContent, ToolCall, UsageMetadata,
};
use ayas_core::model::{CallOptions, ChatModel, ChatResult};

// ---------------------------------------------------------------------------
// OpenAI Chat Completions API request/response types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct OpenAIRequest {
    pub model: String,
    pub messages: Vec<OpenAIMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<OpenAIToolDef>>,
}

#[derive(Debug, Serialize)]
pub struct OpenAIToolDef {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: OpenAIFunctionDef,
}

#[derive(Debug, Serialize)]
pub struct OpenAIFunctionDef {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct OpenAIMessage {
    pub role: String,
    pub content: OpenAIContent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<OpenAIRespToolCall>>,
}

/// OpenAI content: text-only or multimodal parts array.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum OpenAIContent {
    Text(String),
    Parts(Vec<OpenAIContentPart>),
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum OpenAIContentPart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image_url")]
    ImageUrl { image_url: OpenAIImageUrl },
}

#[derive(Debug, Serialize)]
pub struct OpenAIImageUrl {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct OpenAIResponse {
    pub choices: Vec<OpenAIChoice>,
    pub usage: Option<OpenAIUsage>,
}

#[derive(Debug, Deserialize)]
pub struct OpenAIChoice {
    pub message: OpenAIResponseMessage,
}

#[derive(Debug, Deserialize)]
pub struct OpenAIResponseMessage {
    pub content: Option<String>,
    #[serde(default)]
    pub tool_calls: Option<Vec<OpenAIRespToolCall>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIRespToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: OpenAIRespFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIRespFunction {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Deserialize)]
pub struct OpenAIUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

#[derive(Debug, Deserialize)]
pub struct OpenAIError {
    pub error: OpenAIErrorDetail,
}

#[derive(Debug, Deserialize)]
pub struct OpenAIErrorDetail {
    pub message: String,
}

// ---------------------------------------------------------------------------
// Conversion helpers
// ---------------------------------------------------------------------------

pub fn message_content_to_openai(mc: &MessageContent) -> OpenAIContent {
    match mc {
        MessageContent::Text(s) => OpenAIContent::Text(s.clone()),
        MessageContent::Parts(parts) => {
            OpenAIContent::Parts(parts.iter().map(content_part_to_openai).collect())
        }
    }
}

pub fn content_part_to_openai(part: &ContentPart) -> OpenAIContentPart {
    match part {
        ContentPart::Text { text } => OpenAIContentPart::Text { text: text.clone() },
        ContentPart::Image { source } => OpenAIContentPart::ImageUrl {
            image_url: content_source_to_openai_image(source),
        },
        // OpenAI doesn't natively support file parts — convert to text fallback
        ContentPart::File { source } => OpenAIContentPart::Text {
            text: format!("[file: {}]", content_source_to_uri(source)),
        },
    }
}

pub fn content_source_to_openai_image(source: &ContentSource) -> OpenAIImageUrl {
    match source {
        ContentSource::Url { url, detail } => OpenAIImageUrl {
            url: url.clone(),
            detail: detail.clone(),
        },
        ContentSource::Base64 { media_type, data } => OpenAIImageUrl {
            url: format!("data:{};base64,{}", media_type, data),
            detail: None,
        },
        ContentSource::FileId { file_id } => OpenAIImageUrl {
            url: file_id.clone(),
            detail: None,
        },
    }
}

pub fn content_source_to_uri(source: &ContentSource) -> String {
    match source {
        ContentSource::Url { url, .. } => url.clone(),
        ContentSource::Base64 { media_type, data } => {
            format!("data:{};base64,{}", media_type, data)
        }
        ContentSource::FileId { file_id } => file_id.clone(),
    }
}

// ---------------------------------------------------------------------------
// OpenAIChatModel
// ---------------------------------------------------------------------------

pub struct OpenAIChatModel {
    api_key: String,
    model_id: String,
    client: reqwest::Client,
}

impl OpenAIChatModel {
    pub fn new(api_key: String, model_id: String) -> Self {
        Self {
            api_key,
            model_id,
            client: reqwest::Client::new(),
        }
    }

    pub fn build_request(&self, messages: &[Message], options: &CallOptions) -> OpenAIRequest {
        let api_messages: Vec<OpenAIMessage> = messages
            .iter()
            .map(|msg| match msg {
                Message::System { content } => OpenAIMessage {
                    role: "system".into(),
                    content: message_content_to_openai(content),
                    tool_call_id: None,
                    tool_calls: None,
                },
                Message::User { content } => OpenAIMessage {
                    role: "user".into(),
                    content: message_content_to_openai(content),
                    tool_call_id: None,
                    tool_calls: None,
                },
                Message::AI(ai) => {
                    let tool_calls = if ai.tool_calls.is_empty() {
                        None
                    } else {
                        Some(
                            ai.tool_calls
                                .iter()
                                .map(|tc| OpenAIRespToolCall {
                                    id: tc.id.clone(),
                                    call_type: "function".into(),
                                    function: OpenAIRespFunction {
                                        name: tc.name.clone(),
                                        arguments: serde_json::to_string(&tc.arguments)
                                            .unwrap_or_default(),
                                    },
                                })
                                .collect(),
                        )
                    };
                    OpenAIMessage {
                        role: "assistant".into(),
                        content: OpenAIContent::Text(ai.content.clone()),
                        tool_call_id: None,
                        tool_calls,
                    }
                }
                Message::Tool {
                    content,
                    tool_call_id,
                } => OpenAIMessage {
                    role: "tool".into(),
                    content: OpenAIContent::Text(content.clone()),
                    tool_call_id: Some(tool_call_id.clone()),
                    tool_calls: None,
                },
            })
            .collect();

        let tools = if options.tools.is_empty() {
            None
        } else {
            Some(
                options
                    .tools
                    .iter()
                    .map(|t| OpenAIToolDef {
                        tool_type: "function".into(),
                        function: OpenAIFunctionDef {
                            name: t.name.clone(),
                            description: t.description.clone(),
                            parameters: t.parameters.clone(),
                        },
                    })
                    .collect(),
            )
        };

        OpenAIRequest {
            model: self.model_id.clone(),
            messages: api_messages,
            max_tokens: options.max_tokens,
            temperature: options.temperature,
            stop: if options.stop.is_empty() {
                None
            } else {
                Some(options.stop.clone())
            },
            tools,
        }
    }
}

#[async_trait]
impl ChatModel for OpenAIChatModel {
    async fn generate(&self, messages: &[Message], options: &CallOptions) -> Result<ChatResult> {
        let request_body = self.build_request(messages, options);

        let response = self
            .client
            .post("https://api.openai.com/v1/chat/completions")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&request_body)
            .send()
            .await
            .map_err(|e| AyasError::Model(ModelError::ApiRequest(e.to_string())))?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "failed to read response body".into());
            let error_msg = serde_json::from_str::<OpenAIError>(&body)
                .map(|e| e.error.message)
                .unwrap_or(body);
            return Err(AyasError::Model(match status.as_u16() {
                401 => ModelError::Auth(error_msg),
                429 => ModelError::RateLimited {
                    retry_after_secs: None,
                },
                _ => ModelError::ApiRequest(format!("HTTP {status}: {error_msg}")),
            }));
        }

        let api_response: OpenAIResponse = response
            .json()
            .await
            .map_err(|e| AyasError::Model(ModelError::InvalidResponse(e.to_string())))?;

        let choice = api_response.choices.first();
        let text = choice
            .and_then(|c| c.message.content.clone())
            .unwrap_or_default();

        let tool_calls = choice
            .and_then(|c| c.message.tool_calls.as_ref())
            .map(|tcs| {
                tcs.iter()
                    .map(|tc| ToolCall {
                        id: tc.id.clone(),
                        name: tc.function.name.clone(),
                        arguments: serde_json::from_str(&tc.function.arguments)
                            .unwrap_or(serde_json::Value::Null),
                    })
                    .collect()
            })
            .unwrap_or_default();

        let usage = api_response.usage.map(|u| UsageMetadata {
            input_tokens: u.prompt_tokens,
            output_tokens: u.completion_tokens,
            total_tokens: u.total_tokens,
        });

        Ok(ChatResult {
            message: Message::AI(AIContent {
                content: text,
                tool_calls,
                usage: usage.clone(),
            }),
            usage,
        })
    }

    fn model_name(&self) -> &str {
        &self.model_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ayas_core::message::{ContentPart, ContentSource, Message, ToolCall};
    use ayas_core::model::CallOptions;
    use ayas_core::tool::ToolDefinition;

    fn make_model() -> OpenAIChatModel {
        OpenAIChatModel::new("test-key".into(), "gpt-4o-mini".into())
    }

    #[test]
    fn build_request_basic() {
        let model = make_model();
        let messages = vec![Message::user("Hello")];
        let options = CallOptions::default();
        let req = model.build_request(&messages, &options);
        assert_eq!(req.model, "gpt-4o-mini");
        assert_eq!(req.messages.len(), 1);
        assert_eq!(req.messages[0].role, "user");
        assert!(req.tools.is_none());
    }

    #[test]
    fn build_request_system() {
        let model = make_model();
        let messages = vec![
            Message::system("You are helpful"),
            Message::user("Hello"),
        ];
        let options = CallOptions::default();
        let req = model.build_request(&messages, &options);
        assert_eq!(req.messages.len(), 2);
        assert_eq!(req.messages[0].role, "system");
    }

    #[test]
    fn build_request_multimodal() {
        let model = make_model();
        let messages = vec![Message::user_with_parts(vec![
            ContentPart::Text {
                text: "describe".into(),
            },
            ContentPart::Image {
                source: ContentSource::Url {
                    url: "https://example.com/img.png".into(),
                    detail: Some("high".into()),
                },
            },
        ])];
        let options = CallOptions::default();
        let req = model.build_request(&messages, &options);
        match &req.messages[0].content {
            OpenAIContent::Parts(parts) => {
                assert_eq!(parts.len(), 2);
            }
            _ => panic!("expected Parts"),
        }
    }

    #[test]
    fn build_request_with_tools() {
        let model = make_model();
        let messages = vec![Message::user("What is 2+2?")];
        let options = CallOptions {
            tools: vec![ToolDefinition {
                name: "calculator".into(),
                description: "Calculate math".into(),
                parameters: serde_json::json!({"type": "object", "properties": {"expr": {"type": "string"}}}),
            }],
            ..Default::default()
        };
        let req = model.build_request(&messages, &options);
        assert!(req.tools.is_some());
        let tools = req.tools.unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].tool_type, "function");
        assert_eq!(tools[0].function.name, "calculator");
        assert_eq!(tools[0].function.description, "Calculate math");
    }

    #[test]
    fn build_request_no_tools() {
        let model = make_model();
        let messages = vec![Message::user("Hello")];
        let options = CallOptions::default();
        let req = model.build_request(&messages, &options);
        assert!(req.tools.is_none());
    }

    #[test]
    fn build_request_tool_message() {
        let model = make_model();
        let messages = vec![Message::tool("4", "call_123")];
        let options = CallOptions::default();
        let req = model.build_request(&messages, &options);
        assert_eq!(req.messages.len(), 1);
        assert_eq!(req.messages[0].role, "tool");
        assert_eq!(req.messages[0].tool_call_id.as_deref(), Some("call_123"));
        match &req.messages[0].content {
            OpenAIContent::Text(text) => assert_eq!(text, "4"),
            _ => panic!("expected Text"),
        }
    }

    #[test]
    fn build_request_ai_with_tool_calls() {
        let model = make_model();
        let messages = vec![Message::ai_with_tool_calls(
            "Let me calculate that.",
            vec![ToolCall {
                id: "call_1".into(),
                name: "calculator".into(),
                arguments: serde_json::json!({"expression": "2+2"}),
            }],
        )];
        let options = CallOptions::default();
        let req = model.build_request(&messages, &options);
        assert_eq!(req.messages.len(), 1);
        assert_eq!(req.messages[0].role, "assistant");
        assert!(req.messages[0].tool_calls.is_some());
        let tcs = req.messages[0].tool_calls.as_ref().unwrap();
        assert_eq!(tcs.len(), 1);
        assert_eq!(tcs[0].id, "call_1");
        assert_eq!(tcs[0].call_type, "function");
        assert_eq!(tcs[0].function.name, "calculator");
        // arguments is a JSON string
        let parsed: serde_json::Value = serde_json::from_str(&tcs[0].function.arguments).unwrap();
        assert_eq!(parsed, serde_json::json!({"expression": "2+2"}));
    }

    #[test]
    fn parse_response_text() {
        let json = r#"{
            "choices": [{"message": {"content": "Hello!"}}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
        }"#;
        let resp: OpenAIResponse = serde_json::from_str(json).unwrap();
        let text = resp
            .choices
            .first()
            .and_then(|c| c.message.content.clone())
            .unwrap_or_default();
        assert_eq!(text, "Hello!");
    }

    #[test]
    fn parse_response_usage() {
        let json = r#"{
            "choices": [{"message": {"content": "Hi"}}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 20, "total_tokens": 30}
        }"#;
        let resp: OpenAIResponse = serde_json::from_str(json).unwrap();
        let usage = resp.usage.unwrap();
        assert_eq!(usage.prompt_tokens, 10);
        assert_eq!(usage.completion_tokens, 20);
        assert_eq!(usage.total_tokens, 30);
    }

    #[test]
    fn parse_response_tool_calls() {
        let json = r#"{
            "choices": [{
                "message": {
                    "content": null,
                    "tool_calls": [{
                        "id": "call_abc123",
                        "type": "function",
                        "function": {
                            "name": "calculator",
                            "arguments": "{\"expression\":\"2+2\"}"
                        }
                    }]
                }
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
        }"#;
        let resp: OpenAIResponse = serde_json::from_str(json).unwrap();
        let tcs = resp.choices[0].message.tool_calls.as_ref().unwrap();
        assert_eq!(tcs.len(), 1);
        assert_eq!(tcs[0].id, "call_abc123");
        assert_eq!(tcs[0].call_type, "function");
        assert_eq!(tcs[0].function.name, "calculator");
        let args: serde_json::Value = serde_json::from_str(&tcs[0].function.arguments).unwrap();
        assert_eq!(args, serde_json::json!({"expression": "2+2"}));
    }

    #[test]
    fn parse_response_mixed_content_and_tool_calls() {
        let json = r#"{
            "choices": [{
                "message": {
                    "content": "Let me calculate that.",
                    "tool_calls": [{
                        "id": "call_abc123",
                        "type": "function",
                        "function": {
                            "name": "calculator",
                            "arguments": "{\"expression\":\"2+2\"}"
                        }
                    }]
                }
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 15, "total_tokens": 25}
        }"#;
        let resp: OpenAIResponse = serde_json::from_str(json).unwrap();
        assert_eq!(
            resp.choices[0].message.content.as_deref(),
            Some("Let me calculate that.")
        );
        let tcs = resp.choices[0].message.tool_calls.as_ref().unwrap();
        assert_eq!(tcs.len(), 1);
        assert_eq!(tcs[0].function.name, "calculator");
    }
}
