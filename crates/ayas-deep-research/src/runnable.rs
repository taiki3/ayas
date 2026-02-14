use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use ayas_core::config::RunnableConfig;
use ayas_core::error::{AyasError, Result};
use ayas_core::message::{ContentPart, ContentSource};
use ayas_core::runnable::Runnable;

use crate::client::InteractionsClient;
use crate::types::{
    AgentConfig, CreateInteractionRequest, InteractionInput, InteractionStatus,
    ToolConfig,
};

const DEFAULT_AGENT: &str = "deep-research-pro-preview-12-2025";
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Input for the DeepResearchRunnable.
#[derive(Debug, Clone)]
pub struct DeepResearchInput {
    pub query: String,
    pub attachments: Vec<ContentPart>,
    pub agent: Option<String>,
    pub agent_config: Option<AgentConfig>,
    pub tools: Option<Vec<ToolConfig>>,
    pub previous_interaction_id: Option<String>,
}

impl DeepResearchInput {
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            attachments: Vec::new(),
            agent: None,
            agent_config: None,
            tools: None,
            previous_interaction_id: None,
        }
    }

    pub fn with_attachments(mut self, attachments: Vec<ContentPart>) -> Self {
        self.attachments = attachments;
        self
    }

    pub fn with_agent(mut self, agent: impl Into<String>) -> Self {
        self.agent = Some(agent.into());
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

    pub fn with_previous_interaction_id(mut self, id: impl Into<String>) -> Self {
        self.previous_interaction_id = Some(id.into());
        self
    }
}

/// Output from the DeepResearchRunnable.
#[derive(Debug, Clone)]
pub struct DeepResearchOutput {
    pub interaction_id: String,
    pub text: String,
    pub status: InteractionStatus,
}

/// Convert a core ContentPart to an Interactions API ContentPart.
fn to_interaction_part(part: &ContentPart) -> crate::types::ContentPart {
    match part {
        ContentPart::Text { text } => crate::types::ContentPart::Text { text: text.clone() },
        ContentPart::Image { source } => crate::types::ContentPart::Image {
            uri: source_to_uri(source),
        },
        ContentPart::File { source } => crate::types::ContentPart::File {
            uri: source_to_uri(source),
        },
    }
}

/// Convert a core ContentSource to a URI string.
fn source_to_uri(source: &ContentSource) -> String {
    match source {
        ContentSource::Url { url, .. } => url.clone(),
        ContentSource::Base64 { media_type, data } => {
            format!("data:{};base64,{}", media_type, data)
        }
        ContentSource::FileId { file_id } => file_id.clone(),
    }
}

/// Runnable that wraps the Interactions API for deep research.
pub struct DeepResearchRunnable {
    client: Arc<dyn InteractionsClient>,
    default_agent: String,
    poll_interval: Duration,
}

impl DeepResearchRunnable {
    pub fn new(client: Arc<dyn InteractionsClient>) -> Self {
        Self {
            client,
            default_agent: DEFAULT_AGENT.into(),
            poll_interval: DEFAULT_POLL_INTERVAL,
        }
    }

    pub fn with_agent(mut self, agent: impl Into<String>) -> Self {
        self.default_agent = agent.into();
        self
    }

    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }
}

#[async_trait]
impl Runnable for DeepResearchRunnable {
    type Input = DeepResearchInput;
    type Output = DeepResearchOutput;

