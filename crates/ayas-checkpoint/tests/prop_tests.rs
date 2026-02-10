//! Property-based tests for ayas-checkpoint.
//!
//! Covers:
//! 1. Checkpoint JSON serde roundtrip (arbitrary data)
//! 2. interrupt_output / extract_interrupt_value roundtrip
//! 3. Memory vs Sqlite store equivalence

use std::collections::HashMap;

use chrono::{TimeZone, Utc};
use proptest::prelude::*;
use serde_json::Value;

use ayas_checkpoint::prelude::*;
use ayas_checkpoint::interrupt::{extract_interrupt_value, interrupt_output, is_interrupt};
use ayas_checkpoint::store::CheckpointStore;

// ---------------------------------------------------------------------------
// Strategies
// ---------------------------------------------------------------------------

/// Generate an arbitrary JSON value with bounded depth.
fn arb_json_value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        (-1_000_000i64..1_000_000).prop_map(|n| Value::Number(n.into())),
        "[a-zA-Z0-9_ \\-]{0,30}".prop_map(Value::String),
    ];
    leaf.prop_recursive(3, 24, 4, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
            prop::collection::hash_map("[a-zA-Z_]{1,8}", inner, 0..4)
                .prop_map(|m| Value::Object(m.into_iter().collect())),
        ]
    })
}

/// Generate an arbitrary CheckpointMetadata.
fn arb_metadata() -> impl Strategy<Value = CheckpointMetadata> {
    (
        prop_oneof![Just("input"), Just("loop"), Just("interrupt")],
        0..100usize,
        proptest::option::of("[a-z_]{1,12}".prop_map(String::from)),
    )
        .prop_map(|(source, step, node_name)| CheckpointMetadata {
            source: source.to_string(),
            step,
            node_name,
        })
}

/// Generate an arbitrary Checkpoint.
/// Uses second-precision timestamps to ensure SQLite roundtrip fidelity.
fn arb_checkpoint() -> impl Strategy<Value = Checkpoint> {
    (
        "[a-z0-9\\-]{1,16}",
        "[a-z0-9\\-]{1,16}",
        proptest::option::of("[a-z0-9\\-]{1,16}".prop_map(String::from)),
        0..50usize,
        prop::collection::hash_map("[a-z_]{1,8}", arb_json_value(), 0..4),
        prop::collection::vec("[a-z_]{1,10}".prop_map(String::from), 0..3),
        arb_metadata(),
        // Timestamp as seconds since epoch (second precision for SQLite compat)
        (1_700_000_000i64..1_800_000_000i64),
    )
        .prop_map(
            |(id, thread_id, parent_id, step, channel_values, pending_nodes, metadata, ts)| {
                Checkpoint {
                    id,
                    thread_id,
                    parent_id,
                    step,
                    channel_values,
                    pending_nodes,
                    metadata,
                    created_at: Utc.timestamp_opt(ts, 0).unwrap(),
                }
            },
        )
}

// ---------------------------------------------------------------------------
// Helpers for comparing checkpoints without PartialEq
// ---------------------------------------------------------------------------

fn assert_checkpoints_eq(a: &Checkpoint, b: &Checkpoint) {
    assert_eq!(a.id, b.id, "id mismatch");
    assert_eq!(a.thread_id, b.thread_id, "thread_id mismatch");
    assert_eq!(a.parent_id, b.parent_id, "parent_id mismatch");
    assert_eq!(a.step, b.step, "step mismatch");
    assert_eq!(a.channel_values, b.channel_values, "channel_values mismatch");
    assert_eq!(a.pending_nodes, b.pending_nodes, "pending_nodes mismatch");
    assert_eq!(a.metadata.source, b.metadata.source, "metadata.source mismatch");
    assert_eq!(a.metadata.step, b.metadata.step, "metadata.step mismatch");
    assert_eq!(a.metadata.node_name, b.metadata.node_name, "metadata.node_name mismatch");
}

// ===========================================================================
// 1. Checkpoint JSON serde roundtrip
// ===========================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    #[test]
    fn checkpoint_json_serde_roundtrip(cp in arb_checkpoint()) {
        let json_str = serde_json::to_string(&cp).unwrap();
        let deserialized: Checkpoint = serde_json::from_str(&json_str).unwrap();
        assert_checkpoints_eq(&cp, &deserialized);
    }

    #[test]
    fn checkpoint_metadata_json_roundtrip(meta in arb_metadata()) {
        let json_str = serde_json::to_string(&meta).unwrap();
        let deserialized: CheckpointMetadata = serde_json::from_str(&json_str).unwrap();
        assert_eq!(meta.source, deserialized.source);
        assert_eq!(meta.step, deserialized.step);
        assert_eq!(meta.node_name, deserialized.node_name);
    }
}

