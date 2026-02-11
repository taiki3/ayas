use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Streaming mode for graph execution output.
///
/// Multiple modes can be active simultaneously, allowing the caller to
/// receive different views of the execution in a single stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamMode {
    /// Emit the full state after each node execution.
    Values,
    /// Emit only the diff/update produced by each node.
    Updates,
    /// Emit LLM token-level streaming chunks.
    Messages,
    /// Emit internal debug events (node start/end, edge transitions).
    Debug,
}

impl std::fmt::Display for StreamMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Values => write!(f, "values"),
            Self::Updates => write!(f, "updates"),
            Self::Messages => write!(f, "messages"),
            Self::Debug => write!(f, "debug"),
        }
    }
}

impl std::str::FromStr for StreamMode {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "values" => Ok(Self::Values),
            "updates" => Ok(Self::Updates),
            "messages" => Ok(Self::Messages),
            "debug" => Ok(Self::Debug),
            other => Err(format!("unknown stream mode: '{other}'")),
        }
    }
}

/// Events emitted during graph execution streaming.
///
/// Each variant is tagged with the stream mode it belongs to so that
/// consumers receiving a multiplexed stream can filter by mode.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    /// Full state snapshot after a node runs (Values mode).
    Values { state: Value },
    /// Partial output produced by a single node (Updates mode).
    Updates { node: String, data: Value },
    /// A single LLM token/chunk (Messages mode).
    Message { chunk: String },
    /// Internal debug event (Debug mode).
    Debug {
        event_type: String,
        payload: Value,
    },
    /// The graph completed successfully.
    GraphComplete { output: Value },
    /// An error occurred during graph execution.
    Error { message: String },
}

impl StreamEvent {
    /// Returns the StreamMode this event belongs to, or None for
    /// terminal events (GraphComplete, Error) which are always emitted.
    pub fn mode(&self) -> Option<StreamMode> {
        match self {
            Self::Values { .. } => Some(StreamMode::Values),
            Self::Updates { .. } => Some(StreamMode::Updates),
            Self::Message { .. } => Some(StreamMode::Messages),
            Self::Debug { .. } => Some(StreamMode::Debug),
            Self::GraphComplete { .. } | Self::Error { .. } => None,
        }
    }
}

/// Parse a comma-separated stream mode string (e.g. "values,messages").
///
/// Returns default `[Values]` if the input is empty.
pub fn parse_stream_modes(s: &str) -> Result<Vec<StreamMode>, String> {
    if s.is_empty() {
        return Ok(vec![StreamMode::Values]);
    }
    s.split(',')
        .map(|part| part.trim().parse::<StreamMode>())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_mode_display() {
        assert_eq!(StreamMode::Values.to_string(), "values");
        assert_eq!(StreamMode::Updates.to_string(), "updates");
        assert_eq!(StreamMode::Messages.to_string(), "messages");
        assert_eq!(StreamMode::Debug.to_string(), "debug");
    }

    #[test]
    fn stream_mode_from_str() {
        assert_eq!("values".parse::<StreamMode>().unwrap(), StreamMode::Values);
        assert_eq!("updates".parse::<StreamMode>().unwrap(), StreamMode::Updates);
        assert_eq!("messages".parse::<StreamMode>().unwrap(), StreamMode::Messages);
        assert_eq!("debug".parse::<StreamMode>().unwrap(), StreamMode::Debug);
        assert!("unknown".parse::<StreamMode>().is_err());
    }

    #[test]
    fn stream_mode_serde_roundtrip() {
        let mode = StreamMode::Values;
        let json = serde_json::to_string(&mode).unwrap();
        assert_eq!(json, "\"values\"");
        let parsed: StreamMode = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, StreamMode::Values);
    }

    #[test]
    fn stream_event_serde_values() {
        let event = StreamEvent::Values {
            state: serde_json::json!({"count": 5}),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"values\""));
        let parsed: StreamEvent = serde_json::from_str(&json).unwrap();
        match parsed {
            StreamEvent::Values { state } => assert_eq!(state["count"], 5),
            _ => panic!("Expected Values event"),
        }
    }

    #[test]
    fn stream_event_serde_updates() {
        let event = StreamEvent::Updates {
            node: "my_node".into(),
            data: serde_json::json!({"delta": "abc"}),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"updates\""));
    }

    #[test]
    fn stream_event_serde_message() {
        let event = StreamEvent::Message {
            chunk: "Hello".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"message\""));
    }

    #[test]
    fn stream_event_serde_debug() {
        let event = StreamEvent::Debug {
            event_type: "node_start".into(),
            payload: serde_json::json!({"node": "a"}),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"debug\""));
    }

    #[test]
    fn stream_event_mode() {
        assert_eq!(
            StreamEvent::Values { state: serde_json::json!({}) }.mode(),
            Some(StreamMode::Values)
        );
        assert_eq!(
            StreamEvent::Updates { node: "n".into(), data: serde_json::json!({}) }.mode(),
            Some(StreamMode::Updates)
        );
        assert_eq!(
            StreamEvent::Message { chunk: "t".into() }.mode(),
            Some(StreamMode::Messages)
        );
        assert_eq!(
            StreamEvent::Debug { event_type: "e".into(), payload: serde_json::json!({}) }.mode(),
            Some(StreamMode::Debug)
        );
        assert_eq!(
            StreamEvent::GraphComplete { output: serde_json::json!({}) }.mode(),
            None
        );
        assert_eq!(
            StreamEvent::Error { message: "e".into() }.mode(),
            None
        );
    }

    #[test]
    fn parse_stream_modes_empty() {
        let modes = parse_stream_modes("").unwrap();
        assert_eq!(modes, vec![StreamMode::Values]);
    }

    #[test]
    fn parse_stream_modes_single() {
        let modes = parse_stream_modes("updates").unwrap();
        assert_eq!(modes, vec![StreamMode::Updates]);
    }

    #[test]
    fn parse_stream_modes_multiple() {
        let modes = parse_stream_modes("values,messages,debug").unwrap();
        assert_eq!(
            modes,
            vec![StreamMode::Values, StreamMode::Messages, StreamMode::Debug]
        );
    }

    #[test]
    fn parse_stream_modes_with_spaces() {
        let modes = parse_stream_modes("values , messages").unwrap();
        assert_eq!(modes, vec![StreamMode::Values, StreamMode::Messages]);
    }

    #[test]
    fn parse_stream_modes_invalid() {
        assert!(parse_stream_modes("values,bad").is_err());
    }
}
