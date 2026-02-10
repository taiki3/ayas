use thiserror::Error;

/// Top-level error type for the Ayas library.
#[derive(Debug, Error)]
pub enum AyasError {
    #[error("Model error: {0}")]
    Model(#[from] ModelError),

    #[error("Tool error: {0}")]
    Tool(#[from] ToolError),

    #[error("Chain error: {0}")]
    Chain(#[from] ChainError),

    #[error("Graph error: {0}")]
    Graph(#[from] GraphError),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("{0}")]
    Other(String),
}

#[derive(Debug, Error)]
pub enum ModelError {
    #[error("API request failed: {0}")]
    ApiRequest(String),

    #[error("Invalid response: {0}")]
    InvalidResponse(String),

    #[error("Authentication failed: {0}")]
    Auth(String),

    #[error("Rate limited: retry after {retry_after_secs:?}s")]
    RateLimited { retry_after_secs: Option<u64> },
}

#[derive(Debug, Error)]
pub enum ToolError {
    #[error("Tool not found: {0}")]
    NotFound(String),

    #[error("Invalid input: {0}")]
    InvalidInput(String),

    #[error("Execution failed: {0}")]
    ExecutionFailed(String),
}

#[derive(Debug, Error)]
pub enum ChainError {
    #[error("Template error: {0}")]
    Template(String),

    #[error("Parse error: {0}")]
    Parse(String),

    #[error("Missing variable: {0}")]
    MissingVariable(String),
}

#[derive(Debug, Error)]
pub enum GraphError {
    #[error("Invalid graph: {0}")]
    InvalidGraph(String),

    #[error("Recursion limit ({limit}) exceeded")]
    RecursionLimit { limit: usize },

    #[error("Channel error: {0}")]
    Channel(String),

    #[error("Node error in '{node}': {source}")]
    NodeExecution {
        node: String,
        source: Box<AyasError>,
    },

    #[error("Checkpoint error: {0}")]
    Checkpoint(String),

    #[error("Thread not found: {0}")]
    ThreadNotFound(String),
}

pub type Result<T> = std::result::Result<T, AyasError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_error_display() {
        let err = ModelError::ApiRequest("timeout".into());
        assert_eq!(err.to_string(), "API request failed: timeout");
    }

    #[test]
    fn model_error_rate_limited_display() {
        let err = ModelError::RateLimited {
            retry_after_secs: Some(30),
        };
        assert_eq!(err.to_string(), "Rate limited: retry after Some(30)s");
    }

    #[test]
    fn tool_error_display() {
        let err = ToolError::NotFound("web_search".into());
        assert_eq!(err.to_string(), "Tool not found: web_search");
    }

    #[test]
    fn chain_error_display() {
        let err = ChainError::MissingVariable("name".into());
        assert_eq!(err.to_string(), "Missing variable: name");
    }

    #[test]
    fn graph_error_display() {
        let err = GraphError::RecursionLimit { limit: 25 };
        assert_eq!(err.to_string(), "Recursion limit (25) exceeded");
    }

    #[test]
    fn ayas_error_from_model_error() {
        let model_err = ModelError::Auth("bad key".into());
        let err: AyasError = model_err.into();
        assert!(matches!(err, AyasError::Model(ModelError::Auth(_))));
        assert!(err.to_string().contains("bad key"));
    }

    #[test]
    fn ayas_error_from_tool_error() {
        let tool_err = ToolError::InvalidInput("expected number".into());
        let err: AyasError = tool_err.into();
        assert!(matches!(err, AyasError::Tool(ToolError::InvalidInput(_))));
    }

    #[test]
    fn ayas_error_from_chain_error() {
        let chain_err = ChainError::Template("unclosed brace".into());
        let err: AyasError = chain_err.into();
        assert!(matches!(err, AyasError::Chain(ChainError::Template(_))));
    }

    #[test]
    fn ayas_error_from_graph_error() {
        let graph_err = GraphError::InvalidGraph("no START node".into());
        let err: AyasError = graph_err.into();
        assert!(matches!(err, AyasError::Graph(GraphError::InvalidGraph(_))));
    }

    #[test]
    fn graph_error_node_execution() {
        let inner = AyasError::Other("something broke".into());
        let err = GraphError::NodeExecution {
            node: "agent".into(),
            source: Box::new(inner),
        };
        assert!(err.to_string().contains("agent"));
    }
}
