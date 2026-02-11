use async_trait::async_trait;
use ayas_core::config::RunnableConfig;
use ayas_core::error::{AyasError, Result};
use ayas_core::runnable::{
    IdentityRunnable, Runnable, RunnableBranch, RunnableExt, RunnablePassthrough,
};
use proptest::prelude::*;
use serde_json::Value;

// ---------------------------------------------------------------------------
// Helper runnables for property-based tests
// ---------------------------------------------------------------------------

/// Adds a constant `n` to its input.
struct AddN(i32);

#[async_trait]
impl Runnable for AddN {
    type Input = i32;
    type Output = i32;

    async fn invoke(&self, input: i32, _config: &RunnableConfig) -> Result<i32> {
        Ok(input.wrapping_add(self.0))
    }
}

/// Multiplies its input by a constant `n`.
struct MulN(i32);

#[async_trait]
impl Runnable for MulN {
    type Input = i32;
    type Output = i32;

    async fn invoke(&self, input: i32, _config: &RunnableConfig) -> Result<i32> {
        Ok(input.wrapping_mul(self.0))
    }
}

/// Always fails with an error.
struct AlwaysFail;

#[async_trait]
impl Runnable for AlwaysFail {
    type Input = i32;
    type Output = i32;

    async fn invoke(&self, _input: i32, _config: &RunnableConfig) -> Result<i32> {
        Err(AyasError::Other("always fails".into()))
    }
}

/// Returns a constant JSON string value.
struct ConstValue(Value);

#[async_trait]
impl Runnable for ConstValue {
    type Input = Value;
    type Output = Value;

    async fn invoke(&self, _input: Value, _config: &RunnableConfig) -> Result<Value> {
        Ok(self.0.clone())
    }
}

// ---------------------------------------------------------------------------
// Property-based tests
// ---------------------------------------------------------------------------