    async fn invoke(
        &self,
        input: Self::Input,
        _config: &RunnableConfig,
    ) -> Result<Self::Output> {
        let agent = input.agent.unwrap_or_else(|| self.default_agent.clone());
        tracing::info!(agent = %agent, attachments = input.attachments.len(), "DeepResearch invoke start");

        let interaction_input = if input.attachments.is_empty() {
            InteractionInput::Text(input.query)
        } else {
            let mut parts = vec![crate::types::ContentPart::Text { text: input.query }];
            parts.extend(input.attachments.iter().map(to_interaction_part));
            InteractionInput::Multimodal(parts)
        };

        let mut request = CreateInteractionRequest::new(interaction_input, agent);

        if let Some(config) = input.agent_config {
            request = request.with_agent_config(config);
        }
        if let Some(tools) = input.tools {
            request = request.with_tools(tools);
        }
        if let Some(prev_id) = input.previous_interaction_id {
            request = request.with_previous_interaction_id(prev_id);
        }

        tracing::info!("Calling create_and_poll");
        let interaction = self
            .client
            .create_and_poll(&request, self.poll_interval)
            .await?;

        let text = interaction
            .outputs
            .as_ref()
            .and_then(|outputs| outputs.first())
            .map(|o| o.text.clone())
            .ok_or_else(|| {
                AyasError::Other(format!(
                    "Interaction {} completed but has no outputs",
                    interaction.id
                ))
            })?;

        Ok(DeepResearchOutput {
            interaction_id: interaction.id,
            text,
            status: interaction.status,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::MockInteractionsClient;

    #[tokio::test]
    async fn invoke_success() {
        let client = Arc::new(MockInteractionsClient::completed("Deep research result"));
        let runnable = DeepResearchRunnable::new(client)
            .with_poll_interval(Duration::from_millis(1));

        let input = DeepResearchInput::new("What is quantum computing?");
        let config = RunnableConfig::default();

        let output = runnable.invoke(input, &config).await.unwrap();
        assert_eq!(output.text, "Deep research result");
        assert_eq!(output.status, InteractionStatus::Completed);
        assert_eq!(output.interaction_id, "mock-interaction-1");
    }

    #[tokio::test]
    async fn invoke_failure() {
        let client = Arc::new(MockInteractionsClient::failing("API error"));
        let runnable = DeepResearchRunnable::new(client)
            .with_poll_interval(Duration::from_millis(1));

        let input = DeepResearchInput::new("test query");
        let config = RunnableConfig::default();

        let result = runnable.invoke(input, &config).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("API error"));
    }

    #[tokio::test]
    async fn invoke_with_custom_settings() {
        let client = Arc::new(MockInteractionsClient::with_polling(2, "custom result"));
        let runnable = DeepResearchRunnable::new(client)
            .with_agent("custom-agent")
            .with_poll_interval(Duration::from_millis(1));

        let input = DeepResearchInput::new("test query")
            .with_agent("override-agent")
            .with_agent_config(AgentConfig {
                agent_type: "deep-research".into(),
                thinking_summaries: Some("auto".into()),
            })
            .with_previous_interaction_id("prev-123");

        let config = RunnableConfig::default();
        let output = runnable.invoke(input, &config).await.unwrap();
        assert_eq!(output.text, "custom result");
        assert_eq!(output.status, InteractionStatus::Completed);
    }

    // --- New multimodal tests ---

    #[test]
    fn with_attachments_builder() {
        let input = DeepResearchInput::new("test query").with_attachments(vec![
            ContentPart::Image {
                source: ContentSource::Url {
                    url: "https://example.com/img.png".into(),
                    detail: None,
                },
            },
            ContentPart::File {
                source: ContentSource::Base64 {
                    media_type: "application/pdf".into(),
                    data: "base64data".into(),
                },
            },
        ]);
        assert_eq!(input.attachments.len(), 2);
        assert_eq!(input.query, "test query");
    }

    #[test]
    fn default_no_attachments() {
        let input = DeepResearchInput::new("test query");
        assert!(input.attachments.is_empty());
    }

    #[test]
    fn source_to_uri_variants() {
        assert_eq!(
            source_to_uri(&ContentSource::Url {
                url: "https://example.com".into(),
                detail: Some("high".into()),
            }),
            "https://example.com"
        );
        assert_eq!(
            source_to_uri(&ContentSource::Base64 {
                media_type: "image/png".into(),
                data: "abc123".into(),
            }),
            "data:image/png;base64,abc123"
        );
        assert_eq!(
            source_to_uri(&ContentSource::FileId {
                file_id: "file-xyz".into(),
            }),
            "file-xyz"
        );
    }

    #[test]
    fn to_interaction_part_conversion() {
        let text_part = to_interaction_part(&ContentPart::Text {
            text: "hello".into(),
        });
        assert!(matches!(
            text_part,
            crate::types::ContentPart::Text { text } if text == "hello"
        ));

        let image_part = to_interaction_part(&ContentPart::Image {
            source: ContentSource::Url {
                url: "https://example.com/img.png".into(),
                detail: None,
            },
        });
        assert!(matches!(
            image_part,
            crate::types::ContentPart::Image { uri } if uri == "https://example.com/img.png"
        ));

        let file_part = to_interaction_part(&ContentPart::File {
            source: ContentSource::FileId {
                file_id: "file-id-1".into(),
            },
        });
        assert!(matches!(
            file_part,
            crate::types::ContentPart::File { uri } if uri == "file-id-1"
        ));
    }
}
