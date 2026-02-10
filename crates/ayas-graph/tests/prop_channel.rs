//! Property-based tests for Channel checkpoint/restore invariants.
//!
//! The correctness of the entire checkpoint system depends on:
//! channel.restore(channel.checkpoint()) preserving the observable state.

use proptest::prelude::*;
use serde_json::Value;

use ayas_graph::prelude::*;

// ---------------------------------------------------------------------------
// Strategies
// ---------------------------------------------------------------------------

fn arb_json_value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        (-1_000_000i64..1_000_000).prop_map(|n| Value::Number(n.into())),
        "[a-zA-Z0-9_ ]{0,20}".prop_map(Value::String),
    ];
    leaf.prop_recursive(2, 16, 4, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
            prop::collection::hash_map("[a-z_]{1,6}", inner, 0..3)
                .prop_map(|m| Value::Object(m.into_iter().collect())),
        ]
    })
}

// ===========================================================================
// LastValue channel properties
// ===========================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    /// checkpoint() then restore() preserves the observable state of LastValue.
    #[test]
    fn last_value_checkpoint_restore_roundtrip(
        default in arb_json_value(),
        updates in prop::collection::vec(arb_json_value(), 0..5),
    ) {
        let mut ch = LastValue::new(default);

        // Apply a series of single-value updates
        for val in &updates {
            let _ = ch.update(vec![val.clone()]);
        }

        let snapshot = ch.checkpoint();
        let state_before = ch.get().clone();

        // Mutate further
        let _ = ch.update(vec![Value::String("MUTATED".into())]);

        // Restore
        ch.restore(snapshot);
        prop_assert_eq!(ch.get(), &state_before);
    }

    /// checkpoint() returns the current get() value for LastValue.
    #[test]
    fn last_value_checkpoint_equals_get(
        default in arb_json_value(),
        val in arb_json_value(),
    ) {
        let mut ch = LastValue::new(default);
        let _ = ch.update(vec![val]);
        prop_assert_eq!(&ch.checkpoint(), ch.get());
    }

    /// After reset(), the channel returns to the default value.
    #[test]
    fn last_value_reset_returns_to_default(
        default in arb_json_value(),
        updates in prop::collection::vec(arb_json_value(), 1..5),
    ) {
        let mut ch = LastValue::new(default.clone());
        for val in &updates {
            let _ = ch.update(vec![val.clone()]);
        }
        ch.reset();
        prop_assert_eq!(ch.get(), &default);
    }

    /// Multiple checkpoint/restore cycles are idempotent.
    #[test]
    fn last_value_double_checkpoint_restore(val in arb_json_value()) {
        let mut ch = LastValue::new(Value::Null);
        let _ = ch.update(vec![val]);

        let snap1 = ch.checkpoint();
        ch.restore(snap1.clone());
        let snap2 = ch.checkpoint();

        prop_assert_eq!(snap1, snap2);
    }
}

// ===========================================================================
// AppendChannel properties
// ===========================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    /// checkpoint() then restore() preserves the observable state of AppendChannel.
    #[test]
    fn append_checkpoint_restore_roundtrip(
        batches in prop::collection::vec(
            prop::collection::vec(arb_json_value(), 0..4),
            0..4,
        ),
    ) {
        let mut ch = AppendChannel::new();

        for batch in &batches {
            // Avoid passing arrays directly as they get flattened;
            // wrap each value to ensure consistent behavior.
            let non_array_batch: Vec<Value> = batch
                .iter()
                .map(|v| {
                    // Wrap arrays in an object to prevent flattening
                    if v.is_array() {
                        Value::Object(
                            [("wrapped".to_string(), v.clone())]
                                .into_iter()
                                .collect(),
                        )
                    } else {
                        v.clone()
                    }
                })
                .collect();
            let _ = ch.update(non_array_batch);
        }

        let snapshot = ch.checkpoint();
        let state_before = ch.get().clone();

        // Mutate
        let _ = ch.update(vec![Value::String("EXTRA".into())]);

        // Restore
        ch.restore(snapshot);
        prop_assert_eq!(ch.get(), &state_before);
    }

    /// AppendChannel checkpoint is always a JSON array.
    #[test]
    fn append_checkpoint_is_always_array(
        vals in prop::collection::vec(arb_json_value(), 0..6),
    ) {
        let mut ch = AppendChannel::new();
        // Only push non-array values to avoid flattening complexity
        let non_arrays: Vec<Value> = vals.into_iter().filter(|v| !v.is_array()).collect();
        if !non_arrays.is_empty() {
            let _ = ch.update(non_arrays);
        }
        prop_assert!(ch.checkpoint().is_array());
    }

    /// After reset, AppendChannel is an empty array.
    #[test]
    fn append_reset_is_empty(
        vals in prop::collection::vec(arb_json_value(), 1..5),
    ) {
        let mut ch = AppendChannel::new();
        let non_arrays: Vec<Value> = vals.into_iter().filter(|v| !v.is_array()).collect();
        if !non_arrays.is_empty() {
            let _ = ch.update(non_arrays);
        }
        ch.reset();
        prop_assert_eq!(ch.get(), &Value::Array(vec![]));
    }

    /// Multiple checkpoint/restore cycles on AppendChannel are idempotent.
    #[test]
    fn append_double_checkpoint_restore(
        vals in prop::collection::vec(
            // Only non-array leaf values to avoid flatten behavior
            prop_oneof![
                Just(Value::Null),
                any::<bool>().prop_map(Value::Bool),
                (-1000i64..1000).prop_map(|n| Value::Number(n.into())),
                "[a-z]{0,10}".prop_map(Value::String),
            ],
            0..5,
        ),
    ) {
        let mut ch = AppendChannel::new();
        if !vals.is_empty() {
            let _ = ch.update(vals);
        }

        let snap1 = ch.checkpoint();
        ch.restore(snap1.clone());
        let snap2 = ch.checkpoint();

        prop_assert_eq!(snap1, snap2);
    }
}

