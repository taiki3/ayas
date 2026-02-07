use rhai::{Engine, Scope};
use serde_json::Value;

use crate::error::AdlError;

/// Evaluate an ADL condition expression against a JSON state.
///
/// - `"default"` is treated as always-true (fallback condition).
/// - Other expressions are evaluated using the Rhai scripting engine
///   with the state injected as a `state` variable in scope.
pub fn evaluate(expression: &str, state: &Value) -> Result<bool, AdlError> {
    if expression == "default" {
        return Ok(true);
    }

    let engine = create_sandboxed_engine();

    // Convert serde_json::Value -> Rhai Dynamic
    let dynamic_state = rhai::serde::to_dynamic(state).map_err(|e| AdlError::ExpressionError {
        from: expression.to_string(),
        detail: format!("Failed to convert state to Rhai dynamic: {e}"),
    })?;

    let mut scope = Scope::new();
    scope.push_dynamic("state", dynamic_state);

    let result: bool = engine
        .eval_with_scope(&mut scope, expression)
        .map_err(|e| AdlError::ExpressionError {
            from: expression.to_string(),
            detail: e.to_string(),
        })?;

    Ok(result)
}

/// Create a sandboxed Rhai engine with safety limits.
fn create_sandboxed_engine() -> Engine {
    let mut engine = Engine::new();
    engine.set_max_operations(10_000);
    engine.set_max_call_levels(8);
    engine.set_max_expr_depths(32, 16);
    engine.set_max_string_size(4096);
    engine.set_max_array_size(256);
    engine.set_max_map_size(128);
    engine
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn default_expression_always_true() {
        let state = json!({"anything": true});
        assert!(evaluate("default", &state).unwrap());
    }

    #[test]
    fn default_expression_with_empty_state() {
        let state = json!({});
        assert!(evaluate("default", &state).unwrap());
    }

    #[test]
    fn string_comparison() {
        let state = json!({"choice": "a"});
        assert!(evaluate(r#"state.choice == "a""#, &state).unwrap());
        assert!(!evaluate(r#"state.choice == "b""#, &state).unwrap());
    }

    #[test]
    fn numeric_comparison() {
        let state = json!({"score": 85});
        assert!(evaluate("state.score > 50", &state).unwrap());
        assert!(!evaluate("state.score > 90", &state).unwrap());
        assert!(evaluate("state.score >= 85", &state).unwrap());
    }

    #[test]
    fn boolean_expression() {
        let state = json!({"ready": true, "count": 3});
        assert!(evaluate("state.ready && state.count > 2", &state).unwrap());
        assert!(!evaluate("state.ready && state.count > 5", &state).unwrap());
    }

    #[test]
    fn nested_access() {
        let state = json!({"data": {"status": "complete"}});
        assert!(evaluate(r#"state.data.status == "complete""#, &state).unwrap());
    }

    #[test]
    fn invalid_expression_returns_error() {
        let state = json!({});
        let result = evaluate("this is not valid rhai", &state);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, AdlError::ExpressionError { .. }));
    }

    #[test]
    fn expression_returning_non_bool_is_error() {
        let state = json!({"x": 1});
        // Expression returns an integer, not a bool
        let result = evaluate("state.x + 1", &state);
        assert!(result.is_err());
    }
}
