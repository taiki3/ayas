use serde_json::{json, Value};

/// The key used in node output to signal an interrupt.
pub const INTERRUPT_KEY: &str = "__interrupt__";

/// Create an interrupt output from a node.
/// Use this in node functions to signal that human input is needed.
///
/// # Example
///
/// ```
/// use serde_json::json;
/// use ayas_checkpoint::interrupt::interrupt_output;
///
/// let output = interrupt_output(json!({"question": "Approve this summary?"}));
/// assert!(output.get("__interrupt__").is_some());
/// ```
pub fn interrupt_output(value: Value) -> Value {
    json!({ INTERRUPT_KEY: { "value": value } })
}

/// Check if a node output contains an interrupt signal.
pub fn is_interrupt(output: &Value) -> bool {
    output.get(INTERRUPT_KEY).is_some()
}

/// Extract the interrupt value from a node output.
pub fn extract_interrupt_value(output: &Value) -> Option<Value> {
    output
        .get(INTERRUPT_KEY)
        .and_then(|v| v.get("value"))
        .cloned()
}

/// Config key constants for checkpoint-related configuration.
pub mod config_keys {
    pub const THREAD_ID: &str = "thread_id";
    pub const CHECKPOINT_ID: &str = "checkpoint_id";
    pub const RESUME_VALUE: &str = "resume_value";
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn interrupt_output_produces_correct_json() {
        let value = json!({"question": "Approve?", "summary": "test"});
        let output = interrupt_output(value.clone());

        assert!(output.get(INTERRUPT_KEY).is_some());
        let inner = output.get(INTERRUPT_KEY).unwrap();
        assert_eq!(inner.get("value").unwrap(), &value);
    }

    #[test]
    fn is_interrupt_on_interrupt_output() {
        let output = interrupt_output(json!("approve?"));
        assert!(is_interrupt(&output));
    }

    #[test]
    fn is_interrupt_on_normal_output() {
        let output = json!({"result": "done"});
        assert!(!is_interrupt(&output));
    }

    #[test]
    fn is_interrupt_on_empty_object() {
        let output = json!({});
        assert!(!is_interrupt(&output));
    }

    #[test]
    fn is_interrupt_on_null() {
        let output = Value::Null;
        assert!(!is_interrupt(&output));
    }

    #[test]
    fn extract_interrupt_value_from_interrupt() {
        let inner = json!({"question": "Approve this?"});
        let output = interrupt_output(inner.clone());

        let extracted = extract_interrupt_value(&output);
        assert_eq!(extracted, Some(inner));
    }

    #[test]
    fn extract_interrupt_value_from_normal_output() {
        let output = json!({"result": "done"});
        assert_eq!(extract_interrupt_value(&output), None);
    }

    #[test]
    fn extract_interrupt_value_with_nested_data() {
        let inner = json!({
            "question": "Approve?",
            "details": {
                "amount": 100,
                "items": ["a", "b", "c"]
            }
        });
        let output = interrupt_output(inner.clone());
        assert_eq!(extract_interrupt_value(&output), Some(inner));
    }

    #[test]
    fn interrupt_output_with_primitive_value() {
        let output = interrupt_output(json!(42));
        assert!(is_interrupt(&output));
        assert_eq!(extract_interrupt_value(&output), Some(json!(42)));
    }

    #[test]
    fn interrupt_output_with_null_value() {
        let output = interrupt_output(Value::Null);
        assert!(is_interrupt(&output));
        assert_eq!(extract_interrupt_value(&output), Some(Value::Null));
    }

    #[test]
    fn config_keys_are_correct() {
        assert_eq!(config_keys::THREAD_ID, "thread_id");
        assert_eq!(config_keys::CHECKPOINT_ID, "checkpoint_id");
        assert_eq!(config_keys::RESUME_VALUE, "resume_value");
    }
}
