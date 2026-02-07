use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::Mutex;

use async_trait::async_trait;
use futures::Stream;

use ayas_core::error::Result;

use crate::client::InteractionsClient;
use crate::types::{
    CreateInteractionRequest, Interaction, InteractionOutput, InteractionStatus,
    StreamEvent,
};

/// Mock client for testing without HTTP.
pub struct MockInteractionsClient {
    responses: Mutex<VecDeque<Interaction>>,
    stream_events: Mutex<Option<Vec<StreamEvent>>>,
}

impl MockInteractionsClient {
    /// Immediately returns a completed interaction.
    pub fn completed(text: impl Into<String>) -> Self {
        let interaction = Interaction {
            id: "mock-interaction-1".into(),
            status: InteractionStatus::Completed,
            outputs: Some(vec![InteractionOutput {
                text: text.into(),
            }]),
            error: None,
        };
        Self {
            responses: Mutex::new(VecDeque::from([interaction])),
            stream_events: Mutex::new(None),
        }
    }

    /// Returns InProgress `steps` times, then Completed with the given text.
    pub fn with_polling(steps: usize, text: impl Into<String>) -> Self {
        let text = text.into();
        let mut responses = VecDeque::new();

        // First response from create()
        responses.push_back(Interaction {
            id: "mock-interaction-1".into(),
            status: InteractionStatus::InProgress,
            outputs: None,
            error: None,
        });

        // Intermediate poll responses
        for _ in 1..steps {
            responses.push_back(Interaction {
                id: "mock-interaction-1".into(),
                status: InteractionStatus::InProgress,
                outputs: None,
                error: None,
            });
        }

        // Final completed response
        responses.push_back(Interaction {
            id: "mock-interaction-1".into(),
            status: InteractionStatus::Completed,
            outputs: Some(vec![InteractionOutput { text }]),
            error: None,
        });

        Self {
            responses: Mutex::new(responses),
            stream_events: Mutex::new(None),
        }
    }

    /// Returns a failed interaction immediately.
    pub fn failing(error_msg: impl Into<String>) -> Self {
        let interaction = Interaction {
            id: "mock-interaction-1".into(),
            status: InteractionStatus::Failed,
            outputs: None,
            error: Some(error_msg.into()),
        };
        Self {
            responses: Mutex::new(VecDeque::from([interaction])),
            stream_events: Mutex::new(None),
        }
    }

    /// Returns the given SSE events as a stream.
    pub fn with_stream(events: Vec<StreamEvent>) -> Self {
        Self {
            responses: Mutex::new(VecDeque::new()),
            stream_events: Mutex::new(Some(events)),
        }
    }

    fn next_response(&self) -> Interaction {
        let mut responses = self.responses.lock().unwrap();
        responses
            .pop_front()
            .unwrap_or_else(|| Interaction {
                id: "mock-interaction-1".into(),
                status: InteractionStatus::Completed,
                outputs: Some(vec![InteractionOutput {
                    text: "default".into(),
                }]),
                error: None,
            })
    }
}

#[async_trait]
impl InteractionsClient for MockInteractionsClient {
    async fn create(&self, _request: &CreateInteractionRequest) -> Result<Interaction> {
        Ok(self.next_response())
    }

    async fn get(&self, _interaction_id: &str) -> Result<Interaction> {
        Ok(self.next_response())
    }

    async fn create_stream(
        &self,
        _request: &CreateInteractionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let events = self
            .stream_events
            .lock()
            .unwrap()
            .take()
            .unwrap_or_default();
        let stream = futures::stream::iter(events.into_iter().map(Ok));
        Ok(Box::pin(stream))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        InteractionInput, InteractionStatus, StreamDelta, StreamEventType,
    };
    use futures::StreamExt;

    #[tokio::test]
    async fn completed_returns_immediately() {
        let client = MockInteractionsClient::completed("research result");
        let req = CreateInteractionRequest::new(
            InteractionInput::Text("test".into()),
            "test-agent",
        );

        let result = client.create(&req).await.unwrap();
        assert_eq!(result.status, InteractionStatus::Completed);
        assert_eq!(result.outputs.unwrap()[0].text, "research result");
    }

    #[tokio::test]
    async fn with_polling_returns_in_progress_then_completed() {
        let client = MockInteractionsClient::with_polling(2, "done");
        let req = CreateInteractionRequest::new(
            InteractionInput::Text("test".into()),
            "test-agent",
        );

        // First call (create) returns InProgress
        let r1 = client.create(&req).await.unwrap();
        assert_eq!(r1.status, InteractionStatus::InProgress);

        // Second call (get) returns InProgress
        let r2 = client.get("mock-interaction-1").await.unwrap();
        assert_eq!(r2.status, InteractionStatus::InProgress);

        // Third call (get) returns Completed
        let r3 = client.get("mock-interaction-1").await.unwrap();
        assert_eq!(r3.status, InteractionStatus::Completed);
        assert_eq!(r3.outputs.unwrap()[0].text, "done");
    }

    #[tokio::test]
    async fn stream_returns_events() {
        let events = vec![
            StreamEvent {
                event_type: StreamEventType::InteractionStart,
                event_id: Some("evt-1".into()),
                delta: None,
                interaction: Some(Interaction {
                    id: "int-1".into(),
                    status: InteractionStatus::InProgress,
                    outputs: None,
                    error: None,
                }),
            },
            StreamEvent {
                event_type: StreamEventType::ContentDelta,
                event_id: Some("evt-2".into()),
                delta: Some(StreamDelta {
                    delta_type: "text".into(),
                    text: Some("Hello".into()),
                }),
                interaction: None,
            },
            StreamEvent {
                event_type: StreamEventType::InteractionComplete,
                event_id: Some("evt-3".into()),
                delta: None,
                interaction: Some(Interaction {
                    id: "int-1".into(),
                    status: InteractionStatus::Completed,
                    outputs: Some(vec![InteractionOutput {
                        text: "Hello World".into(),
                    }]),
                    error: None,
                }),
            },
        ];

        let client = MockInteractionsClient::with_stream(events);
        let req = CreateInteractionRequest::new(
            InteractionInput::Text("test".into()),
            "test-agent",
        );

        let mut stream = client.create_stream(&req).await.unwrap();
        let e1 = stream.next().await.unwrap().unwrap();
        assert_eq!(e1.event_type, StreamEventType::InteractionStart);

        let e2 = stream.next().await.unwrap().unwrap();
        assert_eq!(e2.event_type, StreamEventType::ContentDelta);
        assert_eq!(e2.delta.unwrap().text.as_deref(), Some("Hello"));

        let e3 = stream.next().await.unwrap().unwrap();
        assert_eq!(e3.event_type, StreamEventType::InteractionComplete);

        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn failing_returns_failed_status() {
        let client = MockInteractionsClient::failing("timeout");
        let req = CreateInteractionRequest::new(
            InteractionInput::Text("test".into()),
            "test-agent",
        );

        let result = client.create(&req).await.unwrap();
        assert_eq!(result.status, InteractionStatus::Failed);
        assert_eq!(result.error.as_deref(), Some("timeout"));
    }

    #[tokio::test]
    async fn create_and_poll_with_mock() {
        let client = MockInteractionsClient::with_polling(2, "final");
        let req = CreateInteractionRequest::new(
            InteractionInput::Text("test".into()),
            "test-agent",
        );

        let result = client
            .create_and_poll(&req, std::time::Duration::from_millis(1))
            .await
            .unwrap();
        assert_eq!(result.status, InteractionStatus::Completed);
        assert_eq!(result.outputs.unwrap()[0].text, "final");
    }
}
