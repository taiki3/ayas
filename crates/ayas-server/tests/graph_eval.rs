//! E2E evaluation tests for graph generation via LLM.
//!
//! These tests call a real LLM API to generate graph structures, then evaluate
//! them using both rule-based checks and LLM-as-Judge scoring.
//!
//! Run with: `GEMINI_API_KEY=xxx cargo test -p ayas-server --test graph_eval -- --ignored`

use std::collections::HashSet;

use ayas_core::message::Message;
use ayas_core::model::{CallOptions, ChatModel};
use ayas_llm::factory::create_chat_model;
use ayas_llm::provider::Provider;
use ayas_server::graph_convert::validate_graph;
use ayas_server::graph_gen::generate_graph;
use ayas_server::types::{GraphChannelDto, GraphEdgeDto, GraphNodeDto};
use ayas_smith::prelude::*;
use serde::Deserialize;

// --- Rule-based validation ---

struct StructureCheckResult {
    errors: Vec<String>,
    warnings: Vec<String>,
}

fn check_graph_structure(
    nodes: &[GraphNodeDto],
    edges: &[GraphEdgeDto],
    channels: &[GraphChannelDto],
    expected_types: &[&str],
    min_nodes: usize,
) -> StructureCheckResult {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    // 1. Reuse existing validate_graph
    let base_errors = validate_graph(nodes, edges, channels);
    errors.extend(base_errors);

    // 2. Valid node types only
    let valid_types: HashSet<&str> = ["llm", "transform", "conditional", "passthrough"]
        .iter()
        .copied()
        .collect();
    for node in nodes {
        if !valid_types.contains(node.node_type.as_str()) {
            errors.push(format!(
                "Invalid node type '{}' on node '{}'",
                node.node_type, node.id
            ));
        }
    }

    // 3. Minimum node count
    if nodes.len() < min_nodes {
        errors.push(format!(
            "Expected at least {} nodes, got {}",
            min_nodes,
            nodes.len()
        ));
    }

    // 4. Expected node types present
    let present_types: HashSet<&str> = nodes.iter().map(|n| n.node_type.as_str()).collect();
    for expected in expected_types {
        if !present_types.contains(expected) {
            errors.push(format!("Expected node type '{}' not found", expected));
        }
    }

    // 5. Channel existence
    if channels.is_empty() {
        warnings.push("No channels defined".into());
    }

    // 6. Dangling edges (edges referencing non-existent nodes)
    let node_ids: HashSet<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
    for edge in edges {
        if edge.from != "start" && !node_ids.contains(edge.from.as_str()) {
            errors.push(format!("Dangling edge from '{}'", edge.from));
        }
        if edge.to != "end" && !node_ids.contains(edge.to.as_str()) {
            errors.push(format!("Dangling edge to '{}'", edge.to));
        }
    }

    StructureCheckResult { errors, warnings }
}

// --- LLM-as-Judge ---

#[derive(Debug, Deserialize)]
struct JudgeScores {
    relevance: u32,
    completeness: u32,
    correctness: u32,
}

async fn llm_judge(
    model: &dyn ChatModel,
    prompt: &str,
    graph_json: &str,
) -> Result<JudgeScores, String> {
    let judge_prompt = format!(
        r#"You are evaluating a generated graph pipeline. The user asked for: "{prompt}"

The generated graph (JSON):
{graph_json}

Rate the following on a scale of 1-5:
1. **Relevance**: How well does this graph match what the user asked for?
2. **Completeness**: Does it include all necessary components for what was requested? A simple request (e.g. "Q&A pipeline") only needs a simple graph — do not penalize simplicity when the request itself is simple.
3. **Correctness**: Is the graph structure logically sound (proper start/end edges, valid flow, no dangling nodes)?

Respond with ONLY a JSON object: {{"relevance": N, "completeness": N, "correctness": N}}"#
    );

    let messages = vec![Message::user(judge_prompt.as_str())];
    let options = CallOptions {
        temperature: Some(0.1),
        ..Default::default()
    };

    let result = model
        .generate(&messages, &options)
        .await
        .map_err(|e| format!("Judge LLM call failed: {e}"))?;

    let text = result.message.content().to_string();
    let trimmed = text.trim();

    // Try direct parse, then brace extraction
    if let Ok(scores) = serde_json::from_str::<JudgeScores>(trimmed) {
        return Ok(scores);
    }

    if let Some(start) = trimmed.find('{') {
        if let Some(end) = trimmed.rfind('}') {
            let json_str = &trimmed[start..=end];
            if let Ok(scores) = serde_json::from_str::<JudgeScores>(json_str) {
                return Ok(scores);
            }
        }
    }

    Err(format!("Failed to parse judge response: {}", &trimmed[..trimmed.len().min(200)]))
}

