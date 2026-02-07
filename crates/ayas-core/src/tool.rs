use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::Result;

/// Definition of a tool that can be called by a model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// The name of the tool.
    pub name: String,

    /// A description of what the tool does.
    pub description: String,

    /// JSON Schema describing the tool's input parameters.
    pub parameters: serde_json::Value,
}

/// Trait for callable tools.
///
/// Tools accept JSON input and return string output.
/// The `definition()` method provides the tool's metadata and JSON Schema
/// for model function-calling.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Return the tool's definition including its JSON Schema.
    fn definition(&self) -> ToolDefinition;

    /// Execute the tool with the given JSON input.
    async fn call(&self, input: serde_json::Value) -> Result<String>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{AyasError, ToolError};

    struct CalculatorTool;

    #[async_trait]
    impl Tool for CalculatorTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                name: "calculator".into(),
                description: "Evaluates simple arithmetic expressions".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "expression": {
                            "type": "string",
                            "description": "The arithmetic expression to evaluate"
                        }
                    },
                    "required": ["expression"]
                }),
            }
        }

        async fn call(&self, input: serde_json::Value) -> Result<String> {
            let expr = input
                .get("expression")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    AyasError::Tool(ToolError::InvalidInput(
                        "missing 'expression' field".into(),
                    ))
                })?;

            // Trivial evaluator: only handles "a+b"
            if let Some((a, b)) = expr.split_once('+') {
                let a: f64 = a.trim().parse().map_err(|e: std::num::ParseFloatError| {
                    AyasError::Tool(ToolError::InvalidInput(e.to_string()))
                })?;
                let b: f64 = b.trim().parse().map_err(|e: std::num::ParseFloatError| {
                    AyasError::Tool(ToolError::InvalidInput(e.to_string()))
                })?;
                Ok((a + b).to_string())
            } else {
                Err(AyasError::Tool(ToolError::ExecutionFailed(
                    format!("unsupported expression: {expr}"),
                )))
            }
        }
    }

    #[tokio::test]
    async fn calculator_tool_definition() {
        let tool = CalculatorTool;
        let def = tool.definition();
        assert_eq!(def.name, "calculator");
        assert!(def.description.contains("arithmetic"));
        assert!(def.parameters.get("properties").is_some());
    }

    #[tokio::test]
    async fn calculator_tool_call_success() {
        let tool = CalculatorTool;
        let input = serde_json::json!({"expression": "2 + 3"});
        let result = tool.call(input).await.unwrap();
        assert_eq!(result, "5");
    }

    #[tokio::test]
    async fn calculator_tool_call_missing_field() {
        let tool = CalculatorTool;
        let input = serde_json::json!({"wrong_field": "2+3"});
        let result = tool.call(input).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, AyasError::Tool(ToolError::InvalidInput(_))));
    }

    #[tokio::test]
    async fn calculator_tool_call_unsupported_expression() {
        let tool = CalculatorTool;
        let input = serde_json::json!({"expression": "2 * 3"});
        let result = tool.call(input).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(
            err,
            AyasError::Tool(ToolError::ExecutionFailed(_))
        ));
    }

    #[test]
    fn tool_definition_serde_roundtrip() {
        let def = ToolDefinition {
            name: "test".into(),
            description: "A test tool".into(),
            parameters: serde_json::json!({"type": "object"}),
        };
        let json = serde_json::to_string(&def).unwrap();
        let deserialized: ToolDefinition = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, "test");
        assert_eq!(deserialized.description, "A test tool");
    }
}
