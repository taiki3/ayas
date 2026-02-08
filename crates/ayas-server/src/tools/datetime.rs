use async_trait::async_trait;
use ayas_core::error::Result;
use ayas_core::tool::{Tool, ToolDefinition};

/// Tool that returns the current date and time.
pub struct DateTimeTool;

#[async_trait]
impl Tool for DateTimeTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "datetime".into(),
            description: "Returns the current date and time in UTC.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        }
    }

    async fn call(&self, _input: serde_json::Value) -> Result<String> {
        let now = chrono::Utc::now();
        Ok(now.format("%Y-%m-%d %H:%M:%S UTC").to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definition_has_correct_schema() {
        let tool = DateTimeTool;
        let def = tool.definition();
        assert_eq!(def.name, "datetime");
        assert!(def.description.contains("date"));
    }

    #[tokio::test]
    async fn returns_current_time() {
        let tool = DateTimeTool;
        let result = tool.call(serde_json::json!({})).await.unwrap();
        // Should contain a date in YYYY-MM-DD format
        assert!(result.contains("-")); // e.g., 2026-02-07
        assert!(result.contains("UTC"));
    }

    #[tokio::test]
    async fn returns_string() {
        let tool = DateTimeTool;
        let result = tool.call(serde_json::json!({})).await.unwrap();
        assert!(!result.is_empty());
    }
}
