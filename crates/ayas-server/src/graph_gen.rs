use serde::Deserialize;

use ayas_core::message::Message;
use ayas_core::model::{CallOptions, ChatModel};

use crate::types::{GraphChannelDto, GraphEdgeDto, GraphNodeDto};

#[derive(Debug, Deserialize)]
pub struct ParsedGraph {
    pub nodes: Vec<GraphNodeDto>,
    pub edges: Vec<GraphEdgeDto>,
    pub channels: Vec<GraphChannelDto>,
}

/// System prompt that teaches the LLM how to generate graph structures.
pub fn graph_system_prompt() -> String {
    r#"You are a graph pipeline designer. Given a user's description, generate a JSON graph structure.

The graph has these components:

**Nodes** (types: "llm", "transform", "conditional", "passthrough"):
- "llm": Calls a language model. Config may include "prompt".
- "transform": Applies a data transformation. Config may include "expression".
- "conditional": Routes based on state. Used with conditional edges.
- "passthrough": Passes state through unchanged.

Note: "start" and "end" are implicit — do NOT include them as nodes.

**Edges**: Connect nodes. "from"/"to" fields. Use "start" and "end" as virtual endpoints.
- Every graph must have at least one edge from "start" and one edge to "end".
- Optional "condition" field for conditional routing (references a state key).

**Channels**: Define state keys. Types: "LastValue" (single value) or "Append" (list).
- At minimum, include a "value" channel of type "LastValue".

Respond with ONLY valid JSON (no markdown fencing, no explanation). The JSON must have exactly these top-level keys: "nodes", "edges", "channels".

Example 1 — Simple Q&A:
{"nodes":[{"id":"qa_llm","type":"llm","label":"Q&A Model"}],"edges":[{"from":"start","to":"qa_llm"},{"from":"qa_llm","to":"end"}],"channels":[{"key":"value","type":"LastValue"}]}

Example 2 — Intent routing:
{"nodes":[{"id":"classifier","type":"llm","label":"Intent Classifier"},{"id":"router","type":"conditional","label":"Route by Intent"},{"id":"faq_handler","type":"llm","label":"FAQ Handler"},{"id":"support_handler","type":"llm","label":"Support Handler"}],"edges":[{"from":"start","to":"classifier"},{"from":"classifier","to":"router"},{"from":"router","to":"faq_handler","condition":"is_faq"},{"from":"router","to":"support_handler","condition":"is_support"},{"from":"faq_handler","to":"end"},{"from":"support_handler","to":"end"}],"channels":[{"key":"value","type":"LastValue"},{"key":"is_faq","type":"LastValue"},{"key":"is_support","type":"LastValue"}]}"#.to_string()
}

/// Parse the LLM response text into a ParsedGraph.
///
/// Tries three strategies in order:
/// 1. Direct JSON parse of the entire text
/// 2. Extract from ```json ... ``` markdown block
/// 3. Extract first `{ ... }` brace-delimited block
pub fn parse_graph_response(text: &str) -> Result<ParsedGraph, String> {
    let trimmed = text.trim();

    // Strategy 1: direct parse
    if let Ok(parsed) = serde_json::from_str::<ParsedGraph>(trimmed) {
        return Ok(parsed);
    }

    // Strategy 2: markdown ```json block
    if let Some(start) = trimmed.find("```json") {
        let after_fence = &trimmed[start + 7..];
        if let Some(end) = after_fence.find("```") {
            let json_str = after_fence[..end].trim();
            if let Ok(parsed) = serde_json::from_str::<ParsedGraph>(json_str) {
                return Ok(parsed);
            }
        }
    }
    // Also try plain ``` block
    if let Some(start) = trimmed.find("```\n") {
        let after_fence = &trimmed[start + 4..];
        if let Some(end) = after_fence.find("```") {
            let json_str = after_fence[..end].trim();
            if let Ok(parsed) = serde_json::from_str::<ParsedGraph>(json_str) {
                return Ok(parsed);
            }
        }
    }

    // Strategy 3: brace extraction
    if let Some(start) = trimmed.find('{') {
        let mut depth = 0;
        let bytes = trimmed.as_bytes();
        for (i, &b) in bytes[start..].iter().enumerate() {
            match b {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        let json_str = &trimmed[start..start + i + 1];
                        if let Ok(parsed) = serde_json::from_str::<ParsedGraph>(json_str) {
                            return Ok(parsed);
                        }
                        break;
                    }
                }
                _ => {}
            }
        }
    }

    Err(format!(
        "Failed to parse graph from LLM response: {}",
        &trimmed[..trimmed.len().min(200)]
    ))
}

