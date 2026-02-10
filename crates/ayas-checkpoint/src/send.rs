use serde_json::{json, Value};

/// The key used in node output to signal send directives.
pub const SEND_KEY: &str = "__send__";

/// A directive to send execution to a specific node with private input.
#[derive(Debug, Clone)]
pub struct SendDirective {
    /// Target node name to execute.
    pub node: String,
    /// Private input to merge into state before executing the target node.
    pub input: Value,
}

impl SendDirective {
    /// Create a new send directive.
    pub fn new(node: impl Into<String>, input: Value) -> Self {
        Self {
            node: node.into(),
            input,
        }
    }
}

/// Create a send output from a node.
///
/// The node's output instructs the Pregel loop to execute each send target
/// sequentially with its own private input merged into the current state.
///
/// # Example
///
/// ```
/// use serde_json::json;
/// use ayas_checkpoint::send::{send_output, SendDirective};
///
/// let output = send_output(vec![
///     SendDirective::new("worker_a", json!({"task": "summarize"})),
///     SendDirective::new("worker_b", json!({"task": "translate"})),
/// ]);
/// assert!(output.get("__send__").is_some());
/// ```
pub fn send_output(sends: Vec<SendDirective>) -> Value {
    let arr: Vec<Value> = sends
        .iter()
        .map(|s| json!({"node": s.node, "input": s.input}))
        .collect();
    json!({ SEND_KEY: arr })
}

/// Check if a node output contains send directives.
pub fn is_send(output: &Value) -> bool {
    output.get(SEND_KEY).is_some()
}

/// Extract send directives from a node output.
pub fn extract_sends(output: &Value) -> Option<Vec<SendDirective>> {
    let arr = output.get(SEND_KEY)?.as_array()?;
    let mut sends = Vec::new();
    for item in arr {
        let node = item.get("node")?.as_str()?.to_string();
        let input = item.get("input")?.clone();
        sends.push(SendDirective { node, input });
    }
    Some(sends)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn send_output_produces_correct_json() {
        let output = send_output(vec![
            SendDirective::new("worker_a", json!({"task": "summarize"})),
            SendDirective::new("worker_b", json!({"task": "translate"})),
        ]);

        assert!(output.get(SEND_KEY).is_some());
        let arr = output.get(SEND_KEY).unwrap().as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["node"], "worker_a");
        assert_eq!(arr[0]["input"]["task"], "summarize");
        assert_eq!(arr[1]["node"], "worker_b");
        assert_eq!(arr[1]["input"]["task"], "translate");
    }

    #[test]
    fn is_send_on_send_output() {
        let output = send_output(vec![SendDirective::new("target", json!({}))]);
        assert!(is_send(&output));
    }

    #[test]
    fn is_send_on_normal_output() {
        let output = json!({"result": "done"});
        assert!(!is_send(&output));
    }

    #[test]
    fn is_send_on_empty_object() {
        assert!(!is_send(&json!({})));
    }

    #[test]
    fn is_send_on_null() {
        assert!(!is_send(&Value::Null));
    }

    #[test]
    fn extract_sends_from_send_output() {
        let output = send_output(vec![
            SendDirective::new("a", json!({"x": 1})),
            SendDirective::new("b", json!({"y": 2})),
        ]);

        let sends = extract_sends(&output).unwrap();
        assert_eq!(sends.len(), 2);
        assert_eq!(sends[0].node, "a");
        assert_eq!(sends[0].input, json!({"x": 1}));
        assert_eq!(sends[1].node, "b");
        assert_eq!(sends[1].input, json!({"y": 2}));
    }

    #[test]
    fn extract_sends_from_normal_output() {
        assert!(extract_sends(&json!({"result": "done"})).is_none());
    }

    #[test]
    fn extract_sends_empty_array() {
        let output = json!({ SEND_KEY: [] });
        let sends = extract_sends(&output).unwrap();
        assert!(sends.is_empty());
    }

    #[test]
    fn extract_sends_missing_node_field() {
        let output = json!({ SEND_KEY: [{"input": {}}] });
        assert!(extract_sends(&output).is_none());
    }

    #[test]
    fn extract_sends_missing_input_field() {
        let output = json!({ SEND_KEY: [{"node": "a"}] });
        assert!(extract_sends(&output).is_none());
    }

    #[test]
    fn send_output_empty_vec() {
        let output = send_output(vec![]);
        assert!(is_send(&output));
        let sends = extract_sends(&output).unwrap();
        assert!(sends.is_empty());
    }

    #[test]
    fn send_output_with_complex_input() {
        let output = send_output(vec![SendDirective::new(
            "worker",
            json!({
                "messages": [{"role": "user", "content": "hello"}],
                "config": {"temperature": 0.7}
            }),
        )]);
        let sends = extract_sends(&output).unwrap();
        assert_eq!(sends.len(), 1);
        assert_eq!(sends[0].input["messages"][0]["content"], "hello");
    }
}
