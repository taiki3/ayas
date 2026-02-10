use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Events emitted during graph execution streaming.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    /// A node is about to execute.
    NodeStart { node_name: String, step: usize },
    /// A node has finished executing.
    NodeEnd {
        node_name: String,
        step: usize,
        state: Value,
    },
    /// The graph completed successfully.
    GraphComplete { output: Value },
    /// The graph was interrupted (HITL).
    Interrupted {
        checkpoint_id: String,
        interrupt_value: Value,
    },
    /// An error occurred.
    Error { message: String },
}
