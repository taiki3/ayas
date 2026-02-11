use std::collections::HashMap;
use std::sync::Arc;

use ayas_checkpoint::prelude::{send_output, SendDirective};
use ayas_core::config::RunnableConfig;
use ayas_core::error::{AyasError, Result};
use ayas_core::message::Message;
use ayas_core::model::{CallOptions, ChatModel};
use ayas_core::runnable::Runnable;
use ayas_graph::compiled::CompiledStateGraph;
use ayas_graph::constants::END;
use ayas_graph::edge::ConditionalEdge;
use ayas_graph::node::NodeFn;
use ayas_graph::state_graph::StateGraph;
use serde_json::{json, Value};

/// Configuration for a worker agent in a supervisor system.
pub struct WorkerConfig {
    /// Unique name of this worker (used as the routing key).
    pub name: String,
    /// Human-readable description of the worker's capabilities.
    pub description: String,
    /// The compiled sub-graph that implements this worker's behavior.
    pub agent: Arc<CompiledStateGraph>,
}

/// Create a supervisor agent that orchestrates multiple worker agents via LLM routing.
///
/// The supervisor follows a loop: an LLM-powered **router** decides which
/// worker(s) to dispatch, a **dispatch** node fans them out via the Send API
/// for parallel execution, and the results are fed back to the router until
/// it signals `FINISH`.
///
/// # Graph structure
/// ```text
/// router ──┬─ FINISH ──→ END
///           └─ workers ──→ dispatch ──→ [worker_* via Send] ──→ router
/// ```
///
/// # State schema
/// - `messages`: `AppendChannel` — shared conversation history
/// - `next`: `LastValue` — the router's decision (`["worker_name"]` or `["FINISH"]`)
///
/// # Example
/// ```ignore
/// let supervisor = create_supervisor_agent(
///     model,
///     vec![
///         WorkerConfig { name: "researcher".into(), description: "Searches for info".into(), agent: researcher },
///         WorkerConfig { name: "coder".into(), description: "Writes code".into(), agent: coder },
///     ],
///     Some("Coordinate research and coding.".into()),
/// )?;
/// let result = supervisor.invoke(
///     json!({"messages": [{"type":"user","content":"Build a web scraper"}]}),
///     &config,
/// ).await?;
/// ```
pub fn create_supervisor_agent(
    model: Arc<dyn ChatModel>,
    workers: Vec<WorkerConfig>,
    system_prompt: Option<String>,
) -> Result<CompiledStateGraph> {
    let mut graph = StateGraph::new();
    graph.add_append_channel("messages");
    graph.add_last_value_channel("next", json!([]));

    // Build worker descriptions for the router prompt
    let worker_descriptions: String = workers
        .iter()
        .map(|w| format!("- {}: {}", w.name, w.description))
        .collect::<Vec<_>>()
        .join("\n");

    let worker_names: Vec<String> = workers.iter().map(|w| w.name.clone()).collect();

    let base_prompt = system_prompt.unwrap_or_default();
    let router_system_prompt = format!(
        "{base_prompt}\n\n\
         You are a supervisor managing these workers:\n\
         {worker_descriptions}\n\n\
         Given the conversation so far, which worker(s) should act next?\n\
         Respond with a JSON object: {{\"next\": [\"worker_name\"]}} or {{\"next\": [\"FINISH\"]}}\n\
         You can select multiple workers to run in parallel: {{\"next\": [\"worker_a\", \"worker_b\"]}}"
    );

    // ── Router node: calls LLM to decide which workers to dispatch ──
    let model_clone = model;
    let router_prompt = router_system_prompt;
    graph.add_node(NodeFn::new(
        "router",
        move |state: Value, _config| {
            let model = model_clone.clone();
            let system_prompt = router_prompt.clone();
            async move {
                let mut messages = parse_messages(&state["messages"])?;

                let has_system = messages
                    .iter()
                    .any(|m| matches!(m, Message::System { .. }));
                if !has_system {
                    messages.insert(0, Message::system(system_prompt.as_str()));
                }

                let options = CallOptions::default();
                let result = model.generate(&messages, &options).await?;

                let content = result.message.content().to_string();
                let next = parse_next_from_response(&content);

                let msg_value = serde_json::to_value(&result.message)
                    .map_err(AyasError::Serialization)?;

                Ok(json!({"messages": msg_value, "next": next}))
            }
        },
    ))?;

    // ── Dispatch node: creates Send directives to selected workers ──
    let valid_worker_names = worker_names;
    graph.add_node(NodeFn::new(
        "dispatch",
        move |state: Value, _config| {
            let valid_workers = valid_worker_names.clone();
            async move {
                let next: Vec<String> = match state.get("next") {
                    Some(Value::Array(arr)) => arr
                        .iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect(),
                    _ => Vec::new(),
                };

                let sends: Vec<SendDirective> = next
                    .into_iter()
                    .filter(|name| valid_workers.contains(name))
                    .map(|name| {
                        let worker_node = format!("worker_{name}");
                        SendDirective::new(&worker_node, json!({}))
                    })
                    .collect();

                Ok(send_output(sends))
            }
        },
    ))?;

    // ── Worker nodes: run sub-graphs and return only new messages ──
    for worker in &workers {
        let worker_graph = worker.agent.clone();
        graph.add_node(NodeFn::new(
            format!("worker_{}", worker.name),
            move |state: Value, config: RunnableConfig| {
                let graph = worker_graph.clone();
                async move {
                    let input_len = match state.get("messages") {
                        Some(Value::Array(arr)) => arr.len(),
                        _ => 0,
                    };

                    let sub_config = RunnableConfig {
                        recursion_limit: config.recursion_limit.saturating_sub(1),
                        ..config
                    };

                    let result = graph.invoke(state, &sub_config).await?;

                    // Extract only messages the worker added (beyond input)
                    let new_messages: Vec<Value> = match result.get("messages") {
                        Some(Value::Array(arr)) => arr.iter().skip(input_len).cloned().collect(),
                        _ => Vec::new(),
                    };

                    Ok(json!({"messages": new_messages}))
                }
            },
        ))?;
    }

    // ── Edges ──
    graph.set_entry_point("router");

    let mut path_map = HashMap::new();
    path_map.insert("dispatch".to_string(), "dispatch".to_string());
    path_map.insert("finish".to_string(), END.to_string());

    graph.add_conditional_edges(ConditionalEdge::new(
        "router",
        |state: &Value| {
            let next = match state.get("next") {
                Some(Value::Array(arr)) => arr,
                _ => return "finish".to_string(),
            };
            let is_finish =
                next.is_empty() || next.iter().any(|v| v.as_str() == Some("FINISH"));
            if is_finish {
                "finish".to_string()
            } else {
                "dispatch".to_string()
            }
        },
        Some(path_map),
    ));

    // dispatch → router (loop back after workers complete)
    // Use None path_map so that validation considers all nodes (including
    // Send-only worker nodes) reachable from dispatch.
    graph.add_conditional_edges(ConditionalEdge::new(
        "dispatch",
        |_: &Value| "router".to_string(),
        None,
    ));

    graph.compile()
}