proptest! {
    // 1. RunnableSequence associativity:
    //    (a.pipe(b)).pipe(c) ≡ a.pipe(b.pipe(c)) for numeric transforms
    #[test]
    fn sequence_associativity(x in any::<i32>(), a in -100i32..100, b in -100i32..100, c in -100i32..100) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let config = RunnableConfig::default();

            // (AddN(a).pipe(AddN(b))).pipe(AddN(c))
            let left = AddN(a).pipe(AddN(b)).pipe(AddN(c));
            let left_result = left.invoke(x, &config).await.unwrap();

            // AddN(a).pipe(AddN(b).pipe(AddN(c)))
            let right = AddN(a).pipe(AddN(b).pipe(AddN(c)));
            let right_result = right.invoke(x, &config).await.unwrap();

            assert_eq!(left_result, right_result,
                "Associativity violated for x={x}, a={a}, b={b}, c={c}");
        });
    }

    // 2. IdentityRunnable is a left identity: id.pipe(f) ≡ f
    #[test]
    fn identity_left(x in any::<i32>(), n in -100i32..100) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let config = RunnableConfig::default();

            let composed = IdentityRunnable::<i32>::new().pipe(AddN(n));
            let composed_result = composed.invoke(x, &config).await.unwrap();

            let direct = AddN(n);
            let direct_result = direct.invoke(x, &config).await.unwrap();

            assert_eq!(composed_result, direct_result,
                "Left identity violated for x={x}, n={n}");
        });
    }

    // 2b. IdentityRunnable is a right identity: f.pipe(id) ≡ f
    #[test]
    fn identity_right(x in any::<i32>(), n in -100i32..100) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let config = RunnableConfig::default();

            let composed = AddN(n).pipe(IdentityRunnable::<i32>::new());
            let composed_result = composed.invoke(x, &config).await.unwrap();

            let direct = AddN(n);
            let direct_result = direct.invoke(x, &config).await.unwrap();

            assert_eq!(composed_result, direct_result,
                "Right identity violated for x={x}, n={n}");
        });
    }

    // 3. RunnableBranch routes to exactly one branch (exhaustiveness)
    //    We verify that the result always matches exactly one of the expected branch outputs.
    #[test]
    fn branch_routes_exactly_one(x in any::<i32>()) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let config = RunnableConfig::default();

            // Branch: positive → x+1, negative → x*2, default (zero) → x
            let branch = RunnableBranch::new(
                vec![
                    (
                        Box::new(|v: &i32| *v > 0) as Box<dyn Fn(&i32) -> bool + Send + Sync>,
                        Box::new(AddN(1)) as Box<dyn Runnable<Input = i32, Output = i32>>,
                    ),
                    (
                        Box::new(|v: &i32| *v < 0) as Box<dyn Fn(&i32) -> bool + Send + Sync>,
                        Box::new(MulN(2)) as Box<dyn Runnable<Input = i32, Output = i32>>,
                    ),
                ],
                Box::new(IdentityRunnable::<i32>::new()),
            );

            let result = branch.invoke(x, &config).await.unwrap();

            if x > 0 {
                assert_eq!(result, x.wrapping_add(1), "Positive branch mismatch");
            } else if x < 0 {
                assert_eq!(result, x.wrapping_mul(2), "Negative branch mismatch");
            } else {
                assert_eq!(result, x, "Default branch mismatch");
            }
        });
    }

    // 4. RunnableBranch default is used when no condition matches
    #[test]
    fn branch_default_when_no_match(x in any::<i32>()) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let config = RunnableConfig::default();

            // All conditions are impossible (always false)
            let branch = RunnableBranch::new(
                vec![
                    (
                        Box::new(|_: &i32| false) as Box<dyn Fn(&i32) -> bool + Send + Sync>,
                        Box::new(AddN(999)) as Box<dyn Runnable<Input = i32, Output = i32>>,
                    ),
                    (
                        Box::new(|_: &i32| false) as Box<dyn Fn(&i32) -> bool + Send + Sync>,
                        Box::new(MulN(999)) as Box<dyn Runnable<Input = i32, Output = i32>>,
                    ),
                ],
                Box::new(AddN(1)),
            );

            let result = branch.invoke(x, &config).await.unwrap();
            assert_eq!(result, x.wrapping_add(1),
                "Default branch was not used for x={x}");
        });
    }

    // 5. RunnableWithFallback: if primary succeeds, result equals primary result
    #[test]
    fn fallback_returns_primary_on_success(x in any::<i32>(), n in -100i32..100) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let config = RunnableConfig::default();

            let with_fallback = AddN(n).with_fallback(MulN(999));
            let result = with_fallback.invoke(x, &config).await.unwrap();

            let primary_result = AddN(n).invoke(x, &config).await.unwrap();
            assert_eq!(result, primary_result,
                "Fallback should not be used when primary succeeds");
        });
    }

    // 6. RunnableWithFallback: if primary fails, result equals fallback result
    #[test]
    fn fallback_returns_fallback_on_failure(x in any::<i32>(), n in -100i32..100) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let config = RunnableConfig::default();

            let with_fallback = AlwaysFail.with_fallback(AddN(n));
            let result = with_fallback.invoke(x, &config).await.unwrap();

            let fallback_result = AddN(n).invoke(x, &config).await.unwrap();
            assert_eq!(result, fallback_result,
                "Fallback result should equal standalone fallback invocation");
        });
    }

    // 7. RunnablePassthrough preserves original keys when assigning new ones
    #[test]
    fn passthrough_preserves_original_keys(
        key1 in "[a-z]{1,8}",
        val1 in any::<i64>(),
        key2 in "[a-z]{1,8}",
        val2 in any::<i64>(),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let config = RunnableConfig::default();

            // Build an input object with two keys
            let input = serde_json::json!({ key1.clone(): val1, key2.clone(): val2 });

            // Assign a new computed key "extra"
            let passthrough = RunnablePassthrough::new()
                .assign("extra", Box::new(ConstValue(serde_json::json!("added"))));

            let result = passthrough.invoke(input.clone(), &config).await.unwrap();
            let result_obj = result.as_object().unwrap();

            // Original keys should be preserved
            assert_eq!(result_obj.get(&key1), input.get(&key1),
                "Original key '{key1}' not preserved");
            assert_eq!(result_obj.get(&key2), input.get(&key2),
                "Original key '{key2}' not preserved");
        });
    }

    // 8. RunnablePassthrough::with_assign output always contains the assigned key
    #[test]
    fn passthrough_always_contains_assigned_key(
        assign_key in "[a-z]{1,8}",
        val in any::<i64>(),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let config = RunnableConfig::default();

            let input = serde_json::json!({ "x": val });

            let passthrough = RunnablePassthrough::new()
                .assign(assign_key.clone(), Box::new(ConstValue(serde_json::json!(42))));

            let result = passthrough.invoke(input, &config).await.unwrap();
            let result_obj = result.as_object().unwrap();

            assert!(result_obj.contains_key(&assign_key),
                "Assigned key '{assign_key}' missing from output");
            assert_eq!(result_obj[&assign_key], serde_json::json!(42));
        });
    }

    // 9. Batch results length equals input length
    #[test]
    fn batch_length_equals_input_length(inputs in prop::collection::vec(any::<i32>(), 0..50)) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let config = RunnableConfig::default();

            let expected_len = inputs.len();
            let results = AddN(1).batch(inputs, &config).await.unwrap();

            assert_eq!(results.len(), expected_len,
                "Batch output length should match input length");
        });
    }

    // 10. Pipe error propagation: error in any stage propagates to output
    #[test]
    fn pipe_error_propagation(x in any::<i32>(), n in -100i32..100) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let config = RunnableConfig::default();

            // Error in the middle of a 3-stage pipe
            let chain = AddN(n).pipe(AlwaysFail).pipe(AddN(1));
            let result = chain.invoke(x, &config).await;
            assert!(result.is_err(), "Error should propagate from middle stage");

            // Error at the beginning
            let chain_start = AlwaysFail.pipe(AddN(n));
            let result_start = chain_start.invoke(x, &config).await;
            assert!(result_start.is_err(), "Error should propagate from first stage");
        });
    }

    // 11. Batch preserves element order and correctness
    #[test]
    fn batch_preserves_order_and_correctness(
        inputs in prop::collection::vec(any::<i32>(), 1..30),
        n in -100i32..100,
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let config = RunnableConfig::default();

            let results = AddN(n).batch(inputs.clone(), &config).await.unwrap();

            for (i, (input, result)) in inputs.iter().zip(results.iter()).enumerate() {
                assert_eq!(*result, input.wrapping_add(n),
                    "Batch result mismatch at index {i}");
            }
        });
    }

    // 12. Fallback with both failing returns error
    #[test]
    fn fallback_both_fail_returns_error(x in any::<i32>()) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let config = RunnableConfig::default();

            let with_fallback = AlwaysFail.with_fallback(AlwaysFail);
            let result = with_fallback.invoke(x, &config).await;

            assert!(result.is_err(), "Both failing should propagate error");
        });
    }
}
