use std::pin::Pin;

use async_trait::async_trait;
use futures::{Stream, StreamExt};
use serde::{Deserialize, Serialize};

use ayas_core::error::{AyasError, ModelError, Result};
use ayas_core::message::{
    AIContent, ContentPart, ContentSource, Message, MessageContent, ToolCall, UsageMetadata,
};
use ayas_core::model::{CallOptions, ChatModel, ChatResult, ChatStreamEvent};

use crate::sse::sse_data_stream;

// ---------------------------------------------------------------------------
// Gemini API request types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct GeminiRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_instruction: Option<GeminiContent>,
    pub contents: Vec<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation_config: Option<GenerationConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<GeminiToolConfig>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GeminiContent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    pub parts: Vec<GeminiPart>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiPart {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inline_data: Option<GeminiInlineData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_data: Option<GeminiFileData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function_call: Option<GeminiFunctionCall>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiInlineData {
    pub mime_type: String,
    pub data: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiFileData {
    pub mime_type: String,
    pub file_uri: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GeminiFunctionCall {
    pub name: String,
    pub args: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GeminiToolConfig {
    pub function_declarations: Vec<GeminiFunctionDeclaration>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GeminiFunctionDeclaration {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct GenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_sequences: Option<Vec<String>>,
}

// ---------------------------------------------------------------------------
// Gemini API response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct GeminiResponse {
    pub candidates: Option<Vec<GeminiCandidate>>,
    #[serde(rename = "usageMetadata")]
    pub usage_metadata: Option<GeminiUsageMetadata>,
}

#[derive(Debug, Deserialize)]
pub struct GeminiCandidate {
    pub content: GeminiContent,
}

#[derive(Debug, Deserialize)]
pub struct GeminiUsageMetadata {
    #[serde(rename = "promptTokenCount", default)]
    pub prompt_token_count: u64,
    #[serde(rename = "candidatesTokenCount", default)]
    pub candidates_token_count: u64,
    #[serde(rename = "totalTokenCount", default)]
    pub total_token_count: u64,
}

// ---------------------------------------------------------------------------
// Conversion helpers
// ---------------------------------------------------------------------------

fn text_part(text: String) -> GeminiPart {
    GeminiPart {
        text: Some(text),
        inline_data: None,
        file_data: None,
        function_call: None,
    }
}

fn message_content_to_gemini_parts(mc: &MessageContent) -> Vec<GeminiPart> {
    match mc {
        MessageContent::Text(s) => vec![text_part(s.clone())],
        MessageContent::Parts(parts) => parts.iter().map(content_part_to_gemini).collect(),
    }
}

fn content_part_to_gemini(part: &ContentPart) -> GeminiPart {
    match part {
        ContentPart::Text { text } => text_part(text.clone()),
        ContentPart::Image { source } | ContentPart::File { source } => {
            content_source_to_gemini(source)
        }
    }
}

fn content_source_to_gemini(source: &ContentSource) -> GeminiPart {
    match source {
        ContentSource::Base64 { media_type, data } => GeminiPart {
            text: None,
            inline_data: Some(GeminiInlineData {
                mime_type: media_type.clone(),
                data: data.clone(),
            }),
            file_data: None,
            function_call: None,
        },
        ContentSource::Url { url, .. } => GeminiPart {
            text: None,
            inline_data: None,
            file_data: Some(GeminiFileData {
                mime_type: "application/octet-stream".into(),
                file_uri: url.clone(),
            }),
            function_call: None,
        },
        ContentSource::FileId { file_id } => GeminiPart {
            text: None,
            inline_data: None,
            file_data: Some(GeminiFileData {
                mime_type: "application/octet-stream".into(),
                file_uri: file_id.clone(),
            }),
            function_call: None,
        },
    }
}

// ---------------------------------------------------------------------------
// GeminiChatModel
// ---------------------------------------------------------------------------

pub struct GeminiChatModel {
    api_key: String,
    model_id: String,
    client: reqwest::Client,
}

impl GeminiChatModel {
    pub fn new(api_key: String, model_id: String) -> Self {
        Self {
            api_key,
            model_id,
            client: reqwest::Client::new(),
        }
    }

    pub fn build_request(&self, messages: &[Message], options: &CallOptions) -> GeminiRequest {
        let mut system_instruction: Option<GeminiContent> = None;
        let mut contents: Vec<GeminiContent> = Vec::new();

        for msg in messages {
            match msg {
                Message::System { content } => {
                    system_instruction = Some(GeminiContent {
                        role: None,
                        parts: message_content_to_gemini_parts(content),
                    });
                }
                Message::User { content } => {
                    contents.push(GeminiContent {
                        role: Some("user".into()),
                        parts: message_content_to_gemini_parts(content),
                    });
                }
                Message::AI(ai) => {
                    let mut parts = Vec::new();
                    if !ai.content.is_empty() {
                        parts.push(text_part(ai.content.clone()));
                    }
                    for tc in &ai.tool_calls {
                        parts.push(GeminiPart {
                            text: None,
                            inline_data: None,
                            file_data: None,
                            function_call: Some(GeminiFunctionCall {
                                name: tc.name.clone(),
                                args: tc.arguments.clone(),
                            }),
                        });
                    }
                    if parts.is_empty() {
                        parts.push(text_part(String::new()));
                    }
                    contents.push(GeminiContent {
                        role: Some("model".into()),
                        parts,
                    });
                }
                Message::Tool { content, .. } => {
                    contents.push(GeminiContent {
                        role: Some("user".into()),
                        parts: vec![text_part(content.clone())],
                    });
                }
            }
        }

        let generation_config = if options.max_tokens.is_some()
            || options.temperature.is_some()
            || !options.stop.is_empty()
        {
            Some(GenerationConfig {
                max_output_tokens: options.max_tokens,
                temperature: options.temperature,
                stop_sequences: if options.stop.is_empty() {
                    None
                } else {
                    Some(options.stop.clone())
                },
            })
        } else {
            None
        };

        let tools = if options.tools.is_empty() {
            None
        } else {
            Some(vec![GeminiToolConfig {
                function_declarations: options
                    .tools
                    .iter()
                    .map(|t| GeminiFunctionDeclaration {
                        name: t.name.clone(),
                        description: t.description.clone(),
                        parameters: t.parameters.clone(),
                    })
                    .collect(),
            }])
        };

        GeminiRequest {
            system_instruction,
            contents,
            generation_config,
            tools,
        }
    }
}

#[async_trait]
impl ChatModel for GeminiChatModel {
    async fn generate(&self, messages: &[Message], options: &CallOptions) -> Result<ChatResult> {
        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
            self.model_id, self.api_key
        );

        let request_body = self.build_request(messages, options);

        let response = self
            .client
            .post(&url)
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
            return Err(AyasError::Model(
                if status.as_u16() == 401 || status.as_u16() == 403 {
                    ModelError::Auth(body)
                } else if status.as_u16() == 429 {
                    ModelError::RateLimited {
                        retry_after_secs: None,
                    }
                } else {
                    ModelError::ApiRequest(format!("HTTP {status}: {body}"))
                },
            ));
        }

        let gemini_response: GeminiResponse = response
            .json()
            .await
            .map_err(|e| AyasError::Model(ModelError::InvalidResponse(e.to_string())))?;

        let mut tool_calls = Vec::new();
        let mut text_parts = Vec::new();

        if let Some(candidates) = &gemini_response.candidates
            && let Some(candidate) = candidates.first()
        {
            for part in &candidate.content.parts {
                if let Some(fc) = &part.function_call {
                    tool_calls.push(ToolCall {
                        id: uuid::Uuid::new_v4().to_string(),
                        name: fc.name.clone(),
                        arguments: fc.args.clone(),
                    });
                }
                if let Some(text) = &part.text {
                    text_parts.push(text.clone());
                }
            }
        }

        let text = text_parts.join("");

        let usage = gemini_response.usage_metadata.map(|u| UsageMetadata {
            input_tokens: u.prompt_token_count,
            output_tokens: u.candidates_token_count,
            total_tokens: u.total_token_count,
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
        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:streamGenerateContent?alt=sse&key={}",
            self.model_id, self.api_key
        );

        let request_body = self.build_request(messages, options);

        let response = self
            .client
            .post(&url)
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
            return Err(AyasError::Model(
                if status.as_u16() == 401 || status.as_u16() == 403 {
                    ModelError::Auth(body)
                } else if status.as_u16() == 429 {
                    ModelError::RateLimited {
                        retry_after_secs: None,
                    }
                } else {
                    ModelError::ApiRequest(format!("HTTP {status}: {body}"))
                },
            ));
        }

        let data_stream = sse_data_stream(response);

        let event_stream = async_stream::stream! {
            let mut data_stream = Box::pin(data_stream);
            let mut last_usage: Option<UsageMetadata> = None;

            while let Some(data) = data_stream.next().await {
                let (events, usage) = parse_gemini_sse_data(&data);
                for event in events {
                    yield Ok(event);
                }
                if let Some(u) = usage {
                    last_usage = Some(u);
                }
            }

            // Emit usage at the end (Gemini sends cumulative usage per chunk)
            if let Some(usage) = last_usage {
                yield Ok(ChatStreamEvent::Usage(usage));
            }
            yield Ok(ChatStreamEvent::Done);
        };

        Ok(Box::pin(event_stream))
    }
}

/// Parse a single Gemini SSE data line into stream events (for testing).
///
/// Returns `(events, optional_usage)`. Usage is tracked separately since
/// Gemini sends cumulative usage in each chunk; the caller should emit
/// only the final value.
pub fn parse_gemini_sse_data(data: &str) -> (Vec<ChatStreamEvent>, Option<UsageMetadata>) {
    let mut events = Vec::new();
    let json: serde_json::Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(_) => return (events, None),
    };

    // Extract candidates
    if let Some(candidates) = json["candidates"].as_array() {
        if let Some(candidate) = candidates.first() {
            if let Some(parts) = candidate["content"]["parts"].as_array() {
                for part in parts {
                    if let Some(text) = part["text"].as_str() {
                        if !text.is_empty() {
                            events.push(ChatStreamEvent::Token(text.to_string()));
                        }
                    }
                    if let Some(fc) = part.get("functionCall") {
                        let name = fc["name"].as_str().unwrap_or("").to_string();
                        let id = uuid::Uuid::new_v4().to_string();
                        let args = fc.get("args").cloned().unwrap_or(serde_json::Value::Null);
                        events.push(ChatStreamEvent::ToolCallStart {
                            id: id.clone(),
                            name,
                        });
                        let args_str = serde_json::to_string(&args).unwrap_or_default();
                        events.push(ChatStreamEvent::ToolCallDelta {
                            id,
                            arguments: args_str,
                        });
                    }
                }
            }
        }
    }

    // Extract usage
    let usage = json.get("usageMetadata").and_then(|u| {
        Some(UsageMetadata {
            input_tokens: u["promptTokenCount"].as_u64()?,
            output_tokens: u["candidatesTokenCount"].as_u64().unwrap_or(0),
            total_tokens: u["totalTokenCount"].as_u64().unwrap_or(0),
        })
    });

    (events, usage)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use ayas_core::message::Message;
    use ayas_core::model::CallOptions;
    use ayas_core::tool::ToolDefinition;

    fn make_model() -> GeminiChatModel {
        GeminiChatModel::new("test-key".into(), "gemini-2.0-flash".into())
    }

    #[test]
    fn build_request_basic() {
        let model = make_model();
        let messages = vec![Message::user("Hello")];
        let options = CallOptions::default();
        let req = model.build_request(&messages, &options);
        assert_eq!(req.contents.len(), 1);
        assert_eq!(req.contents[0].role.as_deref(), Some("user"));
        assert!(req.system_instruction.is_none());
        assert!(req.generation_config.is_none());
        assert!(req.tools.is_none());
    }

    #[test]
    fn build_request_with_system() {
        let model = make_model();
        let messages = vec![
            Message::system("You are helpful"),
            Message::user("Hello"),
        ];
        let options = CallOptions::default();
        let req = model.build_request(&messages, &options);
        assert!(req.system_instruction.is_some());
        let sys = req.system_instruction.unwrap();
        assert!(sys.parts[0].text.as_deref() == Some("You are helpful"));
        assert_eq!(req.contents.len(), 1); // system not in contents
    }

    #[test]
    fn build_request_with_options() {
        let model = make_model();
        let messages = vec![Message::user("Hello")];
        let options = CallOptions {
            temperature: Some(0.5),
            max_tokens: Some(100),
            ..Default::default()
        };
        let req = model.build_request(&messages, &options);
        let config = req.generation_config.unwrap();
        assert_eq!(config.temperature, Some(0.5));
        assert_eq!(config.max_output_tokens, Some(100));
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
        assert_eq!(tools[0].function_declarations.len(), 1);
        assert_eq!(tools[0].function_declarations[0].name, "calculator");
    }

    #[test]
    fn parse_response_text() {
        let json = r#"{
            "candidates": [{
                "content": {
                    "parts": [{"text": "Hello world"}],
                    "role": "model"
                }
            }],
            "usageMetadata": {
                "promptTokenCount": 5,
                "candidatesTokenCount": 2,
                "totalTokenCount": 7
            }
        }"#;
        let resp: GeminiResponse = serde_json::from_str(json).unwrap();
        let text = resp
            .candidates
            .as_ref()
            .and_then(|c| c.first())
            .and_then(|c| c.content.parts.first())
            .and_then(|p| p.text.clone())
            .unwrap_or_default();
        assert_eq!(text, "Hello world");
    }

    #[test]
    fn parse_response_usage() {
        let json = r#"{
            "candidates": [{"content": {"parts": [{"text": "Hi"}]}}],
            "usageMetadata": {
                "promptTokenCount": 10,
                "candidatesTokenCount": 20,
                "totalTokenCount": 30
            }
        }"#;
        let resp: GeminiResponse = serde_json::from_str(json).unwrap();
        let usage = resp.usage_metadata.unwrap();
        assert_eq!(usage.prompt_token_count, 10);
        assert_eq!(usage.candidates_token_count, 20);
        assert_eq!(usage.total_token_count, 30);
    }

    #[test]
    fn parse_response_function_call() {
        let json = r#"{
            "candidates": [{
                "content": {
                    "parts": [{
                        "functionCall": {
                            "name": "calculator",
                            "args": {"expression": "2+2"}
                        }
                    }]
                }
            }]
        }"#;
        let resp: GeminiResponse = serde_json::from_str(json).unwrap();
        let part = &resp.candidates.unwrap()[0].content.parts[0];
        assert!(part.function_call.is_some());
        let fc = part.function_call.as_ref().unwrap();
        assert_eq!(fc.name, "calculator");
    }

    #[test]
    fn parse_response_empty_candidates() {
        let json = r#"{"candidates": []}"#;
        let resp: GeminiResponse = serde_json::from_str(json).unwrap();
        let text = resp
            .candidates
            .as_ref()
            .and_then(|c| c.first())
            .and_then(|c| c.content.parts.first())
            .and_then(|p| p.text.clone())
            .unwrap_or_default();
        assert_eq!(text, "");
    }

    // -----------------------------------------------------------------------
    // SSE parsing tests
    // -----------------------------------------------------------------------

    #[test]
    fn parse_sse_text_chunk() {
        let data = r#"{"candidates":[{"content":{"parts":[{"text":"Hello"}],"role":"model"}}],"usageMetadata":{"promptTokenCount":5,"candidatesTokenCount":1,"totalTokenCount":6}}"#;
        let (events, usage) = parse_gemini_sse_data(data);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], ChatStreamEvent::Token("Hello".into()));
        assert!(usage.is_some());
        let u = usage.unwrap();
        assert_eq!(u.input_tokens, 5);
        assert_eq!(u.output_tokens, 1);
    }

    #[test]
    fn parse_sse_function_call() {
        let data = r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"calculator","args":{"expression":"2+2"}}}],"role":"model"}}]}"#;
        let (events, _) = parse_gemini_sse_data(data);
        assert_eq!(events.len(), 2);
        assert!(matches!(&events[0], ChatStreamEvent::ToolCallStart { name, .. } if name == "calculator"));
        assert!(matches!(&events[1], ChatStreamEvent::ToolCallDelta { arguments, .. } if arguments.contains("expression")));
    }

    #[test]
    fn parse_sse_empty_text_skipped() {
        let data = r#"{"candidates":[{"content":{"parts":[{"text":""}],"role":"model"}}]}"#;
        let (events, _) = parse_gemini_sse_data(data);
        assert!(events.is_empty());
    }

    #[test]
    fn parse_sse_multiple_text_chunks() {
        let chunks = [
            r#"{"candidates":[{"content":{"parts":[{"text":"Hello"}],"role":"model"}}],"usageMetadata":{"promptTokenCount":5,"candidatesTokenCount":1,"totalTokenCount":6}}"#,
            r#"{"candidates":[{"content":{"parts":[{"text":" world!"}],"role":"model"}}],"usageMetadata":{"promptTokenCount":5,"candidatesTokenCount":3,"totalTokenCount":8}}"#,
        ];

        let mut all_events = Vec::new();
        let mut last_usage = None;

        for chunk in &chunks {
            let (events, usage) = parse_gemini_sse_data(chunk);
            all_events.extend(events);
            if usage.is_some() {
                last_usage = usage;
            }
        }

        assert_eq!(all_events.len(), 2);
        assert_eq!(all_events[0], ChatStreamEvent::Token("Hello".into()));
        assert_eq!(all_events[1], ChatStreamEvent::Token(" world!".into()));
        let u = last_usage.unwrap();
        assert_eq!(u.total_tokens, 8);
    }

    #[test]
    fn parse_sse_invalid_json() {
        let (events, usage) = parse_gemini_sse_data("not json");
        assert!(events.is_empty());
        assert!(usage.is_none());
    }
}