// --- Test infrastructure ---

fn get_model() -> Box<dyn ChatModel> {
    let api_key = std::env::var("GEMINI_API_KEY").expect("GEMINI_API_KEY must be set");
    create_chat_model(&Provider::Gemini, api_key, "gemini-2.0-flash".into())
}

fn create_smith_client(trace_dir: &std::path::Path) -> SmithClient {
    let config = SmithConfig::default()
        .with_project("graph-generate-eval")
        .with_base_dir(trace_dir)
        .with_batch_size(1);
    SmithClient::new(config)
}

struct EvalResult {
    structure: StructureCheckResult,
    judge: Option<JudgeScores>,
    nodes: Vec<GraphNodeDto>,
    edges: Vec<GraphEdgeDto>,
    channels: Vec<GraphChannelDto>,
}

async fn run_eval(
    prompt: &str,
    expected_types: &[&str],
    min_nodes: usize,
) -> EvalResult {
    let model = get_model();

    let (nodes, edges, channels) = generate_graph(model.as_ref(), prompt)
        .await
        .expect("generate_graph should succeed");

    let structure = check_graph_structure(&nodes, &edges, &channels, expected_types, min_nodes);

    let graph_json = serde_json::to_string_pretty(&serde_json::json!({
        "nodes": nodes,
        "edges": edges,
        "channels": channels,
    }))
    .unwrap();

    let judge = match llm_judge(model.as_ref(), prompt, &graph_json).await {
        Ok(scores) => Some(scores),
        Err(e) => {
            eprintln!("Judge failed (non-fatal): {e}");
            None
        }
    };

    EvalResult {
        structure,
        judge,
        nodes,
        edges,
        channels,
    }
}

fn record_trace(
    client: &SmithClient,
    test_name: &str,
    prompt: &str,
    result: &EvalResult,
) {
    let structure_ok = result.structure.errors.is_empty();
    let judge_ok = result
        .judge
        .as_ref()
        .is_some_and(|j| j.relevance >= 3 && j.completeness >= 3 && j.correctness >= 3);

    let metadata = serde_json::json!({
        "test_name": test_name,
        "prompt": prompt,
        "node_count": result.nodes.len(),
        "edge_count": result.edges.len(),
        "channel_count": result.channels.len(),
        "structure_errors": result.structure.errors,
        "structure_warnings": result.structure.warnings,
        "judge_scores": result.judge.as_ref().map(|j| serde_json::json!({
            "relevance": j.relevance,
            "completeness": j.completeness,
            "correctness": j.correctness,
        })),
        "structure_ok": structure_ok,
        "judge_ok": judge_ok,
    });

    let output_json = serde_json::to_string(&serde_json::json!({
        "nodes": result.nodes,
        "edges": result.edges,
        "channels": result.channels,
    }))
    .unwrap_or_default();

    let status_ok = structure_ok && judge_ok;
    let run = if status_ok {
        Run::builder(test_name, RunType::Chain)
            .project("graph-generate-eval")
            .input(prompt)
            .tags(vec!["eval".into(), "gemini".into()])
            .metadata(metadata.to_string())
            .finish_ok(output_json)
    } else {
        let error_msg = format!(
            "structure_ok={}, judge_ok={}, errors={:?}",
            structure_ok, judge_ok, result.structure.errors
        );
        Run::builder(test_name, RunType::Chain)
            .project("graph-generate-eval")
            .input(prompt)
            .tags(vec!["eval".into(), "gemini".into()])
            .metadata(metadata.to_string())
            .finish_err(error_msg)
    };

    client.submit_run(run);
}

