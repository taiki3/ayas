use std::sync::Arc;
use std::time::Duration;

use ayas_core::config::RunnableConfig;
use ayas_core::runnable::Runnable;
use futures::StreamExt;

use ayas_deep_research::mock::MockInteractionsClient;
use ayas_deep_research::client::InteractionsClient;
use ayas_deep_research::runnable::{DeepResearchInput, DeepResearchRunnable};
use ayas_deep_research::types::*;

#[tokio::test]
async fn full_cycle_e2e_with_polling() {
    // Simulate 3 polling steps before completion
    let client = Arc::new(MockInteractionsClient::with_polling(
        3,
        "# Research Report\n\nQuantum computing uses qubits...",
    ));
    let runnable = DeepResearchRunnable::new(client)
        .with_poll_interval(Duration::from_millis(1));

    let input = DeepResearchInput::new("Explain quantum computing");
    let config = RunnableConfig::default();

    let output = runnable.invoke(input, &config).await.unwrap();
    assert_eq!(output.status, InteractionStatus::Completed);
    assert!(output.text.contains("Quantum"));
    assert_eq!(output.interaction_id, "mock-interaction-1");
}

#[tokio::test]
async fn stream_e2e() {
    let events = vec![
        StreamEvent {
            event_type: StreamEventType::InteractionStart,
            event_id: Some("evt-1".into()),
            delta: None,
            interaction: Some(Interaction {
                id: "int-stream-1".into(),
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
                text: Some("First chunk. ".into()),
            }),
            interaction: None,
        },
        StreamEvent {
            event_type: StreamEventType::ContentDelta,
            event_id: Some("evt-3".into()),
            delta: Some(StreamDelta {
                delta_type: "text".into(),
                text: Some("Second chunk.".into()),
            }),
            interaction: None,
        },
        StreamEvent {
            event_type: StreamEventType::InteractionComplete,
            event_id: Some("evt-4".into()),
            delta: None,
            interaction: Some(Interaction {
                id: "int-stream-1".into(),
                status: InteractionStatus::Completed,
                outputs: Some(vec![InteractionOutput {
                    text: "First chunk. Second chunk.".into(),
                }]),
                error: None,
            }),
        },
    ];

    let client = MockInteractionsClient::with_stream(events);
    let req = CreateInteractionRequest::new(
        InteractionInput::Text("test query".into()),
        "deep-research-pro-preview-12-2025",
    );

    let mut stream = client.create_stream(&req).await.unwrap();

    // Collect all events
    let mut collected = Vec::new();
    while let Some(event) = stream.next().await {
        collected.push(event.unwrap());
    }

    assert_eq!(collected.len(), 4);
    assert_eq!(collected[0].event_type, StreamEventType::InteractionStart);
    assert_eq!(collected[1].event_type, StreamEventType::ContentDelta);
    assert_eq!(collected[2].event_type, StreamEventType::ContentDelta);
    assert_eq!(collected[3].event_type, StreamEventType::InteractionComplete);

    // Verify we can reconstruct text from deltas
    let text: String = collected
        .iter()
        .filter_map(|e| e.delta.as_ref())
        .filter_map(|d| d.text.as_deref())
        .collect();
    assert_eq!(text, "First chunk. Second chunk.");

    // Verify final interaction has complete output
    let final_interaction = collected[3].interaction.as_ref().unwrap();
    assert_eq!(final_interaction.status, InteractionStatus::Completed);
    assert_eq!(
        final_interaction.outputs.as_ref().unwrap()[0].text,
        "First chunk. Second chunk."
    );
}

#[tokio::test]
async fn follow_up_interaction() {
    let client = Arc::new(MockInteractionsClient::completed(
        "Follow-up research result with more details.",
    ));
    let runnable = DeepResearchRunnable::new(client)
        .with_poll_interval(Duration::from_millis(1));

    let input = DeepResearchInput::new("Tell me more about quantum entanglement")
        .with_previous_interaction_id("prev-interaction-123")
        .with_agent_config(AgentConfig {
            agent_type: "deep-research".into(),
            thinking_summaries: Some("auto".into()),
        });

    let config = RunnableConfig::default();
    let output = runnable.invoke(input, &config).await.unwrap();

    assert_eq!(output.status, InteractionStatus::Completed);
    assert!(output.text.contains("Follow-up"));
}
