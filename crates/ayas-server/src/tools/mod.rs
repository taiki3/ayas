pub mod calculator;
pub mod datetime;
pub mod web_search;

use ayas_core::tool::{Tool, ToolDefinition};

/// Collection of built-in tools available in the playground.
pub struct BuiltinTools;

impl BuiltinTools {
    /// Get all built-in tool instances.
    pub fn all() -> Vec<Box<dyn Tool>> {
        vec![
            Box::new(calculator::CalculatorTool),
            Box::new(datetime::DateTimeTool),
            Box::new(web_search::WebSearchTool),
        ]
    }

    /// Get tool definitions for all built-in tools.
    pub fn definitions() -> Vec<ToolDefinition> {
        Self::all().iter().map(|t| t.definition()).collect()
    }
}

/// Build a list of tools based on enabled tool names.
pub fn build_tools(enabled: &[String], custom: Vec<Box<dyn Tool>>) -> Vec<Box<dyn Tool>> {
    let mut tools: Vec<Box<dyn Tool>> = BuiltinTools::all()
        .into_iter()
        .filter(|t| enabled.contains(&t.definition().name))
        .collect();
    tools.extend(custom);
    tools
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_tools_has_three() {
        let tools = BuiltinTools::all();
        assert_eq!(tools.len(), 3);
    }

    #[test]
    fn build_tools_filters_enabled() {
        let enabled = vec!["calculator".to_string(), "datetime".to_string()];
        let tools = build_tools(&enabled, Vec::new());
        assert_eq!(tools.len(), 2);
        let names: Vec<String> = tools.iter().map(|t| t.definition().name).collect();
        assert!(names.contains(&"calculator".to_string()));
        assert!(names.contains(&"datetime".to_string()));
    }

    #[test]
    fn build_tools_includes_custom() {
        let enabled = vec!["calculator".to_string()];
        let custom: Vec<Box<dyn Tool>> = vec![Box::new(datetime::DateTimeTool)];
        let tools = build_tools(&enabled, custom);
        assert_eq!(tools.len(), 2); // 1 builtin + 1 custom
    }
}