// ── Helpers ──────────────────────────────────────────────────────────────

/// Parse the `"next"` array from the LLM's JSON response.
///
/// Falls back to `["FINISH"]` if the response cannot be parsed.
fn parse_next_from_response(content: &str) -> Vec<String> {
    let trimmed = content.trim();

    // Direct JSON parse
    if let Ok(val) = serde_json::from_str::<Value>(trimmed) {
        if let Some(next) = val.get("next") {
            return value_to_string_vec(next);
        }
    }

    // Extract JSON embedded in prose / markdown
    if let Some(start) = trimmed.find('{') {
        if let Some(end) = trimmed.rfind('}') {
            if start < end {
                if let Ok(val) = serde_json::from_str::<Value>(&trimmed[start..=end]) {
                    if let Some(next) = val.get("next") {
                        return value_to_string_vec(next);
                    }
                }
            }
        }
    }

    vec!["FINISH".to_string()]
}

fn value_to_string_vec(val: &Value) -> Vec<String> {
    match val {
        Value::Array(arr) => arr
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect(),
        Value::String(s) => vec![s.clone()],
        _ => vec!["FINISH".to_string()],
    }
}

fn parse_messages(value: &Value) -> Result<Vec<Message>> {
    match value {
        Value::Array(arr) => arr
            .iter()
            .map(|item| serde_json::from_value(item.clone()).map_err(AyasError::Serialization))
            .collect(),
        Value::Null => Ok(Vec::new()),
        _ => Err(AyasError::Other(
            "Expected messages to be a JSON array".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use ayas_core::model::ChatResult;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // ── Mock Chat Model ──

    struct MockSupervisorModel {
        responses: Vec<String>,
        call_count: AtomicUsize,
    }

    impl MockSupervisorModel {
        fn new(responses: Vec<String>) -> Self {
            Self {
                responses,
                call_count: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl ChatModel for MockSupervisorModel {
        async fn generate(
            &self,
            _messages: &[Message],
            _options: &CallOptions,
        ) -> ayas_core::error::Result<ChatResult> {
            let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
            let content = self
                .responses
                .get(idx)
                .cloned()
                .unwrap_or_else(|| r#"{"next": ["FINISH"]}"#.to_string());
            Ok(ChatResult {
                message: Message::ai(content),
                usage: None,
            })
        }

        fn model_name(&self) -> &str {
            "mock-supervisor"
        }
    }

    // ── Worker Helper ──

    /// Build a minimal worker graph that appends a single AI message.
    fn build_worker_graph(worker_name: &str) -> CompiledStateGraph {
        let mut g = StateGraph::new();
        g.add_append_channel("messages");

        let name = worker_name.to_string();
        g.add_node(NodeFn::new("work", move |_state: Value, _cfg| {
            let name = name.clone();
            async move {
                let msg = Message::ai(format!("Result from {name}"));
                let msg_value =
                    serde_json::to_value(&msg).map_err(AyasError::Serialization)?;
                Ok(json!({"messages": msg_value}))
            }
        }))
        .unwrap();

        g.set_entry_point("work");
        g.set_finish_point("work");
        g.compile().unwrap()
    }

    // ── Tests ──

    #[test]
    fn test_supervisor_compiles() {
        let model = Arc::new(MockSupervisorModel::new(vec![]));
        let researcher = Arc::new(build_worker_graph("researcher"));
        let coder = Arc::new(build_worker_graph("coder"));

        let result = create_supervisor_agent(
            model,
            vec![
                WorkerConfig {
                    name: "researcher".into(),
                    description: "Does research".into(),
                    agent: researcher,
                },
                WorkerConfig {
                    name: "coder".into(),
                    description: "Writes code".into(),
                    agent: coder,
                },
            ],
            Some("You coordinate tasks.".into()),
        );
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_supervisor_sequential_routing() {
        // Router: researcher → coder → FINISH
        let model = Arc::new(MockSupervisorModel::new(vec![
            r#"{"next": ["researcher"]}"#.to_string(),
            r#"{"next": ["coder"]}"#.to_string(),
            r#"{"next": ["FINISH"]}"#.to_string(),
        ]));

        let researcher = Arc::new(build_worker_graph("researcher"));
        let coder = Arc::new(build_worker_graph("coder"));

        let supervisor = create_supervisor_agent(
            model,
            vec![
                WorkerConfig {
                    name: "researcher".into(),
                    description: "Searches for information".into(),
                    agent: researcher,
                },
                WorkerConfig {
                    name: "coder".into(),
                    description: "Writes code".into(),
                    agent: coder,
                },
            ],
            None,
        )
        .unwrap();

        let config = RunnableConfig::default();
        let result = supervisor
            .invoke(
                json!({"messages": [{"type": "user", "content": "Build a web scraper"}]}),
                &config,
            )
            .await
            .unwrap();

        let messages = result["messages"].as_array().unwrap();
        // user + router(researcher) + researcher_result + router(coder) + coder_result + router(FINISH)
        assert_eq!(messages.len(), 6);

        let contents: Vec<&str> = messages
            .iter()
            .filter_map(|m| m["content"].as_str())
            .collect();
        assert!(contents.contains(&"Result from researcher"));
        assert!(contents.contains(&"Result from coder"));
    }

    #[tokio::test]
    async fn test_supervisor_immediate_finish() {
        let model = Arc::new(MockSupervisorModel::new(vec![
            r#"{"next": ["FINISH"]}"#.to_string(),
        ]));

        let researcher = Arc::new(build_worker_graph("researcher"));

        let supervisor = create_supervisor_agent(
            model,
            vec![WorkerConfig {
                name: "researcher".into(),
                description: "Does research".into(),
                agent: researcher,
            }],
            Some("You decide when work is needed.".into()),
        )
        .unwrap();

        let config = RunnableConfig::default();
        let result = supervisor
            .invoke(
                json!({"messages": [{"type": "user", "content": "Hello"}]}),
                &config,
            )
            .await
            .unwrap();

        let messages = result["messages"].as_array().unwrap();
        // user + router(FINISH)
        assert_eq!(messages.len(), 2);
    }

    #[tokio::test]
    async fn test_supervisor_parallel_workers() {
        // Router dispatches both workers in parallel, then FINISH
        let model = Arc::new(MockSupervisorModel::new(vec![
            r#"{"next": ["researcher", "coder"]}"#.to_string(),
            r#"{"next": ["FINISH"]}"#.to_string(),
        ]));

        let researcher = Arc::new(build_worker_graph("researcher"));
        let coder = Arc::new(build_worker_graph("coder"));

        let supervisor = create_supervisor_agent(
            model,
            vec![
                WorkerConfig {
                    name: "researcher".into(),
                    description: "Searches for information".into(),
                    agent: researcher,
                },
                WorkerConfig {
                    name: "coder".into(),
                    description: "Writes code".into(),
                    agent: coder,
                },
            ],
            None,
        )
        .unwrap();

        let config = RunnableConfig::default();
        let result = supervisor
            .invoke(
                json!({"messages": [{"type": "user", "content": "Build something"}]}),
                &config,
            )
            .await
            .unwrap();

        let messages = result["messages"].as_array().unwrap();
        // user + router(both) + researcher_result + coder_result + router(FINISH)
        assert_eq!(messages.len(), 5);

        let contents: Vec<&str> = messages
            .iter()
            .filter_map(|m| m["content"].as_str())
            .collect();
        assert!(contents.contains(&"Result from researcher"));
        assert!(contents.contains(&"Result from coder"));
    }

    // ── parse_next_from_response tests ──

    #[test]
    fn test_parse_next_direct_json() {
        assert_eq!(
            parse_next_from_response(r#"{"next": ["researcher"]}"#),
            vec!["researcher"]
        );
    }

    #[test]
    fn test_parse_next_multiple_workers() {
        assert_eq!(
            parse_next_from_response(r#"{"next": ["researcher", "coder"]}"#),
            vec!["researcher", "coder"]
        );
    }

    #[test]
    fn test_parse_next_finish() {
        assert_eq!(
            parse_next_from_response(r#"{"next": ["FINISH"]}"#),
            vec!["FINISH"]
        );
    }

    #[test]
    fn test_parse_next_embedded_json() {
        assert_eq!(
            parse_next_from_response(
                r#"I'll dispatch the researcher. {"next": ["researcher"]}"#
            ),
            vec!["researcher"]
        );
    }

    #[test]
    fn test_parse_next_unparseable_defaults_to_finish() {
        assert_eq!(
            parse_next_from_response("I'm not sure what to do next."),
            vec!["FINISH"]
        );
    }

    #[test]
    fn test_parse_next_string_value() {
        assert_eq!(
            parse_next_from_response(r#"{"next": "researcher"}"#),
            vec!["researcher"]
        );
    }
}
