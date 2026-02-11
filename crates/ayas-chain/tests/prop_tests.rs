use proptest::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::json;

use ayas_chain::prelude::*;
use ayas_core::config::RunnableConfig;
use ayas_core::error::AyasError;
use ayas_core::message::Message;
use ayas_core::runnable::Runnable;

/// Strategy for generating arbitrary serde_json::Value (nested objects/arrays).
fn arb_json_value() -> impl Strategy<Value = serde_json::Value> {
    let leaf = prop_oneof![
        Just(serde_json::Value::Null),
        any::<bool>().prop_map(serde_json::Value::Bool),
        any::<i64>().prop_map(|n| json!(n)),
        "[a-zA-Z0-9 _]{0,30}".prop_map(|s| json!(s)),
    ];

    leaf.prop_recursive(
        3,  // depth
        64, // max nodes
        4,  // items per collection
        |inner| {
            prop_oneof![
                // JSON array
                prop::collection::vec(inner.clone(), 0..4)
                    .prop_map(serde_json::Value::Array),
                // JSON object
                prop::collection::vec(
                    ("[a-z]{1,8}", inner),
                    0..4,
                )
                .prop_map(|pairs| {
                    let map: serde_json::Map<String, serde_json::Value> =
                        pairs.into_iter().collect();
                    serde_json::Value::Object(map)
                }),
            ]
        },
    )
}

// ---------------------------------------------------------------------------
// 1. JsonOutputParser: any valid JSON round-trips through the parser
// ---------------------------------------------------------------------------
proptest! {
    #[test]
    fn json_parser_roundtrips_valid_json(value in arb_json_value()) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let json_str = serde_json::to_string(&value).unwrap();
        let messages = vec![Message::ai(json_str)];
        let parser = JsonOutputParser;
        let config = RunnableConfig::default();

        let result = rt.block_on(parser.invoke(messages, &config)).unwrap();
        prop_assert_eq!(result, value);
    }
}

// ---------------------------------------------------------------------------
// 2. JsonOutputParser: markdown code block wrapping doesn't change result
// ---------------------------------------------------------------------------
proptest! {
    #[test]
    fn json_parser_code_block_invariant(value in arb_json_value()) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let json_str = serde_json::to_string(&value).unwrap();

        let plain_messages = vec![Message::ai(json_str.clone())];
        let wrapped_messages = vec![Message::ai(format!("```json\n{}\n```", json_str))];
        let bare_wrapped = vec![Message::ai(format!("```\n{}\n```", json_str))];

        let parser = JsonOutputParser;
        let config = RunnableConfig::default();

        let plain_result = rt.block_on(parser.invoke(plain_messages, &config)).unwrap();
        let wrapped_result = rt.block_on(parser.invoke(wrapped_messages, &config)).unwrap();
        let bare_result = rt.block_on(parser.invoke(bare_wrapped, &config)).unwrap();

        prop_assert_eq!(&plain_result, &wrapped_result,
            "```json wrapping changed result");
        prop_assert_eq!(&plain_result, &bare_result,
            "``` wrapping changed result");
    }
}

// ---------------------------------------------------------------------------
// 3. StructuredOutputParser: valid JSON matching target struct always succeeds
// ---------------------------------------------------------------------------
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct TestPerson {
    name: String,
    age: u32,
}

proptest! {
    #[test]
    fn structured_parser_valid_json_succeeds(
        name in "[a-zA-Z]{1,20}",
        age in 0u32..150,
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let person = TestPerson { name: name.clone(), age };
        let json_str = serde_json::to_string(&person).unwrap();
        let messages = vec![Message::ai(json_str)];

        let parser = StructuredOutputParser::<TestPerson>::new();
        let config = RunnableConfig::default();

        let result = rt.block_on(parser.invoke(messages, &config)).unwrap();
        prop_assert_eq!(result.name, name);
        prop_assert_eq!(result.age, age);
    }
}