// ===========================================================================
// BinaryOperatorAggregate properties
// ===========================================================================

/// Strategy for f64-representable JSON numbers.
fn arb_f64_value() -> impl Strategy<Value = Value> {
    (-1_000_000f64..1_000_000f64).prop_map(|n| serde_json::json!(n))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    /// checkpoint/restore roundtrip for BinaryOperatorAggregate (Sum).
    #[test]
    fn binop_sum_checkpoint_restore_roundtrip(
        default in arb_f64_value(),
        updates in prop::collection::vec(arb_f64_value(), 0..5),
    ) {
        let mut ch = BinaryOperatorAggregate::new(default, AggregateOp::Sum);

        for val in &updates {
            let _ = ch.update(vec![val.clone()]);
        }

        let snapshot = ch.checkpoint();
        let state_before = ch.get().clone();

        // Mutate further
        let _ = ch.update(vec![serde_json::json!(999.0)]);

        // Restore
        ch.restore(snapshot);
        prop_assert_eq!(ch.get(), &state_before);
    }

    /// Double checkpoint/restore is idempotent for BinaryOperatorAggregate.
    #[test]
    fn binop_sum_double_checkpoint_restore(
        val in arb_f64_value(),
    ) {
        let mut ch = BinaryOperatorAggregate::new(serde_json::json!(0.0), AggregateOp::Sum);
        let _ = ch.update(vec![val]);

        let snap1 = ch.checkpoint();
        ch.restore(snap1.clone());
        let snap2 = ch.checkpoint();

        prop_assert_eq!(snap1, snap2);
    }

    /// on_step_end is a no-op for BinaryOperatorAggregate.
    #[test]
    fn binop_on_step_end_noop(
        updates in prop::collection::vec(arb_f64_value(), 1..5),
    ) {
        let mut ch = BinaryOperatorAggregate::new(serde_json::json!(0.0), AggregateOp::Sum);
        for val in &updates {
            let _ = ch.update(vec![val.clone()]);
        }
        let before = ch.get().clone();
        ch.on_step_end();
        prop_assert_eq!(ch.get(), &before);
    }

    /// reset() returns BinaryOperatorAggregate to the default.
    #[test]
    fn binop_reset_returns_to_default(
        default in arb_f64_value(),
        updates in prop::collection::vec(arb_f64_value(), 1..5),
    ) {
        let mut ch = BinaryOperatorAggregate::new(default.clone(), AggregateOp::Sum);
        for val in &updates {
            let _ = ch.update(vec![val.clone()]);
        }
        ch.reset();
        prop_assert_eq!(ch.get(), &default);
    }
}

