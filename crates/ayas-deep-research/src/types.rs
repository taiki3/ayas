use serde::{Deserialize, Serialize};

/// Research task status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionStatus {
    InProgress,
    Completed,
    Failed,
}

/// Interaction input (text or multimodal).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum InteractionInput {
    Text(String),
    Multimodal(Vec<ContentPart>),
}

/// Multimodal content part.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text { text: String },
    Image { uri: String },
    File { uri: String },
    Video { uri: String },
}

/// Agent configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentConfig {
    #[serde(rename = "type")]
    pub agent_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_summaries: Option<String>,
}

/// Tool configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolConfig {
    FileSearch {
        file_search_store_names: Vec<String>,
    },
}

/// Request to create an interaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateInteractionRequest {
    pub input: InteractionInput,
    pub agent: String,
    pub background: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_interaction_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_config: Option<AgentConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolConfig>>,
}

impl CreateInteractionRequest {
    /// Create a minimal request with background=true.
    pub fn new(input: InteractionInput, agent: impl Into<String>) -> Self {
        Self {
            input,
            agent: agent.into(),
            background: true,
            stream: None,
            previous_interaction_id: None,
            agent_config: None,
            tools: None,
        }
    }

    pub fn with_stream(mut self, stream: bool) -> Self {
        self.stream = Some(stream);
        self
    }

    pub fn with_previous_interaction_id(mut self, id: impl Into<String>) -> Self {
        self.previous_interaction_id = Some(id.into());
        self
    }

    pub fn with_agent_config(mut self, config: AgentConfig) -> Self {
        self.agent_config = Some(config);
        self
    }

    pub fn with_tools(mut self, tools: Vec<ToolConfig>) -> Self {
        self.tools = Some(tools);
        self
    }
}

/// Interaction output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InteractionOutput {
    pub text: String,
}

/// Interaction response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Interaction {
    pub id: String,
    pub status: InteractionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outputs: Option<Vec<InteractionOutput>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// SSE event type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StreamEventType {
    #[serde(rename = "interaction.start")]
    InteractionStart,
    #[serde(rename = "content.delta")]
    ContentDelta,
    #[serde(rename = "interaction.complete")]
    InteractionComplete,
    #[serde(rename = "error")]
    Error,
}

/// SSE delta content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamDelta {
    #[serde(rename = "type")]
    pub delta_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// SSE stream event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamEvent {
    pub event_type: StreamEventType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta: Option<StreamDelta>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interaction: Option<Interaction>,
}

/// File Search Store metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSearchStore {
    pub name: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub active_documents_count: Option<String>,
    #[serde(default)]
    pub pending_documents_count: Option<String>,
    #[serde(default)]
    pub failed_documents_count: Option<String>,
}

/// Uploaded file metadata (from Files API).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadedFile {
    pub name: String,
    #[serde(default)]
    pub uri: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub mime_type: String,
}

/// Wrapper for Files API upload response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadFileResponse {
    pub file: UploadedFile,
}

/// Long-running operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Operation {
    pub name: String,
    #[serde(default)]
    pub done: bool,
    #[serde(default)]
    pub error: Option<OperationError>,
}

