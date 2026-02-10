use serde::{Deserialize, Serialize};

use ayas_core::message::Message;
use ayas_llm::provider::Provider;

// --- Chat ---

#[derive(Debug, Deserialize)]
pub struct ChatInvokeRequest {
    pub provider: Provider,
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ChatInvokeResponse {
    pub content: String,
    pub tokens_in: u64,
    pub tokens_out: u64,
}

// --- Agent ---

#[derive(Debug, Deserialize)]
pub struct AgentInvokeRequest {
    pub provider: Provider,
    pub model: String,
    #[serde(default)]
    pub tools: Vec<String>,
    pub messages: Vec<Message>,
    #[serde(default)]
    pub recursion_limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentSseEvent {
    Step {
        step_number: usize,
        node_name: String,
        summary: String,
    },
    ToolCall {
        tool_name: String,
        arguments: serde_json::Value,
    },
    ToolResult {
        tool_name: String,
        result: String,
    },
    Message {
        content: String,
    },
    Done {
        total_steps: usize,
    },
    Error {
        message: String,
    },
}

// --- Graph ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphNodeDto {
    pub id: String,
    #[serde(rename = "type")]
    pub node_type: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub config: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphEdgeDto {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub condition: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphChannelDto {
    pub key: String,
    #[serde(rename = "type")]
    pub channel_type: String,
    #[serde(default)]
    pub default: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct GraphValidateRequest {
    pub nodes: Vec<GraphNodeDto>,
    pub edges: Vec<GraphEdgeDto>,
    #[serde(default)]
    pub channels: Vec<GraphChannelDto>,
}

#[derive(Debug, Serialize)]
pub struct GraphValidateResponse {
    pub valid: bool,
    #[serde(default)]
    pub errors: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct GraphExecuteRequest {
    pub nodes: Vec<GraphNodeDto>,
    pub edges: Vec<GraphEdgeDto>,
    #[serde(default)]
    pub channels: Vec<GraphChannelDto>,
    #[serde(default)]
    pub input: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct GraphGenerateRequest {
    pub prompt: String,
    pub provider: Provider,
    pub model: String,
}

#[derive(Debug, Serialize)]
pub struct GraphGenerateResponse {
    pub nodes: Vec<GraphNodeDto>,
    pub edges: Vec<GraphEdgeDto>,
    pub channels: Vec<GraphChannelDto>,
}

// --- HITL ---

#[derive(Debug, Deserialize)]
pub struct ExecuteResumableRequest {
    pub thread_id: String,
    pub nodes: Vec<GraphNodeDto>,
    pub edges: Vec<GraphEdgeDto>,
    #[serde(default)]
    pub channels: Vec<GraphChannelDto>,
    #[serde(default)]
    pub input: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct ResumeRequest {
    pub session_id: String,
    pub resume_value: serde_json::Value,
}

// --- Research ---

#[derive(Debug, Deserialize)]
pub struct ResearchInvokeRequest {
    pub query: String,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub previous_interaction_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_request_deserialize() {
        let json = r#"{
            "provider": "gemini",
            "model": "gemini-2.0-flash",
            "messages": [{"type": "user", "content": "hello"}],
            "temperature": 0.7
        }"#;
        let req: ChatInvokeRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.provider, Provider::Gemini);
        assert_eq!(req.model, "gemini-2.0-flash");
        assert_eq!(req.messages.len(), 1);
        assert_eq!(req.temperature, Some(0.7));
    }

    #[test]
    fn chat_response_serialize() {
        let resp = ChatInvokeResponse {
            content: "Hello!".into(),
            tokens_in: 10,
            tokens_out: 5,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"content\":\"Hello!\""));
        assert!(json.contains("\"tokens_in\":10"));
    }

    #[test]
    fn agent_request_deserialize() {
        let json = r#"{
            "provider": "gemini",
            "model": "gemini-2.0-flash",
            "tools": ["calculator", "datetime"],
            "messages": [{"type": "user", "content": "What time is it?"}]
        }"#;
        let req: AgentInvokeRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.tools.len(), 2);
    }

    #[test]
    fn agent_sse_event_serialize() {
        let event = AgentSseEvent::ToolCall {
            tool_name: "calculator".into(),
            arguments: serde_json::json!({"expression": "2+3"}),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"tool_call\""));
        assert!(json.contains("\"tool_name\":\"calculator\""));
    }

    #[test]
    fn graph_node_dto_deserialize() {
        let json = r#"{
            "id": "node_1",
            "type": "llm",
            "label": "My LLM",
            "config": {"provider": "gemini", "model": "gemini-2.0-flash"}
        }"#;
        let node: GraphNodeDto = serde_json::from_str(json).unwrap();
        assert_eq!(node.id, "node_1");
        assert_eq!(node.node_type, "llm");
    }

    #[test]
    fn research_request_deserialize() {
        let json = r#"{
            "query": "What is Rust?",
            "agent": "gemini"
        }"#;
        let req: ResearchInvokeRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.query, "What is Rust?");
        assert_eq!(req.agent.as_deref(), Some("gemini"));
    }
}
