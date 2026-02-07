use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use ayas_core::config::RunnableConfig;
use ayas_core::error::{AyasError, Result};
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
    pub agent: Option<String>,
    pub agent_config: Option<AgentConfig>,
    pub tools: Option<Vec<ToolConfig>>,
    pub previous_interaction_id: Option<String>,
}

impl DeepResearchInput {
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            agent: None,
            agent_config: None,
            tools: None,
            previous_interaction_id: None,
        }
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

        let mut request = CreateInteractionRequest::new(
            InteractionInput::Text(input.query),
            agent,
        );

        if let Some(config) = input.agent_config {
            request = request.with_agent_config(config);
        }
        if let Some(tools) = input.tools {
            request = request.with_tools(tools);
        }
        if let Some(prev_id) = input.previous_interaction_id {
            request = request.with_previous_interaction_id(prev_id);
        }

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
}
