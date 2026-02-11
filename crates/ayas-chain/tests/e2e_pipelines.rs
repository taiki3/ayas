//! E2E tests for structured pipeline compositions.
//!
//! Tests combine parsers, prompts, and runnable composition patterns
//! into realistic multi-stage pipelines.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::Value;

use ayas_chain::lambda::RunnableLambda;
use ayas_chain::mock::MockChatModel;
use ayas_chain::parser::{JsonOutputParser, StringOutputParser, StructuredOutputParser};
use ayas_chain::prompt::PromptTemplate;
use ayas_core::config::RunnableConfig;
use ayas_core::error::AyasError;
use ayas_core::message::Message;
use ayas_core::runnable::{Runnable, RunnableBranch, RunnableExt, RunnablePassthrough};

// ---------------------------------------------------------------------------
// Test 1: Prompt -> Model -> JsonOutputParser pipeline
// ---------------------------------------------------------------------------

/// Template fills variables, mock model returns JSON, parser extracts it.
#[tokio::test]
async fn prompt_to_model_to_json_parser() {
    let prompt = PromptTemplate::from_messages(vec![
        ("system", "You are a data extractor."),
        ("user", "Extract info from: {text}"),
    ]);
    let model =
        MockChatModel::with_response(r#"{"name": "Alice", "age": 30, "city": "Tokyo"}"#);
    let parser = JsonOutputParser;
    let chain = prompt.pipe(model).pipe(parser);

    let mut vars = HashMap::new();
    vars.insert("text".into(), "Alice is 30 and lives in Tokyo".into());

    let config = RunnableConfig::default();
    let result = chain.invoke(vars, &config).await.unwrap();

    assert_eq!(result["name"], "Alice");
    assert_eq!(result["age"], 30);
    assert_eq!(result["city"], "Tokyo");
}

// ---------------------------------------------------------------------------
// Test 2: Prompt -> Model -> StructuredOutputParser<T>
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, PartialEq)]
struct PersonInfo {
    name: String,
    age: u32,
    occupation: String,
}

/// Typed end-to-end: template fills, model returns JSON in a code block,
/// parser deserializes into a concrete Rust struct.
#[tokio::test]
async fn prompt_to_model_to_structured_parser() {
    let prompt = PromptTemplate::from_template("Extract person info from: {description}");
    let model = MockChatModel::with_response(
        "```json\n{\"name\": \"Bob\", \"age\": 42, \"occupation\": \"Engineer\"}\n```",
    );
    let parser = StructuredOutputParser::<PersonInfo>::new();
    let chain = prompt.pipe(model).pipe(parser);

    let mut vars = HashMap::new();
    vars.insert(
        "description".into(),
        "Bob is a 42 year old engineer".into(),
    );

    let config = RunnableConfig::default();
    let result = chain.invoke(vars, &config).await.unwrap();

    assert_eq!(
        result,
        PersonInfo {
            name: "Bob".into(),
            age: 42,
            occupation: "Engineer".into(),
        }
    );
}

// ---------------------------------------------------------------------------
// Test 3: RunnableBranch routing to different chains
// ---------------------------------------------------------------------------

/// Branch selects chain based on an input field: "formal" routes to one model,
/// anything else defaults to another.
#[tokio::test]
async fn branch_routes_to_different_chains() {
    let formal_prompt = PromptTemplate::from_messages(vec![
        ("system", "Respond formally."),
        ("user", "{input}"),
    ]);
    let casual_prompt = PromptTemplate::from_messages(vec![
        ("system", "Respond casually."),
        ("user", "{input}"),
    ]);

    let formal_model =
        MockChatModel::with_response("I appreciate your inquiry, esteemed colleague.");
    let casual_model = MockChatModel::with_response("Hey, what's up!");

    let formal_chain = formal_prompt.pipe(formal_model).pipe(StringOutputParser);
    let casual_chain = casual_prompt.pipe(casual_model).pipe(StringOutputParser);

    let branch = RunnableBranch::new(
        vec![(
            Box::new(|input: &HashMap<String, String>| {
                input.get("tone").map_or(false, |t| t == "formal")
            }) as Box<dyn Fn(&HashMap<String, String>) -> bool + Send + Sync>,
            Box::new(formal_chain)
                as Box<dyn Runnable<Input = HashMap<String, String>, Output = String>>,
        )],
        Box::new(casual_chain),
    );

    let config = RunnableConfig::default();

    // Formal route
    let mut vars = HashMap::new();
    vars.insert("tone".into(), "formal".into());
    vars.insert("input".into(), "Hello".into());
    let result = branch.invoke(vars, &config).await.unwrap();
    assert_eq!(result, "I appreciate your inquiry, esteemed colleague.");

    // Casual route (default)
    let mut vars = HashMap::new();
    vars.insert("tone".into(), "casual".into());
    vars.insert("input".into(), "Hello".into());
    let result = branch.invoke(vars, &config).await.unwrap();
    assert_eq!(result, "Hey, what's up!");
}