/// Operation error details.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationError {
    pub code: i32,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interaction_status_serde() {
        let status = InteractionStatus::InProgress;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"in_progress\"");

        let deserialized: InteractionStatus = serde_json::from_str("\"completed\"").unwrap();
        assert_eq!(deserialized, InteractionStatus::Completed);

        let deserialized: InteractionStatus = serde_json::from_str("\"failed\"").unwrap();
        assert_eq!(deserialized, InteractionStatus::Failed);
    }

    #[test]
    fn interaction_input_text_serde() {
        let input = InteractionInput::Text("hello".into());
        let json = serde_json::to_string(&input).unwrap();
        assert_eq!(json, "\"hello\"");

        let deserialized: InteractionInput = serde_json::from_str("\"hello\"").unwrap();
        assert!(matches!(deserialized, InteractionInput::Text(s) if s == "hello"));
    }

    #[test]
    fn interaction_input_multimodal_serde() {
        let input = InteractionInput::Multimodal(vec![
            ContentPart::Text {
                text: "describe this".into(),
            },
            ContentPart::Image {
                uri: "https://example.com/img.png".into(),
            },
        ]);
        let json = serde_json::to_string(&input).unwrap();
        let deserialized: InteractionInput = serde_json::from_str(&json).unwrap();
        match deserialized {
            InteractionInput::Multimodal(parts) => {
                assert_eq!(parts.len(), 2);
                assert!(matches!(&parts[0], ContentPart::Text { text } if text == "describe this"));
                assert!(matches!(&parts[1], ContentPart::Image { uri } if uri == "https://example.com/img.png"));
            }
            _ => panic!("expected multimodal"),
        }
    }

    #[test]
    fn content_part_serde_all_variants() {
        let parts = vec![
            ContentPart::Text {
                text: "hello".into(),
            },
            ContentPart::Image {
                uri: "img.png".into(),
            },
            ContentPart::File {
                uri: "doc.pdf".into(),
            },
            ContentPart::Video {
                uri: "clip.mp4".into(),
            },
        ];
        let json = serde_json::to_string(&parts).unwrap();
        let deserialized: Vec<ContentPart> = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.len(), 4);
        assert!(matches!(&deserialized[0], ContentPart::Text { text } if text == "hello"));
        assert!(matches!(&deserialized[3], ContentPart::Video { uri } if uri == "clip.mp4"));
    }

    #[test]
    fn create_interaction_request_builder() {
        let req = CreateInteractionRequest::new(
            InteractionInput::Text("research topic".into()),
            "deep-research-pro-preview-12-2025",
        )
        .with_stream(true)
        .with_previous_interaction_id("prev-123")
        .with_agent_config(AgentConfig {
            agent_type: "deep-research".into(),
            thinking_summaries: Some("auto".into()),
        })
        .with_tools(vec![ToolConfig::FileSearch {
            file_search_store_names: vec!["store1".into()],
        }]);

        assert_eq!(req.agent, "deep-research-pro-preview-12-2025");
        assert!(req.background);
        assert_eq!(req.stream, Some(true));
        assert_eq!(
            req.previous_interaction_id.as_deref(),
            Some("prev-123")
        );
        assert!(req.agent_config.is_some());
        assert!(req.tools.is_some());

        // Verify JSON roundtrip
        let json = serde_json::to_string(&req).unwrap();
        let deserialized: CreateInteractionRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.agent, "deep-research-pro-preview-12-2025");
    }

    #[test]
    fn stream_event_serde() {
        let event = StreamEvent {
            event_type: StreamEventType::ContentDelta,
            event_id: Some("evt-1".into()),
            delta: Some(StreamDelta {
                delta_type: "text".into(),
                text: Some("Hello world".into()),
            }),
            interaction: None,
        };
        let json = serde_json::to_string(&event).unwrap();
        let deserialized: StreamEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.event_type, StreamEventType::ContentDelta);
        assert_eq!(deserialized.event_id.as_deref(), Some("evt-1"));
        assert_eq!(
            deserialized.delta.as_ref().unwrap().text.as_deref(),
            Some("Hello world")
        );
    }

    #[test]
    fn file_search_store_json() {
        let json = r#"{
            "name": "fileSearchStores/abc123",
            "displayName": "my-store",
            "activeDocumentsCount": "2",
            "pendingDocumentsCount": "0",
            "failedDocumentsCount": "0"
        }"#;
        let store: FileSearchStore = serde_json::from_str(json).unwrap();
        assert_eq!(store.name, "fileSearchStores/abc123");
        assert_eq!(store.display_name, "my-store");
        assert_eq!(store.active_documents_count.as_deref(), Some("2"));
        assert_eq!(store.pending_documents_count.as_deref(), Some("0"));

        // Roundtrip
        let serialized = serde_json::to_string(&store).unwrap();
        let deserialized: FileSearchStore = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized.name, store.name);
    }

    #[test]
    fn uploaded_file_json() {
        let json = r#"{
            "name": "files/abc123",
            "uri": "https://generativelanguage.googleapis.com/v1beta/files/abc123",
            "displayName": "test.md",
            "mimeType": "text/markdown"
        }"#;
        let file: UploadedFile = serde_json::from_str(json).unwrap();
        assert_eq!(file.name, "files/abc123");
        assert_eq!(file.display_name, "test.md");
        assert_eq!(file.mime_type, "text/markdown");
    }

    #[test]
    fn operation_json_done() {
        let json = r#"{"name": "operations/op1", "done": true}"#;
        let op: Operation = serde_json::from_str(json).unwrap();
        assert_eq!(op.name, "operations/op1");
        assert!(op.done);
        assert!(op.error.is_none());
    }

    #[test]
    fn operation_json_with_error() {
        let json = r#"{
            "name": "operations/op2",
            "done": true,
            "error": {"code": 400, "message": "bad request"}
        }"#;
        let op: Operation = serde_json::from_str(json).unwrap();
        assert!(op.done);
        let err = op.error.unwrap();
        assert_eq!(err.code, 400);
        assert_eq!(err.message, "bad request");
    }

    #[test]
    fn operation_json_in_progress() {
        let json = r#"{"name": "operations/op3", "done": false}"#;
        let op: Operation = serde_json::from_str(json).unwrap();
        assert!(!op.done);
        assert!(op.error.is_none());
    }
}
