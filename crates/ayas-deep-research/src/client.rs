use std::pin::Pin;
use std::time::Duration;

use async_trait::async_trait;
use futures::Stream;
use tracing::{info, warn};

use ayas_core::error::{AyasError, Result};

use crate::types::{
    CreateInteractionRequest, Interaction, InteractionStatus, StreamEvent,
};

/// Client trait for the Interactions API.
#[async_trait]
pub trait InteractionsClient: Send + Sync {
    /// POST /interactions — create a new interaction.
    async fn create(&self, request: &CreateInteractionRequest) -> Result<Interaction>;

    /// GET /interactions/{id} — get interaction status.
    async fn get(&self, interaction_id: &str) -> Result<Interaction>;

    /// Create and poll until completion.
    async fn create_and_poll(
        &self,
        request: &CreateInteractionRequest,
        poll_interval: Duration,
    ) -> Result<Interaction> {
        let interaction = self.create(request).await?;
        info!(id = %interaction.id, status = ?interaction.status, "Interaction created");
        let mut current = interaction;
        let mut poll_count = 0u32;

        loop {
            match current.status {
                InteractionStatus::Completed => {
                    info!(id = %current.id, poll_count, "Interaction completed");
                    return Ok(current);
                }
                InteractionStatus::Failed => {
                    let msg = current
                        .error
                        .unwrap_or_else(|| "unknown error".into());
                    warn!(id = %current.id, poll_count, error = %msg, "Interaction failed");
                    return Err(AyasError::Other(format!(
                        "Interaction {} failed: {}",
                        current.id, msg
                    )));
                }
                InteractionStatus::InProgress => {
                    tokio::time::sleep(poll_interval).await;
                    poll_count += 1;
                    match self.get(&current.id).await {
                        Ok(updated) => {
                            if poll_count % 12 == 0 {
                                info!(id = %updated.id, poll_count, status = ?updated.status, "Polling...");
                            }
                            current = updated;
                        }
                        Err(e) => {
                            warn!(id = %current.id, poll_count, error = %e, "Poll GET failed");
                            return Err(e);
                        }
                    }
                }
            }
        }
    }

    /// SSE stream for receiving events.
    async fn create_stream(
        &self,
        request: &CreateInteractionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::MockInteractionsClient;
    use crate::types::InteractionStatus;

    #[tokio::test]
    async fn create_and_poll_default_impl_polls_until_complete() {
        let client = MockInteractionsClient::with_polling(3, "final result");
        let req = CreateInteractionRequest::new(
            crate::types::InteractionInput::Text("test".into()),
            "test-agent",
        );

        let result = client
            .create_and_poll(&req, Duration::from_millis(1))
            .await
            .unwrap();

        assert_eq!(result.status, InteractionStatus::Completed);
        assert_eq!(
            result.outputs.as_ref().unwrap()[0].text,
            "final result"
        );
    }

    #[tokio::test]
    async fn create_and_poll_returns_error_on_failure() {
        let client = MockInteractionsClient::failing("something went wrong");
        let req = CreateInteractionRequest::new(
            crate::types::InteractionInput::Text("test".into()),
            "test-agent",
        );

        let result = client
            .create_and_poll(&req, Duration::from_millis(1))
            .await;

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("something went wrong"));
    }
}