// ---------------------------------------------------------------------------
// Test 4: RunnableWithFallback chain
// ---------------------------------------------------------------------------

/// Primary chain fails (model error), fallback chain succeeds.
#[tokio::test]
async fn fallback_chain_recovers_from_failure() {
    let primary_prompt = PromptTemplate::from_template("Translate: {text}");
    let failing_model = RunnableLambda::new(|_input: Vec<Message>, _config| async move {
        Err::<Vec<Message>, _>(AyasError::Other("model unavailable".into()))
    });
    let primary = primary_prompt
        .pipe(failing_model)
        .pipe(StringOutputParser);

    let fallback_prompt = PromptTemplate::from_template("Translate: {text}");
    let fallback_model = MockChatModel::with_response("Translated text successfully");
    let fallback = fallback_prompt
        .pipe(fallback_model)
        .pipe(StringOutputParser);

    let chain = primary.with_fallback(fallback);

    let mut vars = HashMap::new();
    vars.insert("text".into(), "Hello, world!".into());

    let config = RunnableConfig::default();
    let result = chain.invoke(vars, &config).await.unwrap();
    assert_eq!(result, "Translated text successfully");
}

// ---------------------------------------------------------------------------
// Test 5: RunnablePassthrough::with_assign feeding retriever results
// ---------------------------------------------------------------------------

/// Passthrough preserves original input and augments it with computed fields
/// from a mock retriever.
#[tokio::test]
async fn passthrough_with_assign_feeds_retriever() {
    let mock_retriever = RunnableLambda::new(|input: Value, _config| async move {
        let query = input
            .get("question")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let context = format!("Retrieved context for: {}", query);
        Ok(Value::String(context))
    });

    let passthrough =
        RunnablePassthrough::new().assign("context", Box::new(mock_retriever));

    let config = RunnableConfig::default();
    let input = serde_json::json!({
        "question": "What is Rust?"
    });

    let result = passthrough.invoke(input, &config).await.unwrap();

    // Original input preserved
    assert_eq!(result["question"], "What is Rust?");
    // Context added by retriever
    assert_eq!(result["context"], "Retrieved context for: What is Rust?");
}

// ---------------------------------------------------------------------------
// Test 6: Three-stage pipeline: parse input -> transform -> format output
// ---------------------------------------------------------------------------

#[tokio::test]
async fn three_stage_pipeline() {
    // Stage 1: Parse input string into JSON
    let parse_stage = RunnableLambda::new(|input: String, _config| async move {
        let value: Value = serde_json::from_str(&input)
            .map_err(|e| AyasError::Other(format!("parse error: {e}")))?;
        Ok(value)
    });

    // Stage 2: Transform — extract and compute derived fields
    let transform_stage = RunnableLambda::new(|input: Value, _config| async move {
        let name = input
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let age = input.get("age").and_then(|v| v.as_u64()).unwrap_or(0);
        Ok(serde_json::json!({
            "greeting": format!("Hello, {}!", name),
            "birth_year": 2025 - age,
        }))
    });

    // Stage 3: Format output to final string
    let format_stage = RunnableLambda::new(|input: Value, _config| async move {
        let greeting = input["greeting"].as_str().unwrap_or("");
        let birth_year = input["birth_year"].as_u64().unwrap_or(0);
        Ok(format!("{} You were born around {}.", greeting, birth_year))
    });

    let pipeline = parse_stage.pipe(transform_stage).pipe(format_stage);
    let config = RunnableConfig::default();

    let result = pipeline
        .invoke(r#"{"name": "Alice", "age": 30}"#.to_string(), &config)
        .await
        .unwrap();

    assert_eq!(result, "Hello, Alice! You were born around 1995.");
}

// ---------------------------------------------------------------------------
// Test 7: Error propagation through multi-stage pipeline
// ---------------------------------------------------------------------------

/// Stage 2 fails; error propagates through the pipeline without reaching stage 3.
#[tokio::test]
async fn error_propagation_through_pipeline() {
    let stage1 =
        RunnableLambda::new(|input: String, _config| async move { Ok(input.to_uppercase()) });

    let stage2 = RunnableLambda::new(|_input: String, _config| async move {
        Err::<String, _>(AyasError::Other("stage 2 processing failed".into()))
    });

    let stage3 = RunnableLambda::new(|input: String, _config| async move {
        Ok(format!("final: {}", input))
    });

    let pipeline = stage1.pipe(stage2).pipe(stage3);
    let config = RunnableConfig::default();

    let result = pipeline.invoke("test input".to_string(), &config).await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("stage 2 processing failed"));
}
