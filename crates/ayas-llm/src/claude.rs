//! Anthropic Claude API integration.

use std::pin::Pin;

use async_trait::async_trait;
use futures::{Stream, StreamExt};
use serde::{Deserialize, Serialize};

use ayas_core::error::{AyasError, ModelError, Result};
use ayas_core::message::{
    AIContent, ContentPart, ContentSource, Message, MessageContent, ToolCall, UsageMetadata,
};
use ayas_core::model::{CallOptions, ChatModel, ChatResult, ChatStreamEvent, ResponseFormat};

use crate::sse::sse_data_stream;

// ---------------------------------------------------------------------------
// Anthropic Messages API request/response types
// ---------------------------------------------------------------------------

fn is_false(v: &bool) -> bool {
    !*v
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnthropicToolChoice {
    Auto,
    Any,
    Tool { name: String },
}

#[derive(Debug, Serialize)]
pub struct AnthropicRequest {
    pub model: String,
    pub max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    pub messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_sequences: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<AnthropicToolDef>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<AnthropicToolChoice>,
    #[serde(skip_serializing_if = "is_false")]
    pub stream: bool,
}

#[derive(Debug, Serialize)]
pub struct AnthropicToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct AnthropicMessage {
    pub role: String,
    pub content: AnthropicContent,
}

/// Anthropic content: text-only or multimodal parts array.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum AnthropicContent {
    Text(String),
    Parts(Vec<AnthropicContentPart>),
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum AnthropicContentPart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image { source: AnthropicImageSource },
    #[serde(rename = "document")]
    Document { source: AnthropicDocSource },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
    },
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum AnthropicImageSource {
    #[serde(rename = "base64")]
    Base64 { media_type: String, data: String },
    #[serde(rename = "url")]
    Url { url: String },
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum AnthropicDocSource {
    #[serde(rename = "base64")]
    Base64 { media_type: String, data: String },
    #[serde(rename = "url")]
    Url { url: String },
    #[serde(rename = "file")]
    File { file_id: String },
}

#[derive(Debug, Deserialize)]
pub struct AnthropicResponse {
    pub content: Vec<AnthropicResponseContent>,
    pub usage: AnthropicUsage,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum AnthropicResponseContent {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
}

#[derive(Debug, Deserialize)]
pub struct AnthropicUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Deserialize)]
pub struct AnthropicError {
    pub error: AnthropicErrorDetail,
}

#[derive(Debug, Deserialize)]
pub struct AnthropicErrorDetail {
    pub message: String,
}

// ---------------------------------------------------------------------------
// Conversion helpers
// ---------------------------------------------------------------------------

pub fn message_content_to_anthropic(mc: &MessageContent) -> AnthropicContent {
    match mc {
        MessageContent::Text(s) => AnthropicContent::Text(s.clone()),
        MessageContent::Parts(parts) => {
            AnthropicContent::Parts(parts.iter().map(content_part_to_anthropic).collect())
        }
    }
}

pub fn content_part_to_anthropic(part: &ContentPart) -> AnthropicContentPart {
    match part {
        ContentPart::Text { text } => AnthropicContentPart::Text { text: text.clone() },
        ContentPart::Image { source } => AnthropicContentPart::Image {
            source: content_source_to_anthropic_image(source),
        },
        ContentPart::File { source } => AnthropicContentPart::Document {
            source: content_source_to_anthropic_doc(source),
        },
    }
}

pub fn content_source_to_anthropic_image(source: &ContentSource) -> AnthropicImageSource {
    match source {
        ContentSource::Base64 { media_type, data } => AnthropicImageSource::Base64 {
            media_type: media_type.clone(),
            data: data.clone(),
        },
        ContentSource::Url { url, .. } => AnthropicImageSource::Url { url: url.clone() },
        ContentSource::FileId { file_id } => AnthropicImageSource::Url {
            url: file_id.clone(),
        },
    }
}

pub fn content_source_to_anthropic_doc(source: &ContentSource) -> AnthropicDocSource {
    match source {
        ContentSource::Base64 { media_type, data } => AnthropicDocSource::Base64 {
            media_type: media_type.clone(),
            data: data.clone(),
        },
        ContentSource::Url { url, .. } => AnthropicDocSource::Url { url: url.clone() },
        ContentSource::FileId { file_id } => AnthropicDocSource::File {
            file_id: file_id.clone(),
        },
    }
}

// ---------------------------------------------------------------------------
// ClaudeChatModel
// ---------------------------------------------------------------------------

