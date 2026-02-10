use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A snapshot of graph state at a particular execution step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    /// Unique identifier for this checkpoint.
    pub id: String,
    /// Thread (conversation) identifier. Multiple checkpoints share a thread.
    pub thread_id: String,
    /// Parent checkpoint ID, forming a linked-list history.
    pub parent_id: Option<String>,
    /// The super-step number at which this checkpoint was taken.
    pub step: usize,
    /// Snapshot of all channel values (key → serialized channel state).
    pub channel_values: HashMap<String, Value>,
    /// Names of the next nodes to execute after this checkpoint.
    pub pending_nodes: Vec<String>,
    /// Metadata about the checkpoint.
    pub metadata: CheckpointMetadata,
    /// When the checkpoint was created.
    pub created_at: DateTime<Utc>,
}

/// Metadata describing how a checkpoint was created.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointMetadata {
    /// Source of the checkpoint: "input", "loop", or "interrupt".
    pub source: String,
    /// The execution step number.
    pub step: usize,
    /// The node that was just executed (if applicable).
    pub node_name: Option<String>,
}

/// The outcome of a resumable graph execution.
#[derive(Debug, Clone)]
pub enum GraphOutput {
    /// Graph completed normally with final state.
    Complete(Value),
    /// Graph was interrupted by a node requesting human input.
    Interrupted {
        /// The checkpoint ID from which execution can be resumed.
        checkpoint_id: String,
        /// The value the interrupting node wants to present to the human.
        interrupt_value: Value,
        /// Current graph state at the point of interruption.
        state: Value,
    },
}

impl GraphOutput {
    /// Returns `true` if the graph completed normally.
    pub fn is_complete(&self) -> bool {
        matches!(self, GraphOutput::Complete(_))
    }

    /// Returns `true` if the graph was interrupted.
    pub fn is_interrupted(&self) -> bool {
        matches!(self, GraphOutput::Interrupted { .. })
    }

    /// Extract the final state value (panics if interrupted).
    pub fn into_value(self) -> Value {
        match self {
            GraphOutput::Complete(v) => v,
            GraphOutput::Interrupted { state, .. } => state,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn checkpoint_serde_roundtrip() {
        let cp = Checkpoint {
            id: "cp-1".into(),
            thread_id: "thread-1".into(),
            parent_id: None,
            step: 0,
            channel_values: HashMap::from([("count".into(), json!(42))]),
            pending_nodes: vec!["node_b".into()],
            metadata: CheckpointMetadata {
                source: "loop".into(),
                step: 0,
                node_name: Some("node_a".into()),
            },
            created_at: Utc::now(),
        };

        let json = serde_json::to_string(&cp).unwrap();
        let deserialized: Checkpoint = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, "cp-1");
        assert_eq!(deserialized.thread_id, "thread-1");
        assert_eq!(deserialized.channel_values["count"], json!(42));
    }

    #[test]
    fn graph_output_complete() {
        let output = GraphOutput::Complete(json!({"result": "done"}));
        assert!(output.is_complete());
        assert!(!output.is_interrupted());
        assert_eq!(output.into_value(), json!({"result": "done"}));
    }

    #[test]
    fn graph_output_interrupted() {
        let output = GraphOutput::Interrupted {
            checkpoint_id: "cp-1".into(),
            interrupt_value: json!({"question": "approve?"}),
            state: json!({"count": 5}),
        };
        assert!(output.is_interrupted());
        assert!(!output.is_complete());
    }
}