// ---------------------------------------------------------------------------
// 4. StructuredOutputParser: invalid JSON always returns ChainError::Parse
// ---------------------------------------------------------------------------
proptest! {
    #[test]
    fn structured_parser_invalid_json_returns_parse_error(
        garbage in "[^{}\\[\\]\"]{1,50}",
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let messages = vec![Message::ai(garbage)];

        let parser = StructuredOutputParser::<TestPerson>::new();
        let config = RunnableConfig::default();

        let result = rt.block_on(parser.invoke(messages, &config));
        prop_assert!(result.is_err());
        match result.unwrap_err() {
            AyasError::Chain(ayas_core::error::ChainError::Parse(msg)) => {
                prop_assert!(msg.contains("structured parse error"),
                    "Expected 'structured parse error', got: {}", msg);
            }
            other => prop_assert!(false, "Expected ChainError::Parse, got: {:?}", other),
        }
    }
}

// ---------------------------------------------------------------------------
// 5. RegexOutputParser: if pattern has capture group and text contains match,
//    result is non-empty
// ---------------------------------------------------------------------------
proptest! {
    #[test]
    fn regex_parser_capture_group_nonempty(
        prefix in "[a-zA-Z ]{0,20}",
        captured in "[a-zA-Z0-9]{1,20}",
        suffix in "[a-zA-Z ]{0,20}",
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        // Build text that always matches "KEY:(.+?)(?:\s|$)"
        let text = format!("{}KEY:{} {}", prefix, captured, suffix);
        let messages = vec![Message::ai(text)];

        let parser = RegexOutputParser::new(r"KEY:([a-zA-Z0-9]+)").unwrap();
        let config = RunnableConfig::default();

        let result = rt.block_on(parser.invoke(messages, &config)).unwrap();
        prop_assert!(!result.is_empty(), "Capture group result was empty");
        prop_assert_eq!(result, captured);
    }
}

// ---------------------------------------------------------------------------
// 6. StringOutputParser: Vec<Message> with at least one AI message succeeds
// ---------------------------------------------------------------------------
proptest! {
    #[test]
    fn string_parser_with_ai_message_succeeds(
        user_msgs in prop::collection::vec("[a-zA-Z0-9 ]{0,30}", 0..3),
        ai_content in "[a-zA-Z0-9 ]{1,50}",
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut messages: Vec<Message> = user_msgs
            .into_iter()
            .map(|s| Message::user(s))
            .collect();
        messages.push(Message::ai(ai_content));

        let parser = StringOutputParser;
        let config = RunnableConfig::default();

        let result = rt.block_on(parser.invoke(messages, &config));
        prop_assert!(result.is_ok(), "StringOutputParser should succeed with AI message");
    }
}

// ---------------------------------------------------------------------------
// 7. StringOutputParser: result equals the last AI message content
// ---------------------------------------------------------------------------
proptest! {
    #[test]
    fn string_parser_returns_last_ai_content(
        first_ai in "[a-zA-Z0-9]{1,30}",
        last_ai in "[a-zA-Z0-9]{1,30}",
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let messages = vec![
            Message::user("question"),
            Message::ai(first_ai),
            Message::user("follow-up"),
            Message::ai(last_ai.clone()),
        ];

        let parser = StringOutputParser;
        let config = RunnableConfig::default();

        let result = rt.block_on(parser.invoke(messages, &config)).unwrap();
        prop_assert_eq!(result, last_ai,
            "StringOutputParser should return the last AI message content");
    }
}

// ---------------------------------------------------------------------------
// 8. MessageContentParser: content() equals parsed result for any Message
// ---------------------------------------------------------------------------
proptest! {
    #[test]
    fn message_content_parser_matches_content(
        text in "[a-zA-Z0-9 ]{0,50}",
        variant in 0u8..4,
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let msg = match variant {
            0 => Message::system(text.clone()),
            1 => Message::user(text.clone()),
            2 => Message::ai(text.clone()),
            _ => Message::tool(text.clone(), "tool-call-id"),
        };

        let expected_content = msg.content().to_string();

        let parser = MessageContentParser;
        let config = RunnableConfig::default();

        let result = rt.block_on(parser.invoke(msg, &config)).unwrap();
        prop_assert_eq!(result, expected_content,
            "MessageContentParser result should match Message::content()");
    }
}