fn assert_eval(result: &EvalResult, test_name: &str) {
    // Print details for debugging
    eprintln!("=== {test_name} ===");
    eprintln!("Nodes: {:?}", result.nodes.iter().map(|n| format!("{}({})", n.id, n.node_type)).collect::<Vec<_>>());
    eprintln!("Edges: {:?}", result.edges.iter().map(|e| format!("{}→{}", e.from, e.to)).collect::<Vec<_>>());
    if !result.structure.errors.is_empty() {
        eprintln!("Structure errors: {:?}", result.structure.errors);
    }
    if let Some(ref j) = result.judge {
        eprintln!("Judge scores: relevance={}, completeness={}, correctness={}", j.relevance, j.completeness, j.correctness);
    }

    // Structure checks
    assert!(
        result.structure.errors.is_empty(),
        "[{test_name}] Structure errors: {:?}",
        result.structure.errors
    );

    // Judge checks (if available)
    if let Some(ref scores) = result.judge {
        assert!(
            scores.relevance >= 3,
            "[{test_name}] Relevance score {} < 3",
            scores.relevance
        );
        assert!(
            scores.completeness >= 3,
            "[{test_name}] Completeness score {} < 3",
            scores.completeness
        );
        assert!(
            scores.correctness >= 3,
            "[{test_name}] Correctness score {} < 3",
            scores.correctness
        );
    }
}

// --- Test cases ---

#[tokio::test]
#[ignore]
async fn eval_simple_qa() {
    let trace_dir = tempfile::tempdir().unwrap();
    let client = create_smith_client(trace_dir.path());

    let prompt = "Simple Q&A pipeline: takes user input, processes it through an LLM, and returns the answer";
    let result = run_eval(prompt, &["llm"], 1).await;
    record_trace(&client, "eval_simple_qa", prompt, &result);
    assert_eval(&result, "eval_simple_qa");
}

#[tokio::test]
#[ignore]
async fn eval_rag_pipeline() {
    let trace_dir = tempfile::tempdir().unwrap();
    let client = create_smith_client(trace_dir.path());

    let result = run_eval(
        "RAG pipeline with document retrieval and answer generation",
        &["llm"],
        2,
    )
    .await;
    record_trace(
        &client,
        "eval_rag_pipeline",
        "RAG pipeline with document retrieval and answer generation",
        &result,
    );
    assert_eval(&result, "eval_rag_pipeline");
}

#[tokio::test]
#[ignore]
async fn eval_chatbot_routing() {
    let trace_dir = tempfile::tempdir().unwrap();
    let client = create_smith_client(trace_dir.path());

    let result = run_eval(
        "Chatbot with intent routing that sends FAQ questions to one handler and support requests to another",
        &["conditional"],
        3,
    )
    .await;
    record_trace(
        &client,
        "eval_chatbot_routing",
        "Chatbot with intent routing",
        &result,
    );
    assert_eval(&result, "eval_chatbot_routing");
}

#[tokio::test]
#[ignore]
async fn eval_iterative_refinement() {
    let trace_dir = tempfile::tempdir().unwrap();
    let client = create_smith_client(trace_dir.path());

    let result = run_eval(
        "Iterative refinement loop: an LLM generates a draft, a reviewer evaluates it, and if not good enough routes back to the LLM for improvement",
        &["llm", "conditional"],
        3,
    )
    .await;
    record_trace(
        &client,
        "eval_iterative_refinement",
        "Iterative refinement loop",
        &result,
    );
    assert_eval(&result, "eval_iterative_refinement");
}