pub struct ClaudeChatModel {
    api_key: String,
    model_id: String,
    client: reqwest::Client,
}

impl ClaudeChatModel {
    pub fn new(api_key: String, model_id: String) -> Self {
        Self {
            api_key,
            model_id,
            client: reqwest::Client::new(),
        }
    }

    pub fn build_request(&self, messages: &[Message], options: &CallOptions) -> AnthropicRequest {
        let mut system: Option<String> = None;
        let mut api_messages: Vec<AnthropicMessage> = Vec::new();

        for msg in messages {
            match msg {
                Message::System { content } => {
                    system = Some(content.text());
                }
                Message::User { content } => {
                    api_messages.push(AnthropicMessage {
                        role: "user".into(),
                        content: message_content_to_anthropic(content),
                    });
                }
                Message::AI(ai) => {
                    let mut parts: Vec<AnthropicContentPart> = Vec::new();
                    if !ai.content.is_empty() {
                        parts.push(AnthropicContentPart::Text {
                            text: ai.content.clone(),
                        });
                    }
                    for tc in &ai.tool_calls {
                        parts.push(AnthropicContentPart::ToolUse {
                            id: tc.id.clone(),
                            name: tc.name.clone(),
                            input: tc.arguments.clone(),
                        });
                    }
                    let content = if parts.is_empty() {
                        AnthropicContent::Text(ai.content.clone())
                    } else if ai.tool_calls.is_empty() {
                        // Text-only AI message
                        AnthropicContent::Text(ai.content.clone())
                    } else {
                        AnthropicContent::Parts(parts)
                    };
                    api_messages.push(AnthropicMessage {
                        role: "assistant".into(),
                        content,
                    });
                }
                Message::Tool {
                    content,
                    tool_call_id,
                } => {
                    api_messages.push(AnthropicMessage {
                        role: "user".into(),
                        content: AnthropicContent::Parts(vec![AnthropicContentPart::ToolResult {
                            tool_use_id: tool_call_id.clone(),
                            content: content.clone(),
                        }]),
                    });
                }
            }
        }

        let mut tools: Vec<AnthropicToolDef> = options
            .tools
            .iter()
            .map(|t| AnthropicToolDef {
                name: t.name.clone(),
                description: t.description.clone(),
                input_schema: t.parameters.clone(),
            })
            .collect();

        let mut tool_choice: Option<AnthropicToolChoice> = None;

        // Handle response_format via tool_choice pattern
        match &options.response_format {
            Some(ResponseFormat::JsonObject) => {
                // Append instruction to system prompt
                let suffix = "\n\nAlways respond in valid JSON.";
                system = Some(match system {
                    Some(s) => format!("{s}{suffix}"),
                    None => suffix.trim_start().to_string(),
                });
            }
            Some(ResponseFormat::JsonSchema { name, schema, .. }) => {
                // Inject a synthetic tool and force its use
                tools.push(AnthropicToolDef {
                    name: name.clone(),
                    description: "Structured output".into(),
                    input_schema: schema.clone(),
                });
                tool_choice = Some(AnthropicToolChoice::Tool { name: name.clone() });
            }
            Some(ResponseFormat::Text) | None => {}
        }

        let tools_opt = if tools.is_empty() { None } else { Some(tools) };

        AnthropicRequest {
            model: self.model_id.clone(),
            max_tokens: options.max_tokens.unwrap_or(1024),
            system,
            messages: api_messages,
            temperature: options.temperature,
            stop_sequences: if options.stop.is_empty() {
                None
            } else {
                Some(options.stop.clone())
            },
            tools: tools_opt,
            tool_choice,
            stream: false,
        }
    }
}

