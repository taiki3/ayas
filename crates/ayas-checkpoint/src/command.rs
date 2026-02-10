use serde_json::{json, Value};

/// The key used in node output to signal a command (state update + routing).
pub const COMMAND_KEY: &str = "__command__";

/// Create a command output from a node.
///
/// The node's output instructs the Pregel loop to apply `update` to channels
/// AND route to the `goto` node, bypassing normal edge resolution.
///
/// # Example
///
/// ```
/// use serde_json::json;
/// use ayas_checkpoint::command::command_output;
///
/// let output = command_output(json!({"count": 5}), "next_node");
/// assert!(output.get("__command__").is_some());
/// ```
pub fn command_output(update: Value, goto: impl Into<String>) -> Value {
    json!({
        COMMAND_KEY: {
            "update": update,
            "goto": goto.into()
        }
    })
}

/// Check if a node output contains a command.
pub fn is_command(output: &Value) -> bool {
    output.get(COMMAND_KEY).is_some()
}

/// Extract command parts (update, goto) from a node output.
pub fn extract_command(output: &Value) -> Option<(Value, String)> {
    let cmd = output.get(COMMAND_KEY)?;
    let update = cmd.get("update")?.clone();
    let goto = cmd.get("goto")?.as_str()?.to_string();
    Some((update, goto))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn command_output_produces_correct_json() {
        let update = json!({"count": 5, "name": "test"});
        let output = command_output(update.clone(), "next_node");

        assert!(output.get(COMMAND_KEY).is_some());
        let cmd = output.get(COMMAND_KEY).unwrap();
        assert_eq!(cmd.get("update").unwrap(), &update);
        assert_eq!(cmd.get("goto").unwrap(), "next_node");
    }

    #[test]
    fn is_command_on_command_output() {
        let output = command_output(json!({}), "target");
        assert!(is_command(&output));
    }

    #[test]
    fn is_command_on_normal_output() {
        let output = json!({"result": "done"});
        assert!(!is_command(&output));
    }

    #[test]
    fn is_command_on_empty_object() {
        let output = json!({});
        assert!(!is_command(&output));
    }

    #[test]
    fn is_command_on_null() {
        assert!(!is_command(&Value::Null));
    }

    #[test]
    fn extract_command_from_command_output() {
        let update = json!({"count": 42});
        let output = command_output(update.clone(), "target_node");

        let (extracted_update, extracted_goto) = extract_command(&output).unwrap();
        assert_eq!(extracted_update, update);
        assert_eq!(extracted_goto, "target_node");
    }

    #[test]
    fn extract_command_from_normal_output() {
        let output = json!({"result": "done"});
        assert!(extract_command(&output).is_none());
    }

    #[test]
    fn extract_command_missing_update() {
        let output = json!({ COMMAND_KEY: { "goto": "target" } });
        assert!(extract_command(&output).is_none());
    }

    #[test]
    fn extract_command_missing_goto() {
        let output = json!({ COMMAND_KEY: { "update": {} } });
        assert!(extract_command(&output).is_none());
    }

    #[test]
    fn extract_command_goto_not_string() {
        let output = json!({ COMMAND_KEY: { "update": {}, "goto": 42 } });
        assert!(extract_command(&output).is_none());
    }

    #[test]
    fn command_output_with_empty_update() {
        let output = command_output(json!({}), "end");
        let (update, goto) = extract_command(&output).unwrap();
        assert_eq!(update, json!({}));
        assert_eq!(goto, "end");
    }

    #[test]
    fn command_output_with_nested_update() {
        let update = json!({
            "messages": [{"role": "assistant", "content": "hello"}],
            "count": 5
        });
        let output = command_output(update.clone(), "process");
        let (extracted, _) = extract_command(&output).unwrap();
        assert_eq!(extracted, update);
    }
}