/// Generate a graph structure from a user prompt using an LLM.
pub async fn generate_graph(
    model: &dyn ChatModel,
    prompt: &str,
) -> Result<(Vec<GraphNodeDto>, Vec<GraphEdgeDto>, Vec<GraphChannelDto>), String> {
    let system_prompt = graph_system_prompt();
    let messages = vec![
        Message::system(system_prompt.as_str()),
        Message::user(prompt),
    ];
    let options = CallOptions {
        temperature: Some(0.2),
        ..Default::default()
    };

    let result = model
        .generate(&messages, &options)
        .await
        .map_err(|e| format!("LLM call failed: {e}"))?;

    let text = result.message.content().to_string();
    let parsed = parse_graph_response(&text)?;

    Ok((parsed.nodes, parsed.edges, parsed.channels))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_direct_json() {
        let json = r#"{"nodes":[{"id":"n1","type":"llm"}],"edges":[{"from":"start","to":"n1"},{"from":"n1","to":"end"}],"channels":[{"key":"value","type":"LastValue"}]}"#;
        let result = parse_graph_response(json);
        assert!(result.is_ok(), "Failed: {:?}", result.err());
        let g = result.unwrap();
        assert_eq!(g.nodes.len(), 1);
        assert_eq!(g.edges.len(), 2);
        assert_eq!(g.channels.len(), 1);
    }

    #[test]
    fn parse_markdown_json_block() {
        let text = r#"Here is the graph:

```json
{"nodes":[{"id":"t1","type":"transform"}],"edges":[{"from":"start","to":"t1"},{"from":"t1","to":"end"}],"channels":[{"key":"value","type":"LastValue"}]}
```

That should work."#;
        let result = parse_graph_response(text);
        assert!(result.is_ok(), "Failed: {:?}", result.err());
        assert_eq!(result.unwrap().nodes[0].id, "t1");
    }

    #[test]
    fn parse_plain_backtick_block() {
        let text = "```\n{\"nodes\":[{\"id\":\"p1\",\"type\":\"passthrough\"}],\"edges\":[{\"from\":\"start\",\"to\":\"p1\"},{\"from\":\"p1\",\"to\":\"end\"}],\"channels\":[{\"key\":\"value\",\"type\":\"LastValue\"}]}\n```";
        let result = parse_graph_response(text);
        assert!(result.is_ok(), "Failed: {:?}", result.err());
        assert_eq!(result.unwrap().nodes[0].node_type, "passthrough");
    }

    #[test]
    fn parse_brace_extraction() {
        let text = r#"Sure! Here is the result: {"nodes":[{"id":"a","type":"llm"}],"edges":[{"from":"start","to":"a"},{"from":"a","to":"end"}],"channels":[{"key":"value","type":"LastValue"}]} Hope that helps!"#;
        let result = parse_graph_response(text);
        assert!(result.is_ok(), "Failed: {:?}", result.err());
        assert_eq!(result.unwrap().nodes[0].id, "a");
    }

    #[test]
    fn parse_whitespace_around_json() {
        let json = r#"
        {
            "nodes": [{"id": "n1", "type": "llm"}],
            "edges": [{"from": "start", "to": "n1"}, {"from": "n1", "to": "end"}],
            "channels": [{"key": "value", "type": "LastValue"}]
        }
        "#;
        let result = parse_graph_response(json);
        assert!(result.is_ok(), "Failed: {:?}", result.err());
    }

    #[test]
    fn parse_failure_returns_error() {
        let text = "This is not JSON at all";
        let result = parse_graph_response(text);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Failed to parse"));
    }
}