#[async_trait]
impl ChatModel for ClaudeChatModel {
    async fn generate(&self, messages: &[Message], options: &CallOptions) -> Result<ChatResult> {
        let is_structured = matches!(
            options.response_format,
            Some(ResponseFormat::JsonSchema { .. })
        );
        let request_body = self.build_request(messages, options);

        let response = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
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
            let error_msg = serde_json::from_str::<AnthropicError>(&body)
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

        let api_response: AnthropicResponse = response
            .json()
            .await
            .map_err(|e| AyasError::Model(ModelError::InvalidResponse(e.to_string())))?;

        let mut text_parts = Vec::new();
        let mut tool_calls = Vec::new();

        for block in &api_response.content {
            match block {
                AnthropicResponseContent::Text { text } => {
                    text_parts.push(text.clone());
                }
                AnthropicResponseContent::ToolUse { id, name, input } => {
                    if is_structured {
                        // Convert tool_use input to text content for structured output
                        text_parts.push(serde_json::to_string(input).unwrap_or_default());
                    } else {
                        tool_calls.push(ToolCall {
                            id: id.clone(),
                            name: name.clone(),
                            arguments: input.clone(),
                        });
                    }
                }
            }
        }

        let text = text_parts.join("");

        let usage = UsageMetadata {
            input_tokens: api_response.usage.input_tokens,
            output_tokens: api_response.usage.output_tokens,
            total_tokens: api_response.usage.input_tokens + api_response.usage.output_tokens,
        };

        Ok(ChatResult {
            message: Message::AI(AIContent {
                content: text,
                tool_calls,
                usage: Some(usage.clone()),
            }),
            usage: Some(usage),
        })
    }

    fn model_name(&self) -> &str {
        &self.model_id
    }

    async fn stream(
        &self,
        messages: &[Message],
        options: &CallOptions,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<ChatStreamEvent>> + Send>>> {
        let mut request_body = self.build_request(messages, options);
        request_body.stream = true;

        let response = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
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
            let error_msg = serde_json::from_str::<AnthropicError>(&body)
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

        let data_stream = sse_data_stream(response);

        let event_stream = async_stream::stream! {
            let mut current_tool_id = String::new();
            let mut input_tokens = 0u64;
            let mut data_stream = Box::pin(data_stream);

            while let Some(data) = data_stream.next().await {
                let json: serde_json::Value = match serde_json::from_str(&data) {
                    Ok(v) => v,
                    Err(_) => continue,
                };

                match json["type"].as_str().unwrap_or("") {
                    "message_start" => {
                        input_tokens = json["message"]["usage"]["input_tokens"]
                            .as_u64()
                            .unwrap_or(0);
                    }
                    "content_block_start" => {
                        let block = &json["content_block"];
                        if block["type"].as_str() == Some("tool_use") {
                            current_tool_id =
                                block["id"].as_str().unwrap_or("").to_string();
                            yield Ok(ChatStreamEvent::ToolCallStart {
                                id: current_tool_id.clone(),
                                name: block["name"].as_str().unwrap_or("").to_string(),
                            });
                        }
                    }
                    "content_block_delta" => {
                        let delta = &json["delta"];
                        match delta["type"].as_str() {
                            Some("text_delta") => {
                                if let Some(text) = delta["text"].as_str() {
                                    yield Ok(ChatStreamEvent::Token(text.to_string()));
                                }
                            }
                            Some("input_json_delta") => {
                                if let Some(partial) = delta["partial_json"].as_str() {
                                    yield Ok(ChatStreamEvent::ToolCallDelta {
                                        id: current_tool_id.clone(),
                                        arguments: partial.to_string(),
                                    });
                                }
                            }
                            _ => {}
                        }
                    }
                    "message_delta" => {
                        let output_tokens =
                            json["usage"]["output_tokens"].as_u64().unwrap_or(0);
                        yield Ok(ChatStreamEvent::Usage(UsageMetadata {
                            input_tokens,
                            output_tokens,
                            total_tokens: input_tokens + output_tokens,
                        }));
                    }
                    "message_stop" => {
                        yield Ok(ChatStreamEvent::Done);
                    }
                    _ => {}
                }
            }
        };

        Ok(Box::pin(event_stream))
    }
}

/// Parse a single Claude SSE data line into stream events (for testing).
pub fn parse_claude_sse_data(
    data: &str,
    current_tool_id: &mut String,
    input_tokens: &mut u64,
) -> Vec<ChatStreamEvent> {
    let mut events = Vec::new();
    let json: serde_json::Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(_) => return events,
    };

