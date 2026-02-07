use ayas_core::error::{GraphError, Result};
use serde_json::Value;

/// A channel manages a single key in the graph state.
///
/// Channels control how values are accumulated or overwritten during
/// graph execution. Each step's output is fed through the channel's
/// `update` method.
pub trait Channel: Send + Sync {
    /// Update the channel with new values from a single step.
    ///
    /// Returns `Ok(true)` if the value changed, `Ok(false)` if unchanged.
    fn update(&mut self, values: Vec<Value>) -> Result<bool>;

    /// Get the current value of the channel.
    fn get(&self) -> &Value;

    /// Create a checkpoint of the current state.
    fn checkpoint(&self) -> Value;

    /// Restore state from a checkpoint.
    fn restore(&mut self, data: Value);

    /// Reset the channel to its initial state.
    fn reset(&mut self);
}

/// A channel that keeps only the last value written.
///
/// If multiple values are written in a single step, an error is returned
/// to prevent ambiguous state.
pub struct LastValue {
    value: Value,
    default: Value,
}

impl LastValue {
    /// Create a new `LastValue` channel with the given default.
    pub fn new(default: Value) -> Self {
        Self {
            value: default.clone(),
            default,
        }
    }
}

impl Channel for LastValue {
    fn update(&mut self, values: Vec<Value>) -> Result<bool> {
        match values.len() {
            0 => Ok(false),
            1 => {
                let new_val = values.into_iter().next().unwrap();
                if self.value == new_val {
                    Ok(false)
                } else {
                    self.value = new_val;
                    Ok(true)
                }
            }
            n => Err(GraphError::Channel(format!(
                "LastValue channel received {n} values in a single step; expected at most 1"
            ))
            .into()),
        }
    }

    fn get(&self) -> &Value {
        &self.value
    }

    fn checkpoint(&self) -> Value {
        self.value.clone()
    }

    fn restore(&mut self, data: Value) {
        self.value = data;
    }

    fn reset(&mut self) {
        self.value = self.default.clone();
    }
}

/// A channel that appends values to a JSON array.
///
/// Multiple values in a single step are all appended in order.
pub struct AppendChannel {
    items: Vec<Value>,
    /// Cached JSON array so `get()` can return `&Value`.
    cached: Value,
}

impl AppendChannel {
    /// Create a new, empty `AppendChannel`.
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            cached: Value::Array(Vec::new()),
        }
    }

    fn rebuild_cache(&mut self) {
        self.cached = Value::Array(self.items.clone());
    }
}

impl Default for AppendChannel {
    fn default() -> Self {
        Self::new()
    }
}

impl Channel for AppendChannel {
    fn update(&mut self, values: Vec<Value>) -> Result<bool> {
        if values.is_empty() {
            return Ok(false);
        }
        self.items.extend(values);
        self.rebuild_cache();
        Ok(true)
    }

    fn get(&self) -> &Value {
        &self.cached
    }

    fn checkpoint(&self) -> Value {
        self.cached.clone()
    }

    fn restore(&mut self, data: Value) {
        if let Value::Array(arr) = data {
            self.items = arr;
        } else {
            self.items = vec![data];
        }
        self.rebuild_cache();
    }

    fn reset(&mut self) {
        self.items.clear();
        self.rebuild_cache();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // --- LastValue tests ---

    #[test]
    fn last_value_initial_state() {
        let ch = LastValue::new(json!(0));
        assert_eq!(ch.get(), &json!(0));
    }

    #[test]
    fn last_value_update_single() {
        let mut ch = LastValue::new(json!(0));
        let changed = ch.update(vec![json!(42)]).unwrap();
        assert!(changed);
        assert_eq!(ch.get(), &json!(42));
    }

    #[test]
    fn last_value_update_same_value() {
        let mut ch = LastValue::new(json!(5));
        let changed = ch.update(vec![json!(5)]).unwrap();
        assert!(!changed);
    }

    #[test]
    fn last_value_update_empty() {
        let mut ch = LastValue::new(json!("hello"));
        let changed = ch.update(vec![]).unwrap();
        assert!(!changed);
        assert_eq!(ch.get(), &json!("hello"));
    }

    #[test]
    fn last_value_update_multiple_errors() {
        let mut ch = LastValue::new(json!(0));
        let result = ch.update(vec![json!(1), json!(2)]);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("2 values"));
    }

    #[test]
    fn last_value_checkpoint_restore() {
        let mut ch = LastValue::new(json!(0));
        ch.update(vec![json!(99)]).unwrap();
        let cp = ch.checkpoint();
        assert_eq!(cp, json!(99));

        ch.update(vec![json!(200)]).unwrap();
        assert_eq!(ch.get(), &json!(200));

        ch.restore(cp);
        assert_eq!(ch.get(), &json!(99));
    }

    #[test]
    fn last_value_reset() {
        let mut ch = LastValue::new(json!("default"));
        ch.update(vec![json!("changed")]).unwrap();
        assert_eq!(ch.get(), &json!("changed"));
        ch.reset();
        assert_eq!(ch.get(), &json!("default"));
    }

    // --- AppendChannel tests ---

    #[test]
    fn append_initial_state() {
        let ch = AppendChannel::new();
        assert_eq!(ch.get(), &json!([]));
    }

    #[test]
    fn append_single_value() {
        let mut ch = AppendChannel::new();
        let changed = ch.update(vec![json!(1)]).unwrap();
        assert!(changed);
        assert_eq!(ch.get(), &json!([1]));
    }

    #[test]
    fn append_multiple_values() {
        let mut ch = AppendChannel::new();
        ch.update(vec![json!(1), json!(2)]).unwrap();
        ch.update(vec![json!(3)]).unwrap();
        assert_eq!(ch.get(), &json!([1, 2, 3]));
    }

    #[test]
    fn append_empty_update() {
        let mut ch = AppendChannel::new();
        let changed = ch.update(vec![]).unwrap();
        assert!(!changed);
        assert_eq!(ch.get(), &json!([]));
    }

    #[test]
    fn append_checkpoint_restore() {
        let mut ch = AppendChannel::new();
        ch.update(vec![json!("a"), json!("b")]).unwrap();
        let cp = ch.checkpoint();
        assert_eq!(cp, json!(["a", "b"]));

        ch.update(vec![json!("c")]).unwrap();
        assert_eq!(ch.get(), &json!(["a", "b", "c"]));

        ch.restore(cp);
        assert_eq!(ch.get(), &json!(["a", "b"]));
    }

    #[test]
    fn append_reset() {
        let mut ch = AppendChannel::new();
        ch.update(vec![json!(1), json!(2)]).unwrap();
        ch.reset();
        assert_eq!(ch.get(), &json!([]));
    }
}
