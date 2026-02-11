//! OpenAI Chat Completions API integration.

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
// OpenAI Chat Completions API request/response types
// ---------------------------------------------------------------------------

fn is_false(v: &bool) -> bool {
    !*v
}

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<OpenAIResponseFormat>,
    #[serde(skip_serializing_if = "is_false")]
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<OpenAIStreamOptions>,
}

#[derive(Debug, Serialize)]
pub struct OpenAIStreamOptions {
    pub include_usage: bool,
}

#[derive(Debug, Serialize)]
pub struct OpenAIResponseFormat {
    #[serde(rename = "type")]
    pub format_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub json_schema: Option<OpenAIJsonSchema>,
}

#[derive(Debug, Serialize)]
pub struct OpenAIJsonSchema {
    pub name: String,
    pub schema: serde_json::Value,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub strict: bool,
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

        let response_format = match &options.response_format {
            Some(ResponseFormat::JsonObject) => Some(OpenAIResponseFormat {
                format_type: "json_object".into(),
                json_schema: None,
            }),
            Some(ResponseFormat::JsonSchema {
                name,
                schema,
                strict,
            }) => Some(OpenAIResponseFormat {
                format_type: "json_schema".into(),
                json_schema: Some(OpenAIJsonSchema {
                    name: name.clone(),
                    schema: schema.clone(),
                    strict: *strict,
                }),
            }),
            Some(ResponseFormat::Text) | None => None,
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
            response_format,
            stream: false,
            stream_options: None,
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

    async fn stream(
        &self,
        messages: &[Message],
        options: &CallOptions,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<ChatStreamEvent>> + Send>>> {
        let mut request_body = self.build_request(messages, options);
        request_body.stream = true;
        request_body.stream_options = Some(OpenAIStreamOptions {
            include_usage: true,
        });

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

        let data_stream = sse_data_stream(response);

        let event_stream = async_stream::stream! {
            let mut data_stream = Box::pin(data_stream);

            while let Some(data) = data_stream.next().await {
                if data == "[DONE]" {
                    yield Ok(ChatStreamEvent::Done);
                    break;
                }

                for event in parse_openai_sse_data(&data) {
                    yield Ok(event);
                }
            }
        };

        Ok(Box::pin(event_stream))
    }
}

/// Parse a single OpenAI SSE data line into stream events (for testing).
pub fn parse_openai_sse_data(data: &str) -> Vec<ChatStreamEvent> {
    let mut events = Vec::new();
    let json: serde_json::Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(_) => return events,
    };

    // Handle usage-only message (sent at the end with stream_options.include_usage)
    if let Some(usage) = json.get("usage") {
        if usage.is_object() && !usage.is_null() {
            let prompt = usage["prompt_tokens"].as_u64().unwrap_or(0);
            let completion = usage["completion_tokens"].as_u64().unwrap_or(0);
            let total = usage["total_tokens"].as_u64().unwrap_or(prompt + completion);
            events.push(ChatStreamEvent::Usage(UsageMetadata {
                input_tokens: prompt,
                output_tokens: completion,
                total_tokens: total,
            }));
        }
    }

    let choices = match json["choices"].as_array() {
        Some(c) => c,
        None => return events,
    };

    for choice in choices {
        let delta = &choice["delta"];

        // Text content
        if let Some(text) = delta["content"].as_str() {
            if !text.is_empty() {
                events.push(ChatStreamEvent::Token(text.to_string()));
            }
        }

        // Tool calls
        if let Some(tool_calls) = delta["tool_calls"].as_array() {
            for tc in tool_calls {
                if let Some(id) = tc["id"].as_str() {
                    let name = tc["function"]["name"].as_str().unwrap_or("").to_string();
                    events.push(ChatStreamEvent::ToolCallStart {
                        id: id.to_string(),
                        name,
                    });
                }
                if let Some(args) = tc["function"]["arguments"].as_str() {
                    if !args.is_empty() {
                        let id = tc["id"]
                            .as_str()
                            .unwrap_or("")
                            .to_string();
                        events.push(ChatStreamEvent::ToolCallDelta {
                            id,
                            arguments: args.to_string(),
                        });
                    }
                }
            }
        }
    }

    events
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

    #[test]
    fn build_request_stream_false_by_default() {
        let model = make_model();
        let messages = vec![Message::user("Hello")];
        let options = CallOptions::default();
        let req = model.build_request(&messages, &options);
        assert!(!req.stream);
        assert!(req.stream_options.is_none());
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
        let rf = req.response_format.unwrap();
        assert_eq!(rf.format_type, "json_object");
        assert!(rf.json_schema.is_none());
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
        let rf = req.response_format.unwrap();
        assert_eq!(rf.format_type, "json_schema");
        let js = rf.json_schema.unwrap();
        assert_eq!(js.name, "person");
        assert_eq!(js.schema, schema);
        assert!(js.strict);
    }

    #[test]
    fn build_request_text_format_omits_response_format() {
        let model = make_model();
        let messages = vec![Message::user("Hello")];
        let options = CallOptions {
            response_format: Some(ayas_core::model::ResponseFormat::Text),
            ..Default::default()
        };
        let req = model.build_request(&messages, &options);
        assert!(req.response_format.is_none());
    }

    // -----------------------------------------------------------------------
    // SSE parsing tests
    // -----------------------------------------------------------------------

    #[test]
    fn parse_sse_text_content() {
        let data = r#"{"id":"chatcmpl-1","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}]}"#;
        let events = parse_openai_sse_data(data);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], ChatStreamEvent::Token("Hello".into()));
    }

    #[test]
    fn parse_sse_empty_content_skipped() {
        let data = r#"{"id":"chatcmpl-1","choices":[{"index":0,"delta":{"role":"assistant","content":""},"finish_reason":null}]}"#;
        let events = parse_openai_sse_data(data);
        assert!(events.is_empty());
    }

    #[test]
    fn parse_sse_tool_call_start() {
        let data = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_abc","type":"function","function":{"name":"calculator","arguments":""}}]}}]}"#;
        let events = parse_openai_sse_data(data);
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            ChatStreamEvent::ToolCallStart {
                id: "call_abc".into(),
                name: "calculator".into(),
            }
        );
    }

    #[test]
    fn parse_sse_tool_call_delta() {
        let data = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_abc","function":{"arguments":"{\"x\":"}}]}}]}"#;
        let events = parse_openai_sse_data(data);
        // Should have ToolCallStart (due to id present) + ToolCallDelta
        assert!(events.len() >= 1);
        let has_delta = events.iter().any(|e| {
            matches!(e, ChatStreamEvent::ToolCallDelta { arguments, .. } if arguments == r#"{"x":"#)
        });
        assert!(has_delta);
    }

    #[test]
    fn parse_sse_usage() {
        let data = r#"{"id":"chatcmpl-1","choices":[],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}"#;
        let events = parse_openai_sse_data(data);
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            ChatStreamEvent::Usage(UsageMetadata {
                input_tokens: 10,
                output_tokens: 5,
                total_tokens: 15,
            })
        );
    }

    #[test]
    fn parse_sse_full_text_sequence() {
        let sse_events = [
            r#"{"id":"chatcmpl-1","choices":[{"index":0,"delta":{"role":"assistant","content":""},"finish_reason":null}]}"#,
            r#"{"id":"chatcmpl-1","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}]}"#,
            r#"{"id":"chatcmpl-1","choices":[{"index":0,"delta":{"content":" world!"},"finish_reason":null}]}"#,
            r#"{"id":"chatcmpl-1","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}"#,
        ];

        let mut all_events = Vec::new();
        for data in &sse_events {
            all_events.extend(parse_openai_sse_data(data));
        }

        // Should have: Token("Hello"), Token(" world!"), Usage
        assert_eq!(all_events.len(), 3);
        assert_eq!(all_events[0], ChatStreamEvent::Token("Hello".into()));
        assert_eq!(all_events[1], ChatStreamEvent::Token(" world!".into()));
        assert!(matches!(all_events[2], ChatStreamEvent::Usage(_)));
    }
}
