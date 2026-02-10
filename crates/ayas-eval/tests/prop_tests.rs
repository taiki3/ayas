use proptest::prelude::*;
use serde_json::json;

use ayas_eval::prelude::*;

// Strategy for generating Example values
fn arb_example() -> impl Strategy<Value = Example> {
    (
        "[a-z]{1,10}",                              // id
        prop::bool::ANY,                             // has expected
        "[a-zA-Z0-9 ]{0,50}",                       // input text
        "[a-zA-Z0-9 ]{0,50}",                       // expected text
    )
        .prop_map(|(id, has_expected, input, expected)| Example {
            id,
            input: json!(input),
            expected: if has_expected {
                Some(json!(expected))
            } else {
                None
            },
            metadata: Default::default(),
        })
}

proptest! {
    /// ExactMatchEvaluator always returns 0.0 or 1.0 (binary).
    #[test]
    fn exact_match_is_binary(example in arb_example(), actual in "[a-zA-Z0-9 ]{0,50}") {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let eval = ExactMatchEvaluator;
        let actual_val = json!(actual);
        let score = rt.block_on(eval.evaluate(&example, &actual_val)).unwrap();
        prop_assert!(score.value == 0.0 || score.value == 1.0,
            "ExactMatch score was {} (expected 0.0 or 1.0)", score.value);
    }

    /// ContainsEvaluator: if actual equals expected, score must be 1.0.
    #[test]
    fn contains_same_is_one(text in "[a-zA-Z0-9]{1,50}") {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let eval = ContainsEvaluator;
        let example = Example {
            id: "prop".into(),
            input: json!("test"),
            expected: Some(json!(text.clone())),
            metadata: Default::default(),
        };
        let score = rt.block_on(eval.evaluate(&example, &json!(text))).unwrap();
        prop_assert_eq!(score.value, 1.0,
            "Contains should be 1.0 when actual equals expected");
    }

    /// Dataset serde roundtrip preserves all fields.
    #[test]
    fn dataset_roundtrip(
        name in "[a-z]{1,20}",
        desc in "[a-zA-Z ]{0,50}",
        examples in prop::collection::vec(arb_example(), 0..5)
    ) {
        let mut ds = Dataset::new(name.clone()).with_description(desc.clone());
        for ex in &examples {
            ds.add_example(ex.clone());
        }

        let json_str = ds.to_json().unwrap();
        let ds2 = Dataset::from_json(&json_str).unwrap();

        prop_assert_eq!(&ds2.name, &name);
        prop_assert_eq!(&ds2.description, &desc);
        prop_assert_eq!(ds2.len(), examples.len());

        for (orig, deser) in examples.iter().zip(ds2.examples.iter()) {
            prop_assert_eq!(&orig.id, &deser.id);
            prop_assert_eq!(&orig.input, &deser.input);
            prop_assert_eq!(&orig.expected, &deser.expected);
        }
    }

    /// EvalScore value is always in [0, 1] for ExactMatchEvaluator.
    #[test]
    fn eval_score_in_range(example in arb_example(), actual in "[a-zA-Z0-9 ]{0,50}") {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let eval = ExactMatchEvaluator;
        let actual_val = json!(actual);
        let score = rt.block_on(eval.evaluate(&example, &actual_val)).unwrap();
        prop_assert!(score.value >= 0.0 && score.value <= 1.0,
            "Score {} out of [0, 1] range", score.value);
    }
}