// ===========================================================================
// 2. interrupt_output / extract_interrupt_value roundtrip
// ===========================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    /// For any JSON value, interrupt_output -> extract_interrupt_value is lossless.
    #[test]
    fn interrupt_roundtrip(val in arb_json_value()) {
        let output = interrupt_output(val.clone());
        let extracted = extract_interrupt_value(&output);
        prop_assert_eq!(extracted, Some(val));
    }

    /// interrupt_output always produces an output that is_interrupt recognizes.
    #[test]
    fn interrupt_output_always_detected(val in arb_json_value()) {
        let output = interrupt_output(val);
        prop_assert!(is_interrupt(&output));
    }

    /// A JSON object without __interrupt__ key is never detected as interrupt.
    #[test]
    fn non_interrupt_never_detected(
        entries in prop::collection::hash_map(
            "[a-zA-Z]{1,10}".prop_filter("not interrupt key", |s| s != "__interrupt__"),
            arb_json_value(),
            0..5,
        )
    ) {
        let obj = Value::Object(entries.into_iter().collect());
        prop_assert!(!is_interrupt(&obj));
    }
}

// ===========================================================================
// 3. Memory vs Sqlite store equivalence
// ===========================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(50))]

    /// put then get returns the same checkpoint for both stores.
    #[test]
    fn store_equivalence_put_get(cp in arb_checkpoint()) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mem = MemoryCheckpointStore::new();
            let sqlite = SqliteCheckpointStore::in_memory().unwrap();

            mem.put(cp.clone()).await.unwrap();
            sqlite.put(cp.clone()).await.unwrap();

            let m = mem.get(&cp.thread_id, &cp.id).await.unwrap().unwrap();
            let s = sqlite.get(&cp.thread_id, &cp.id).await.unwrap().unwrap();

            assert_checkpoints_eq(&m, &s);
        });
    }

    /// get_latest returns the same checkpoint for both stores after N inserts.
    #[test]
    fn store_equivalence_get_latest(
        steps in prop::collection::vec(arb_json_value(), 1..6),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mem = MemoryCheckpointStore::new();
            let sqlite = SqliteCheckpointStore::in_memory().unwrap();
            let thread_id = "equiv-thread";

            for (i, val) in steps.iter().enumerate() {
                let cp = Checkpoint {
                    id: format!("cp-{i}"),
                    thread_id: thread_id.into(),
                    parent_id: if i > 0 { Some(format!("cp-{}", i - 1)) } else { None },
                    step: i,
                    channel_values: HashMap::from([("v".into(), val.clone())]),
                    pending_nodes: vec![],
                    metadata: CheckpointMetadata {
                        source: "loop".into(),
                        step: i,
                        node_name: None,
                    },
                    created_at: Utc.timestamp_opt(1_700_000_000 + i as i64, 0).unwrap(),
                };
                mem.put(cp.clone()).await.unwrap();
                sqlite.put(cp).await.unwrap();
            }

            let m = mem.get_latest(thread_id).await.unwrap().unwrap();
            let s = sqlite.get_latest(thread_id).await.unwrap().unwrap();
            assert_checkpoints_eq(&m, &s);
        });
    }

    /// list returns the same ordered checkpoints from both stores.
    #[test]
    fn store_equivalence_list_ordering(
        // Generate step numbers that may be out of insertion order
        step_order in prop::collection::vec(0..20usize, 1..8),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mem = MemoryCheckpointStore::new();
            let sqlite = SqliteCheckpointStore::in_memory().unwrap();
            let thread_id = "order-thread";

            // Deduplicate steps (store uses (thread_id, id) as PK)
            let mut seen = std::collections::HashSet::new();
            for &step in &step_order {
                if !seen.insert(step) {
                    continue;
                }
                let cp = Checkpoint {
                    id: format!("cp-{step}"),
                    thread_id: thread_id.into(),
                    parent_id: None,
                    step,
                    channel_values: HashMap::from([("s".into(), Value::Number(step.into()))]),
                    pending_nodes: vec![],
                    metadata: CheckpointMetadata {
                        source: "loop".into(),
                        step,
                        node_name: None,
                    },
                    created_at: Utc.timestamp_opt(1_700_000_000 + step as i64, 0).unwrap(),
                };
                mem.put(cp.clone()).await.unwrap();
                sqlite.put(cp).await.unwrap();
            }

            let m_list = mem.list(thread_id).await.unwrap();
            let s_list = sqlite.list(thread_id).await.unwrap();

            // Same length
            assert_eq!(m_list.len(), s_list.len(), "list length mismatch");

            // Same order and content
            for (m, s) in m_list.iter().zip(s_list.iter()) {
                assert_checkpoints_eq(m, s);
            }

            // Both sorted by step
            for w in m_list.windows(2) {
                assert!(w[0].step <= w[1].step, "Memory list not sorted by step");
            }
            for w in s_list.windows(2) {
                assert!(w[0].step <= w[1].step, "SQLite list not sorted by step");
            }
        });
    }

    /// delete_thread then list returns empty for both stores.
    #[test]
    fn store_equivalence_delete(cp in arb_checkpoint()) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mem = MemoryCheckpointStore::new();
            let sqlite = SqliteCheckpointStore::in_memory().unwrap();

            mem.put(cp.clone()).await.unwrap();
            sqlite.put(cp.clone()).await.unwrap();

            mem.delete_thread(&cp.thread_id).await.unwrap();
            sqlite.delete_thread(&cp.thread_id).await.unwrap();

            let m = mem.list(&cp.thread_id).await.unwrap();
            let s = sqlite.list(&cp.thread_id).await.unwrap();
            assert!(m.is_empty(), "Memory store not empty after delete");
            assert!(s.is_empty(), "SQLite store not empty after delete");
        });
    }
}
