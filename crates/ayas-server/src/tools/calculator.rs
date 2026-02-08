use async_trait::async_trait;
use ayas_core::error::{AyasError, Result, ToolError};
use ayas_core::tool::{Tool, ToolDefinition};

/// Calculator tool that evaluates math expressions using the `meval` crate.
pub struct CalculatorTool;

#[async_trait]
impl Tool for CalculatorTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "calculator".into(),
            description: "Evaluates mathematical expressions. Supports +, -, *, /, ^, parentheses, and common math functions.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "expression": {
                        "type": "string",
                        "description": "The mathematical expression to evaluate (e.g., '2 * (3 + 4)')"
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
            .ok_or_else(|| AyasError::Tool(ToolError::InvalidInput("missing 'expression' field".into())))?;

        let result: f64 = meval::eval_str(expr)
            .map_err(|e| AyasError::Tool(ToolError::ExecutionFailed(format!("Failed to evaluate '{expr}': {e}"))))?;

        // Format: remove trailing zeros for clean output
        if result == result.floor() && result.abs() < 1e15 {
            Ok(format!("{}", result as i64))
        } else {
            Ok(result.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definition_has_correct_schema() {
        let tool = CalculatorTool;
        let def = tool.definition();
        assert_eq!(def.name, "calculator");
        assert!(def.description.contains("mathematical"));
        assert!(def.parameters.get("properties").is_some());
        assert!(def.parameters["required"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("expression")));
    }

    #[tokio::test]
    async fn eval_simple_add() {
        let tool = CalculatorTool;
        let result = tool
            .call(serde_json::json!({"expression": "2 + 3"}))
            .await
            .unwrap();
        assert_eq!(result, "5");
    }

    #[tokio::test]
    async fn eval_complex() {
        let tool = CalculatorTool;
        let result = tool
            .call(serde_json::json!({"expression": "2 * (3 + 4)"}))
            .await
            .unwrap();
        assert_eq!(result, "14");
    }

    #[tokio::test]
    async fn eval_decimal() {
        let tool = CalculatorTool;
        let result = tool
            .call(serde_json::json!({"expression": "1.5 + 2.5"}))
            .await
            .unwrap();
        assert_eq!(result, "4");
    }

    #[tokio::test]
    async fn eval_invalid_expr() {
        let tool = CalculatorTool;
        let result = tool
            .call(serde_json::json!({"expression": "abc"}))
            .await;
        assert!(result.is_err());
    }
}
