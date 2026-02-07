use std::collections::HashMap;

use ayas_chain::mock::MockChatModel;
use ayas_chain::parser::StringOutputParser;
use ayas_chain::prompt::PromptTemplate;
use ayas_core::config::RunnableConfig;
use ayas_core::runnable::{Runnable, RunnableExt};

/// E2E test: PromptTemplate -> MockChatModel -> StringOutputParser
///
/// This is the Sprint 3 milestone test that validates the full chain
/// composition: `prompt.pipe(model).pipe(parser)` works correctly.
#[tokio::test]
async fn prompt_to_model_to_parser_chain() {
    // 1. Create a prompt template
    let prompt = PromptTemplate::from_messages(vec![
        ("system", "You are a helpful assistant who speaks {language}."),
        ("user", "Tell me about {topic}."),
    ]);

    // 2. Create a mock model
    let model = MockChatModel::with_response("Rust is a systems programming language.");

    // 3. Create a parser
    let parser = StringOutputParser;

    // 4. Compose the chain: prompt -> model -> parser
    let chain = prompt.pipe(model).pipe(parser);

    // 5. Prepare input variables
    let mut vars = HashMap::new();
    vars.insert("language".into(), "English".into());
    vars.insert("topic".into(), "Rust".into());

    // 6. Execute
    let config = RunnableConfig::default();
    let result = chain.invoke(vars, &config).await.unwrap();

    assert_eq!(result, "Rust is a systems programming language.");
}

#[tokio::test]
async fn chain_with_cycling_model() {
    let prompt = PromptTemplate::from_template("Question: {question}");
    let model = MockChatModel::new(vec![
        "Answer 1".into(),
        "Answer 2".into(),
    ]);
    let parser = StringOutputParser;
    let chain = prompt.pipe(model).pipe(parser);

    let config = RunnableConfig::default();

    // First call
    let mut vars = HashMap::new();
    vars.insert("question".into(), "What is 1+1?".into());
    let result = chain.invoke(vars, &config).await.unwrap();
    assert_eq!(result, "Answer 1");

    // Second call
    let mut vars = HashMap::new();
    vars.insert("question".into(), "What is 2+2?".into());
    let result = chain.invoke(vars, &config).await.unwrap();
    assert_eq!(result, "Answer 2");
}

#[tokio::test]
async fn chain_missing_variable_propagates_error() {
    let prompt = PromptTemplate::from_template("Hello, {name}!");
    let model = MockChatModel::with_response("Response");
    let parser = StringOutputParser;
    let chain = prompt.pipe(model).pipe(parser);

    let config = RunnableConfig::default();
    let vars = HashMap::new(); // empty - missing 'name'

    let result = chain.invoke(vars, &config).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn batch_chain_execution() {
    let prompt = PromptTemplate::from_template("{input}");
    let model = MockChatModel::with_response("processed");
    let parser = StringOutputParser;
    let chain = prompt.pipe(model).pipe(parser);

    let config = RunnableConfig::default();
    let inputs: Vec<HashMap<String, String>> = (0..3)
        .map(|i| {
            let mut m = HashMap::new();
            m.insert("input".into(), format!("item {i}"));
            m
        })
        .collect();

    let results = chain.batch(inputs, &config).await.unwrap();
    assert_eq!(results.len(), 3);
    assert!(results.iter().all(|r| r == "processed"));
}
