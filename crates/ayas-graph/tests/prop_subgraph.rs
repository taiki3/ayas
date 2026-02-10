use std::collections::HashMap;

use proptest::prelude::*;
use serde_json::{json, Value};

/// For any input mapping and any parent state, mapped input contains exactly the mapped keys.
fn apply_input_mapping(state: &Value, in_map: &HashMap<String, String>) -> Value {
    if in_map.is_empty() {
        return state.clone();
    }
    let mut input = serde_json::Map::new();
    if let Value::Object(parent_state) = state {
        for (parent_key, sub_key) in in_map {
            if let Some(val) = parent_state.get(parent_key) {
                input.insert(sub_key.clone(), val.clone());
            }
        }
    }
    Value::Object(input)
}

/// For any output mapping and any sub-graph result, mapped output contains exactly the mapped keys.
fn apply_output_mapping(sub_output: &Value, out_map: &HashMap<String, String>) -> Value {
    if out_map.is_empty() {
        return sub_output.clone();
    }
    let mut output = serde_json::Map::new();
    if let Value::Object(sub_state) = sub_output {
        for (sub_key, parent_key) in out_map {
            if let Some(val) = sub_state.get(sub_key) {
                output.insert(parent_key.clone(), val.clone());
            }
        }
    }
    Value::Object(output)
}

/// Strategy for generating simple key names.
fn key_strategy() -> impl Strategy<Value = String> {
    "[a-z]{1,8}"
}

/// Strategy for generating a simple JSON value.
fn simple_value_strategy() -> impl Strategy<Value = Value> {
    prop_oneof![
        any::<i64>().prop_map(|n| json!(n)),
        "[a-z]{0,10}".prop_map(|s| json!(s)),
        any::<bool>().prop_map(|b| json!(b)),
    ]
}

/// Strategy for generating a JSON object state.
fn state_strategy() -> impl Strategy<Value = Value> {
    prop::collection::hash_map(key_strategy(), simple_value_strategy(), 0..8)
        .prop_map(|map| {
            let obj: serde_json::Map<String, Value> = map.into_iter().collect();
            Value::Object(obj)
        })
}

/// Strategy for generating a mapping (HashMap<String, String>).
fn mapping_strategy() -> impl Strategy<Value = HashMap<String, String>> {
    prop::collection::hash_map(key_strategy(), key_strategy(), 0..5)
}

proptest! {
    #[test]
    fn input_mapping_contains_only_mapped_keys(
        state in state_strategy(),
        in_map in mapping_strategy(),
    ) {
        let result = apply_input_mapping(&state, &in_map);
        if in_map.is_empty() {
            // Empty mapping = passthrough (identity)
            prop_assert_eq!(&result, &state);
        } else if let Value::Object(result_map) = &result {
            // All keys in result must be values from in_map
            let expected_keys: std::collections::HashSet<&String> = in_map.values().collect();
            for key in result_map.keys() {
                prop_assert!(
                    expected_keys.contains(key),
                    "Unexpected key '{}' in mapped input", key
                );
            }
            // Result should only contain keys where parent state had the source key
            if let Value::Object(parent_state) = &state {
                for (parent_key, sub_key) in &in_map {
                    if parent_state.contains_key(parent_key) {
                        prop_assert!(result_map.contains_key(sub_key));
                    }
                }
            }
        }
    }

    #[test]
    fn output_mapping_contains_only_mapped_keys(
        sub_output in state_strategy(),
        out_map in mapping_strategy(),
    ) {
        let result = apply_output_mapping(&sub_output, &out_map);
        if out_map.is_empty() {
            // Empty mapping = passthrough (identity)
            prop_assert_eq!(&result, &sub_output);
        } else if let Value::Object(result_map) = &result {
            // All keys in result must be values from out_map
            let expected_keys: std::collections::HashSet<&String> = out_map.values().collect();
            for key in result_map.keys() {
                prop_assert!(
                    expected_keys.contains(key),
                    "Unexpected key '{}' in mapped output", key
                );
            }
            // Result should only contain keys where sub_output had the source key
            if let Value::Object(sub_state) = &sub_output {
                for (sub_key, parent_key) in &out_map {
                    if sub_state.contains_key(sub_key) {
                        prop_assert!(result_map.contains_key(parent_key));
                    }
                }
            }
        }
    }

    #[test]
    fn empty_mapping_is_identity(
        state in state_strategy(),
    ) {
        let empty_map: HashMap<String, String> = HashMap::new();
        let input_result = apply_input_mapping(&state, &empty_map);
        let output_result = apply_output_mapping(&state, &empty_map);
        prop_assert_eq!(&input_result, &state);
        prop_assert_eq!(&output_result, &state);
    }

    #[test]
    fn mapped_values_are_preserved(
        state in state_strategy(),
        in_map in mapping_strategy(),
    ) {
        let result = apply_input_mapping(&state, &in_map);
        if !in_map.is_empty() {
            if let (Value::Object(parent_state), Value::Object(result_map)) = (&state, &result) {
                for (parent_key, sub_key) in &in_map {
                    if let Some(expected_val) = parent_state.get(parent_key) {
                        prop_assert_eq!(
                            result_map.get(sub_key).unwrap(),
                            expected_val,
                            "Value mismatch for mapping {} -> {}", parent_key, sub_key
                        );
                    }
                }
            }
        }
    }
}
