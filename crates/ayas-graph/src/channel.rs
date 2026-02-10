use std::sync::Arc;

use ayas_core::error::{GraphError, Result};
use serde_json::Value;

/// Specification for creating a channel. Used by `CompiledStateGraph` to
/// create fresh channel instances for each invocation.
/// Built-in aggregation operators for `BinaryOperatorAggregate`.
#[derive(Clone)]
pub enum AggregateOp {
    /// Sum numeric values (f64). Non-numeric values are ignored.
    Sum,
    /// Keep the maximum numeric value.
    Max,
    /// Keep the minimum numeric value.
    Min,
    /// Custom: use a function. Cannot be serialized, so checkpoint saves the current value.
    Custom(Arc<dyn Fn(&Value, &Value) -> Value + Send + Sync>),
}

impl std::fmt::Debug for AggregateOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AggregateOp::Sum => write!(f, "Sum"),
            AggregateOp::Max => write!(f, "Max"),
            AggregateOp::Min => write!(f, "Min"),
            AggregateOp::Custom(_) => write!(f, "Custom(...)"),
        }
    }
}

/// Specification for creating a channel. Used by `CompiledStateGraph` to
/// create fresh channel instances for each invocation.
#[derive(Clone, Debug)]
pub enum ChannelSpec {
    /// A `LastValue` channel with the given default.
    LastValue { default: Value },
    /// An `AppendChannel`.
    Append,
    /// A `BinaryOperatorAggregate` channel with a default value and operator.
    BinaryOperator { default: Value, op: AggregateOp },
    /// An `EphemeralValue` channel that auto-clears after each super-step.
    Ephemeral,
    /// A `TopicChannel` (message-queue style).
    Topic { accumulate: bool },
}

impl ChannelSpec {
    /// Create a fresh `Channel` instance from this spec.
    pub fn create(&self) -> Box<dyn Channel> {
        match self {
            ChannelSpec::LastValue { default } => Box::new(LastValue::new(default.clone())),
            ChannelSpec::Append => Box::new(AppendChannel::new()),
            ChannelSpec::BinaryOperator { default, op } => {
                Box::new(BinaryOperatorAggregate::new(default.clone(), op.clone()))
            }
            ChannelSpec::Ephemeral => Box::new(EphemeralValue::new()),
            ChannelSpec::Topic { accumulate } => Box::new(TopicChannel::new(*accumulate)),
        }
    }
}

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

    /// Called at the end of each Pregel super-step.
    ///
    /// Default: no-op. Override in channels like `EphemeralValue` (auto-clear)
    /// or `Topic` with `accumulate=false`.
    fn on_step_end(&mut self) {}
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
        for value in values {
            // Flatten array values: [a, b] adds a and b individually
            if let Value::Array(arr) = value {
                self.items.extend(arr);
            } else {
                self.items.push(value);
            }
        }
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

// ---------------------------------------------------------------------------
// BinaryOperatorAggregate
// ---------------------------------------------------------------------------

/// A channel that aggregates values using a binary operator.
///
/// Each incoming value is combined with the current accumulated value using the
/// specified `AggregateOp`. For example, `AggregateOp::Sum` adds f64 values.
pub struct BinaryOperatorAggregate {
    value: Value,
    default: Value,
    op: AggregateOp,
}

impl BinaryOperatorAggregate {
    /// Create a new `BinaryOperatorAggregate` with the given default and operator.
    pub fn new(default: Value, op: AggregateOp) -> Self {
        Self {
            value: default.clone(),
            default,
            op,
        }
    }

    fn apply(&self, current: &Value, new_val: &Value) -> Value {
        match &self.op {
            AggregateOp::Sum => {
                let a = current.as_f64().unwrap_or(0.0);
                let b = new_val.as_f64().unwrap_or(0.0);
                serde_json::json!(a + b)
            }
            AggregateOp::Max => {
                let a = current.as_f64().unwrap_or(f64::NEG_INFINITY);
                let b = new_val.as_f64().unwrap_or(f64::NEG_INFINITY);
                serde_json::json!(a.max(b))
            }
            AggregateOp::Min => {
                let a = current.as_f64().unwrap_or(f64::INFINITY);
                let b = new_val.as_f64().unwrap_or(f64::INFINITY);
                serde_json::json!(a.min(b))
            }
            AggregateOp::Custom(f) => f(current, new_val),
        }
    }
}

