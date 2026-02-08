use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use ayas_core::error::{AyasError, ModelError, Result};
use ayas_core::message::{
    AIContent, ContentPart, ContentSource, Message, MessageContent, ToolCall, UsageMetadata,
};
use ayas_core::model::{CallOptions, ChatModel, ChatResult};

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
}