// ===========================================================================
// EphemeralValue properties
// ===========================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    /// checkpoint/restore roundtrip for EphemeralValue always yields Null.
    #[test]
    fn ephemeral_checkpoint_restore_roundtrip(
        updates in prop::collection::vec(arb_json_value(), 0..5),
    ) {
        let mut ch = EphemeralValue::new();

        for val in &updates {
            let _ = ch.update(vec![val.clone()]);
        }

        let snapshot = ch.checkpoint();
        // Checkpoint is always Null
        prop_assert_eq!(&snapshot, &Value::Null);

        ch.restore(snapshot);
        prop_assert_eq!(ch.get(), &Value::Null);
    }

    /// on_step_end always clears EphemeralValue.
    #[test]
    fn ephemeral_on_step_end_clears(
        val in arb_json_value(),
    ) {
        let mut ch = EphemeralValue::new();
        let _ = ch.update(vec![val]);
        ch.on_step_end();
        prop_assert_eq!(ch.get(), &Value::Null);
    }

    /// Double checkpoint/restore is idempotent for EphemeralValue.
    #[test]
    fn ephemeral_double_checkpoint_restore(val in arb_json_value()) {
        let mut ch = EphemeralValue::new();
        let _ = ch.update(vec![val]);

        let snap1 = ch.checkpoint();
        ch.restore(snap1.clone());
        let snap2 = ch.checkpoint();

        prop_assert_eq!(snap1, snap2);
    }

    /// reset() clears EphemeralValue to Null.
    #[test]
    fn ephemeral_reset_clears(
        updates in prop::collection::vec(arb_json_value(), 1..5),
    ) {
        let mut ch = EphemeralValue::new();
        for val in &updates {
            let _ = ch.update(vec![val.clone()]);
        }
        ch.reset();
        prop_assert_eq!(ch.get(), &Value::Null);
    }
}

// ===========================================================================
// TopicChannel properties
// ===========================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    /// checkpoint/restore roundtrip for TopicChannel (accumulate=true).
    #[test]
    fn topic_accumulate_checkpoint_restore_roundtrip(
        batches in prop::collection::vec(
            prop::collection::vec(arb_json_value(), 0..4),
            0..4,
        ),
    ) {
        let mut ch = TopicChannel::new(true);

        for batch in &batches {
            if !batch.is_empty() {
                let _ = ch.update(batch.clone());
            }
        }

        let snapshot = ch.checkpoint();
        let state_before = ch.get().clone();

        // Mutate
        let _ = ch.update(vec![Value::String("EXTRA".into())]);

        // Restore
        ch.restore(snapshot);
        prop_assert_eq!(ch.get(), &state_before);
    }

    /// on_step_end with accumulate=false clears TopicChannel.
    #[test]
    fn topic_no_accumulate_on_step_end_clears(
        vals in prop::collection::vec(arb_json_value(), 1..5),
    ) {
        let mut ch = TopicChannel::new(false);
        let _ = ch.update(vals);
        ch.on_step_end();
        prop_assert_eq!(ch.get(), &Value::Array(vec![]));
    }

    /// on_step_end with accumulate=true preserves TopicChannel values.
    #[test]
    fn topic_accumulate_on_step_end_preserves(
        vals in prop::collection::vec(arb_json_value(), 1..5),
    ) {
        let mut ch = TopicChannel::new(true);
        let _ = ch.update(vals);
        let before = ch.get().clone();
        ch.on_step_end();
        prop_assert_eq!(ch.get(), &before);
    }

    /// Double checkpoint/restore is idempotent for TopicChannel.
    #[test]
    fn topic_double_checkpoint_restore(
        vals in prop::collection::vec(arb_json_value(), 0..5),
    ) {
        let mut ch = TopicChannel::new(true);
        if !vals.is_empty() {
            let _ = ch.update(vals);
        }

        let snap1 = ch.checkpoint();
        ch.restore(snap1.clone());
        let snap2 = ch.checkpoint();

        prop_assert_eq!(snap1, snap2);
    }

    /// TopicChannel checkpoint is always a JSON array.
    #[test]
    fn topic_checkpoint_is_always_array(
        vals in prop::collection::vec(arb_json_value(), 0..6),
        accumulate in any::<bool>(),
    ) {
        let mut ch = TopicChannel::new(accumulate);
        if !vals.is_empty() {
            let _ = ch.update(vals);
        }
        prop_assert!(ch.checkpoint().is_array());
    }
}

// ===========================================================================
// ChannelSpec factory property
// ===========================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// ChannelSpec::LastValue creates a channel whose initial get() matches the default.
    #[test]
    fn channel_spec_last_value_initial(default in arb_json_value()) {
        let spec = ChannelSpec::LastValue { default: default.clone() };
        let ch = spec.create();
        prop_assert_eq!(ch.get(), &default);
    }
}