    match json["type"].as_str().unwrap_or("") {
        "message_start" => {
            *input_tokens = json["message"]["usage"]["input_tokens"]
                .as_u64()
                .unwrap_or(0);
        }
        "content_block_start" => {
            let block = &json["content_block"];
            if block["type"].as_str() == Some("tool_use") {
                *current_tool_id = block["id"].as_str().unwrap_or("").to_string();
                events.push(ChatStreamEvent::ToolCallStart {
                    id: current_tool_id.clone(),
                    name: block["name"].as_str().unwrap_or("").to_string(),
                });
            }
        }
        "content_block_delta" => {
            let delta = &json["delta"];
            match delta["type"].as_str() {
                Some("text_delta") => {
                    if let Some(text) = delta["text"].as_str() {
                        events.push(ChatStreamEvent::Token(text.to_string()));
                    }
                }
                Some("input_json_delta") => {
                    if let Some(partial) = delta["partial_json"].as_str() {
                        events.push(ChatStreamEvent::ToolCallDelta {
                            id: current_tool_id.clone(),
                            arguments: partial.to_string(),
                        });
                    }
                }
                _ => {}
            }
        }
        "message_delta" => {
            let output_tokens = json["usage"]["output_tokens"].as_u64().unwrap_or(0);
            events.push(ChatStreamEvent::Usage(UsageMetadata {
                input_tokens: *input_tokens,
                output_tokens,
                total_tokens: *input_tokens + output_tokens,
            }));
        }
        "message_stop" => {
            events.push(ChatStreamEvent::Done);
        }
        _ => {}
    }

    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use ayas_core::message::{ContentPart, ContentSource, Message, ToolCall};
    use ayas_core::model::CallOptions;
    use ayas_core::tool::ToolDefinition;

    fn make_model() -> ClaudeChatModel {
        ClaudeChatModel::new("test-key".into(), "claude-sonnet-4-5-20250929".into())
    }

    #[test]
    fn build_request_basic() {
        let model = make_model();
        let messages = vec![Message::user("Hello")];
        let options = CallOptions::default();
        let req = model.build_request(&messages, &options);
        assert_eq!(req.model, "claude-sonnet-4-5-20250929");
        assert_eq!(req.messages.len(), 1);
        assert_eq!(req.messages[0].role, "user");
        assert!(req.system.is_none());
        assert!(req.tools.is_none());
    }

    #[test]
    fn build_request_system_extract() {
        let model = make_model();
        let messages = vec![
            Message::system("You are helpful"),
            Message::user("Hello"),
        ];
        let options = CallOptions::default();
        let req = model.build_request(&messages, &options);
        assert_eq!(req.system.as_deref(), Some("You are helpful"));
        assert_eq!(req.messages.len(), 1); // system not in messages
    }

    #[test]
    fn build_request_multimodal() {
        let model = make_model();
        let messages = vec![Message::user_with_parts(vec![
            ContentPart::Text {
                text: "describe".into(),
            },
            ContentPart::Image {
                source: ContentSource::Base64 {
                    media_type: "image/png".into(),
                    data: "abc123".into(),
                },
            },
        ])];
        let options = CallOptions::default();
        let req = model.build_request(&messages, &options);
        assert_eq!(req.messages.len(), 1);
        // The content should be Parts variant
        match &req.messages[0].content {
            AnthropicContent::Parts(parts) => {
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
        assert_eq!(tools[0].name, "calculator");
        assert_eq!(tools[0].description, "Calculate math");
        assert_eq!(
            tools[0].input_schema,
            serde_json::json!({"type": "object", "properties": {"expr": {"type": "string"}}})
        );
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
        assert_eq!(req.messages[0].role, "user");
        match &req.messages[0].content {
            AnthropicContent::Parts(parts) => {
                assert_eq!(parts.len(), 1);
                match &parts[0] {
                    AnthropicContentPart::ToolResult {
                        tool_use_id,
                        content,
                    } => {
                        assert_eq!(tool_use_id, "call_123");
                        assert_eq!(content, "4");
                    }
                    _ => panic!("expected ToolResult"),
                }
            }
            _ => panic!("expected Parts"),
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
        match &req.messages[0].content {
            AnthropicContent::Parts(parts) => {
                assert_eq!(parts.len(), 2);
                match &parts[0] {
                    AnthropicContentPart::Text { text } => {
                        assert_eq!(text, "Let me calculate that.");
                    }
                    _ => panic!("expected Text"),
                }
                match &parts[1] {
                    AnthropicContentPart::ToolUse { id, name, input } => {
                        assert_eq!(id, "call_1");
                        assert_eq!(name, "calculator");
                        assert_eq!(input, &serde_json::json!({"expression": "2+2"}));
                    }
                    _ => panic!("expected ToolUse"),
                }
            }
            _ => panic!("expected Parts"),
        }
    }

    #[test]
    fn parse_response_text() {
        let json = r#"{
            "content": [{"type": "text", "text": "Hello!"}],
            "usage": {"input_tokens": 10, "output_tokens": 5}
        }"#;
        let resp: AnthropicResponse = serde_json::from_str(json).unwrap();
        match &resp.content[0] {
            AnthropicResponseContent::Text { text } => assert_eq!(text, "Hello!"),
            _ => panic!("expected Text"),
        }
    }

    #[test]
    fn parse_response_usage() {
        let json = r#"{
            "content": [{"type": "text", "text": "Hi"}],
            "usage": {"input_tokens": 15, "output_tokens": 25}
        }"#;
        let resp: AnthropicResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.usage.input_tokens, 15);
        assert_eq!(resp.usage.output_tokens, 25);
    }