impl Channel for BinaryOperatorAggregate {
    fn update(&mut self, values: Vec<Value>) -> Result<bool> {
        if values.is_empty() {
            return Ok(false);
        }
        let old = self.value.clone();
        for v in values {
            self.value = self.apply(&self.value, &v);
        }
        Ok(self.value != old)
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

// ---------------------------------------------------------------------------
// EphemeralValue
// ---------------------------------------------------------------------------

/// A channel that auto-clears after each Pregel super-step.
///
/// Useful for transient data like tool calls or intermediate results that
/// should not persist beyond the step that produced them.
pub struct EphemeralValue {
    value: Value,
}

impl EphemeralValue {
    /// Create a new `EphemeralValue` (starts as `Null`).
    pub fn new() -> Self {
        Self { value: Value::Null }
    }
}

impl Default for EphemeralValue {
    fn default() -> Self {
        Self::new()
    }
}

impl Channel for EphemeralValue {
    fn update(&mut self, values: Vec<Value>) -> Result<bool> {
        match values.len() {
            0 => Ok(false),
            _ => {
                // Take the last value (allows overwrite within a step)
                let new_val = values.into_iter().last().unwrap();
                if self.value == new_val {
                    Ok(false)
                } else {
                    self.value = new_val;
                    Ok(true)
                }
            }
        }
    }

    fn get(&self) -> &Value {
        &self.value
    }

    fn checkpoint(&self) -> Value {
        // Ephemeral values are not persisted
        Value::Null
    }

    fn restore(&mut self, _data: Value) {
        self.value = Value::Null;
    }

    fn reset(&mut self) {
        self.value = Value::Null;
    }

    fn on_step_end(&mut self) {
        self.value = Value::Null;
    }
}

// ---------------------------------------------------------------------------
// TopicChannel
// ---------------------------------------------------------------------------

/// A message-queue style channel with optional accumulation.
///
/// When `accumulate` is `false`, values are consumed (cleared) after each
/// super-step via `on_step_end()`. When `true`, values persist across steps.
pub struct TopicChannel {
    values: Vec<Value>,
    cached: Value,
    accumulate: bool,
}

impl TopicChannel {
    /// Create a new `TopicChannel`.
    pub fn new(accumulate: bool) -> Self {
        Self {
            values: Vec::new(),
            cached: Value::Array(Vec::new()),
            accumulate,
        }
    }

    fn rebuild_cache(&mut self) {
        self.cached = Value::Array(self.values.clone());
    }
}

impl Channel for TopicChannel {
    fn update(&mut self, values: Vec<Value>) -> Result<bool> {
        if values.is_empty() {
            return Ok(false);
        }
        self.values.extend(values);
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
            self.values = arr;
        } else {
            self.values = vec![data];
        }
        self.rebuild_cache();
    }

    fn reset(&mut self) {
        self.values.clear();
        self.rebuild_cache();
    }

    fn on_step_end(&mut self) {
        if !self.accumulate {
            self.values.clear();
            self.rebuild_cache();
        }
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

    // --- BinaryOperatorAggregate tests ---

    #[test]
    fn binop_sum_initial_state() {
        let ch = BinaryOperatorAggregate::new(json!(0.0), AggregateOp::Sum);
        assert_eq!(ch.get(), &json!(0.0));
    }

    #[test]
    fn binop_sum_basic() {
        let mut ch = BinaryOperatorAggregate::new(json!(0.0), AggregateOp::Sum);
        let changed = ch.update(vec![json!(3.0)]).unwrap();
        assert!(changed);
        assert_eq!(ch.get(), &json!(3.0));

        ch.update(vec![json!(7.0)]).unwrap();
        assert_eq!(ch.get(), &json!(10.0));
    }

    #[test]
    fn binop_sum_multiple_in_one_step() {
        let mut ch = BinaryOperatorAggregate::new(json!(0.0), AggregateOp::Sum);
        ch.update(vec![json!(1.0), json!(2.0), json!(3.0)]).unwrap();
        assert_eq!(ch.get(), &json!(6.0));
    }

    #[test]
    fn binop_sum_empty_update() {
        let mut ch = BinaryOperatorAggregate::new(json!(5.0), AggregateOp::Sum);
        let changed = ch.update(vec![]).unwrap();
        assert!(!changed);
        assert_eq!(ch.get(), &json!(5.0));
    }

    #[test]
    fn binop_max_basic() {
        let mut ch = BinaryOperatorAggregate::new(json!(0.0), AggregateOp::Max);
        ch.update(vec![json!(5.0)]).unwrap();
        assert_eq!(ch.get(), &json!(5.0));
        ch.update(vec![json!(3.0)]).unwrap();
        assert_eq!(ch.get(), &json!(5.0));
        ch.update(vec![json!(10.0)]).unwrap();
        assert_eq!(ch.get(), &json!(10.0));
    }

    #[test]
    fn binop_min_basic() {
        let mut ch = BinaryOperatorAggregate::new(json!(100.0), AggregateOp::Min);
        ch.update(vec![json!(50.0)]).unwrap();
        assert_eq!(ch.get(), &json!(50.0));
        ch.update(vec![json!(75.0)]).unwrap();
        assert_eq!(ch.get(), &json!(50.0));
        ch.update(vec![json!(10.0)]).unwrap();
        assert_eq!(ch.get(), &json!(10.0));
    }

    #[test]
    fn binop_custom_concat() {
        let concat_op = AggregateOp::Custom(Arc::new(|a: &Value, b: &Value| {
            let sa = a.as_str().unwrap_or("");
            let sb = b.as_str().unwrap_or("");
            json!(format!("{sa}{sb}"))
        }));
        let mut ch = BinaryOperatorAggregate::new(json!(""), concat_op);
        ch.update(vec![json!("hello")]).unwrap();
        assert_eq!(ch.get(), &json!("hello"));
        ch.update(vec![json!(" world")]).unwrap();
        assert_eq!(ch.get(), &json!("hello world"));
    }

    #[test]
    fn binop_checkpoint_restore() {
        let mut ch = BinaryOperatorAggregate::new(json!(0.0), AggregateOp::Sum);
        ch.update(vec![json!(10.0)]).unwrap();
        let cp = ch.checkpoint();
        assert_eq!(cp, json!(10.0));

        ch.update(vec![json!(5.0)]).unwrap();
        assert_eq!(ch.get(), &json!(15.0));

        ch.restore(cp);
        assert_eq!(ch.get(), &json!(10.0));
    }

    #[test]
    fn binop_reset() {
        let mut ch = BinaryOperatorAggregate::new(json!(0.0), AggregateOp::Sum);
        ch.update(vec![json!(42.0)]).unwrap();
        ch.reset();
        assert_eq!(ch.get(), &json!(0.0));
    }

    #[test]
    fn binop_on_step_end_is_noop() {
        let mut ch = BinaryOperatorAggregate::new(json!(0.0), AggregateOp::Sum);
        ch.update(vec![json!(5.0)]).unwrap();
        ch.on_step_end();
        assert_eq!(ch.get(), &json!(5.0));
    }

    // --- EphemeralValue tests ---

    #[test]
    fn ephemeral_initial_state() {
        let ch = EphemeralValue::new();
        assert_eq!(ch.get(), &Value::Null);
    }

    #[test]
    fn ephemeral_update_single() {
        let mut ch = EphemeralValue::new();
        let changed = ch.update(vec![json!("data")]).unwrap();
        assert!(changed);
        assert_eq!(ch.get(), &json!("data"));
    }

    #[test]
    fn ephemeral_update_multiple_takes_last() {
        let mut ch = EphemeralValue::new();
        ch.update(vec![json!(1), json!(2), json!(3)]).unwrap();
        assert_eq!(ch.get(), &json!(3));
    }

    #[test]
    fn ephemeral_update_empty() {
        let mut ch = EphemeralValue::new();
        let changed = ch.update(vec![]).unwrap();
        assert!(!changed);
        assert_eq!(ch.get(), &Value::Null);
    }

    #[test]
    fn ephemeral_on_step_end_clears() {
        let mut ch = EphemeralValue::new();
        ch.update(vec![json!("important")]).unwrap();
        assert_eq!(ch.get(), &json!("important"));
        ch.on_step_end();
        assert_eq!(ch.get(), &Value::Null);
    }

    #[test]
    fn ephemeral_checkpoint_always_null() {
        let mut ch = EphemeralValue::new();
        ch.update(vec![json!(42)]).unwrap();
        assert_eq!(ch.checkpoint(), Value::Null);
    }

    #[test]
    fn ephemeral_restore_sets_null() {
        let mut ch = EphemeralValue::new();
        ch.update(vec![json!("data")]).unwrap();
        ch.restore(json!("anything"));
        assert_eq!(ch.get(), &Value::Null);
    }

    #[test]
    fn ephemeral_reset() {
        let mut ch = EphemeralValue::new();
        ch.update(vec![json!("data")]).unwrap();
        ch.reset();
        assert_eq!(ch.get(), &Value::Null);
    }

    #[test]
    fn ephemeral_update_same_value_not_changed() {
        let mut ch = EphemeralValue::new();
        let changed = ch.update(vec![Value::Null]).unwrap();
        assert!(!changed);
    }

    // --- TopicChannel tests ---

    #[test]
    fn topic_initial_state() {
        let ch = TopicChannel::new(false);
        assert_eq!(ch.get(), &json!([]));
    }

    #[test]
    fn topic_update_single() {
        let mut ch = TopicChannel::new(false);
        let changed = ch.update(vec![json!("msg1")]).unwrap();
        assert!(changed);
        assert_eq!(ch.get(), &json!(["msg1"]));
    }

    #[test]
    fn topic_update_multiple() {
        let mut ch = TopicChannel::new(false);
        ch.update(vec![json!("a"), json!("b")]).unwrap();
        ch.update(vec![json!("c")]).unwrap();
        assert_eq!(ch.get(), &json!(["a", "b", "c"]));
    }

    #[test]
    fn topic_update_empty() {
        let mut ch = TopicChannel::new(false);
        let changed = ch.update(vec![]).unwrap();
        assert!(!changed);
        assert_eq!(ch.get(), &json!([]));
    }

    #[test]
    fn topic_on_step_end_no_accumulate_clears() {
        let mut ch = TopicChannel::new(false);
        ch.update(vec![json!("msg")]).unwrap();
        ch.on_step_end();
        assert_eq!(ch.get(), &json!([]));
    }

    #[test]
    fn topic_on_step_end_accumulate_keeps() {
        let mut ch = TopicChannel::new(true);
        ch.update(vec![json!("msg")]).unwrap();
        ch.on_step_end();
        assert_eq!(ch.get(), &json!(["msg"]));
    }

    #[test]
    fn topic_checkpoint_restore() {
        let mut ch = TopicChannel::new(true);
        ch.update(vec![json!("a"), json!("b")]).unwrap();
        let cp = ch.checkpoint();
        assert_eq!(cp, json!(["a", "b"]));

        ch.update(vec![json!("c")]).unwrap();
        assert_eq!(ch.get(), &json!(["a", "b", "c"]));

        ch.restore(cp);
        assert_eq!(ch.get(), &json!(["a", "b"]));
    }

    #[test]
    fn topic_reset() {
        let mut ch = TopicChannel::new(true);
        ch.update(vec![json!("a"), json!("b")]).unwrap();
        ch.reset();
        assert_eq!(ch.get(), &json!([]));
    }

    #[test]
    fn topic_no_accumulate_multi_step() {
        let mut ch = TopicChannel::new(false);
        ch.update(vec![json!("step1")]).unwrap();
        assert_eq!(ch.get(), &json!(["step1"]));
        ch.on_step_end();
        assert_eq!(ch.get(), &json!([]));

        ch.update(vec![json!("step2")]).unwrap();
        assert_eq!(ch.get(), &json!(["step2"]));
        ch.on_step_end();
        assert_eq!(ch.get(), &json!([]));
    }
}
