use async_trait::async_trait;
use ayas_core::error::Result;
use ayas_core::tool::{Tool, ToolDefinition};

/// Stub web search tool (not yet implemented).
pub struct WebSearchTool;

#[async_trait]
impl Tool for WebSearchTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "web_search".into(),
            description: "Search the web for information (stub - not yet implemented).".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The search query"
                    }
                },
                "required": ["query"]
            }),
        }
    }

    async fn call(&self, _input: serde_json::Value) -> Result<String> {
        Ok("Web search is not yet implemented. This is a stub tool.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definition_has_correct_schema() {
        let tool = WebSearchTool;
        let def = tool.definition();
        assert_eq!(def.name, "web_search");
        assert!(def.description.contains("Search"));
    }

    #[tokio::test]
    async fn returns_stub_message() {
        let tool = WebSearchTool;
        let result = tool
            .call(serde_json::json!({"query": "test"}))
            .await
            .unwrap();
        assert!(result.contains("not yet"));
    }
}