    #[test]
    fn parse_response_tool_use() {
        let json = r#"{
            "content": [
                {"type": "tool_use", "id": "toolu_01", "name": "calculator", "input": {"expression": "2+2"}}
            ],
            "usage": {"input_tokens": 10, "output_tokens": 5}
        }"#;
        let resp: AnthropicResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.content.len(), 1);
        match &resp.content[0] {
            AnthropicResponseContent::ToolUse { id, name, input } => {
                assert_eq!(id, "toolu_01");
                assert_eq!(name, "calculator");
                assert_eq!(input, &serde_json::json!({"expression": "2+2"}));
            }
            _ => panic!("expected ToolUse"),
        }
    }

    #[test]
    fn parse_response_mixed_text_and_tool_use() {
        let json = r#"{
            "content": [
                {"type": "text", "text": "Let me calculate that."},
                {"type": "tool_use", "id": "toolu_01", "name": "calculator", "input": {"expression": "2+2"}}
            ],
            "usage": {"input_tokens": 10, "output_tokens": 15}
        }"#;
        let resp: AnthropicResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.content.len(), 2);
        match &resp.content[0] {
            AnthropicResponseContent::Text { text } => {
                assert_eq!(text, "Let me calculate that.");
            }
            _ => panic!("expected Text"),
        }
        match &resp.content[1] {
            AnthropicResponseContent::ToolUse { id, name, input } => {
                assert_eq!(id, "toolu_01");
                assert_eq!(name, "calculator");
                assert_eq!(input, &serde_json::json!({"expression": "2+2"}));
            }
            _ => panic!("expected ToolUse"),
        }
    }

    #[test]
    fn build_request_stream_false_by_default() {
        let model = make_model();
        let messages = vec![Message::user("Hello")];
        let options = CallOptions::default();
        let req = model.build_request(&messages, &options);
        assert!(!req.stream);
        // stream: false should be omitted in JSON
        let json = serde_json::to_string(&req).unwrap();
        assert!(!json.contains("stream"));
    }

    // -----------------------------------------------------------------------
    // Structured output tests
    // -----------------------------------------------------------------------

    #[test]
    fn build_request_json_object() {
        let model = make_model();
        let messages = vec![Message::user("Return JSON")];
        let options = CallOptions {
            response_format: Some(ayas_core::model::ResponseFormat::JsonObject),
            ..Default::default()
        };
        let req = model.build_request(&messages, &options);
        // Should append JSON instruction to system prompt
        assert!(req.system.as_deref().unwrap().contains("Always respond in valid JSON."));
        // No tool_choice for JsonObject
        assert!(req.tool_choice.is_none());
    }

    #[test]
    fn build_request_json_object_appends_to_existing_system() {
        let model = make_model();
        let messages = vec![
            Message::system("You are helpful"),
            Message::user("Return JSON"),
        ];
        let options = CallOptions {
            response_format: Some(ayas_core::model::ResponseFormat::JsonObject),
            ..Default::default()
        };
        let req = model.build_request(&messages, &options);
        let sys = req.system.as_deref().unwrap();
        assert!(sys.starts_with("You are helpful"));
        assert!(sys.contains("Always respond in valid JSON."));
    }

    #[test]
    fn build_request_json_schema() {
        let model = make_model();
        let messages = vec![Message::user("Extract info")];
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "age": {"type": "integer"}
            },
            "required": ["name", "age"]
        });
        let options = CallOptions {
            response_format: Some(ayas_core::model::ResponseFormat::JsonSchema {
                name: "person".into(),
                schema: schema.clone(),
                strict: true,
            }),
            ..Default::default()
        };
        let req = model.build_request(&messages, &options);

        // Should have a synthetic tool injected
        let tools = req.tools.unwrap();
        let synthetic = tools.iter().find(|t| t.name == "person").unwrap();
        assert_eq!(synthetic.description, "Structured output");
        assert_eq!(synthetic.input_schema, schema);

        // tool_choice should force the synthetic tool
        match &req.tool_choice {
            Some(AnthropicToolChoice::Tool { name }) => assert_eq!(name, "person"),
            other => panic!("expected Tool choice, got {:?}", other),
        }
    }

    #[test]
    fn build_request_json_schema_with_existing_tools() {
        let model = make_model();
        let messages = vec![Message::user("Extract info")];
        let schema = serde_json::json!({"type": "object", "properties": {"name": {"type": "string"}}});
        let options = CallOptions {
            tools: vec![ToolDefinition {
                name: "calculator".into(),
                description: "Calculate math".into(),
                parameters: serde_json::json!({"type": "object"}),
            }],
            response_format: Some(ayas_core::model::ResponseFormat::JsonSchema {
                name: "person".into(),
                schema,
                strict: true,
            }),
            ..Default::default()
        };
        let req = model.build_request(&messages, &options);
        let tools = req.tools.unwrap();
        // Should have both the user tool and the synthetic tool
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "calculator");
        assert_eq!(tools[1].name, "person");
    }

    // -----------------------------------------------------------------------
    // SSE parsing tests
    // -----------------------------------------------------------------------

    #[test]
    fn parse_sse_message_start() {
        let data = r#"{"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","content":[],"usage":{"input_tokens":25,"output_tokens":1}}}"#;
        let mut tool_id = String::new();
        let mut input_tokens = 0u64;
        let events = parse_claude_sse_data(data, &mut tool_id, &mut input_tokens);
        assert!(events.is_empty());
        assert_eq!(input_tokens, 25);
    }

    #[test]
    fn parse_sse_text_delta() {
        let data = r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#;
        let mut tool_id = String::new();
        let mut input_tokens = 0u64;
        let events = parse_claude_sse_data(data, &mut tool_id, &mut input_tokens);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], ChatStreamEvent::Token("Hello".into()));
    }

    #[test]
    fn parse_sse_tool_use_start() {
        let data = r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_01","name":"calculator","input":{}}}"#;
        let mut tool_id = String::new();
        let mut input_tokens = 0u64;
        let events = parse_claude_sse_data(data, &mut tool_id, &mut input_tokens);
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            ChatStreamEvent::ToolCallStart {
                id: "toolu_01".into(),
                name: "calculator".into(),
            }
        );
        assert_eq!(tool_id, "toolu_01");
    }

    #[test]
    fn parse_sse_tool_use_delta() {
        let data = r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"expr"}}"#;
        let mut tool_id = "toolu_01".to_string();
        let mut input_tokens = 0u64;
        let events = parse_claude_sse_data(data, &mut tool_id, &mut input_tokens);
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            ChatStreamEvent::ToolCallDelta {
                id: "toolu_01".into(),
                arguments: r#"{"expr"#.into(),
            }
        );
    }

    #[test]
    fn parse_sse_message_delta_usage() {
        let data =
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":12}}"#;
        let mut tool_id = String::new();
        let mut input_tokens = 25u64;
        let events = parse_claude_sse_data(data, &mut tool_id, &mut input_tokens);
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            ChatStreamEvent::Usage(UsageMetadata {
                input_tokens: 25,
                output_tokens: 12,
                total_tokens: 37,
            })
        );
    }

    #[test]
    fn parse_sse_message_stop() {
        let data = r#"{"type":"message_stop"}"#;
        let mut tool_id = String::new();
        let mut input_tokens = 0u64;
        let events = parse_claude_sse_data(data, &mut tool_id, &mut input_tokens);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], ChatStreamEvent::Done);
    }

    #[test]
    fn parse_sse_full_text_sequence() {
        let sse_events = [
            r#"{"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","content":[],"usage":{"input_tokens":10,"output_tokens":1}}}"#,
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":" world!"}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":5}}"#,
            r#"{"type":"message_stop"}"#,
        ];

        let mut tool_id = String::new();
        let mut input_tokens = 0u64;
        let mut all_events = Vec::new();

        for sse in &sse_events {
            all_events.extend(parse_claude_sse_data(sse, &mut tool_id, &mut input_tokens));
        }

        assert_eq!(all_events.len(), 4);
        assert_eq!(all_events[0], ChatStreamEvent::Token("Hello".into()));
        assert_eq!(all_events[1], ChatStreamEvent::Token(" world!".into()));
        assert_eq!(
            all_events[2],
            ChatStreamEvent::Usage(UsageMetadata {
                input_tokens: 10,
                output_tokens: 5,
                total_tokens: 15,
            })
        );
        assert_eq!(all_events[3], ChatStreamEvent::Done);
    }
}
