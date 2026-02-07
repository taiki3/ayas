use thiserror::Error;

/// ADL-specific error type.
#[derive(Debug, Error)]
pub enum AdlError {
    #[error("ADL parse error: {0}")]
    Parse(String),

    #[error("ADL validation error: {0}")]
    Validation(String),

    #[error("Unknown node type: '{node_type}'")]
    UnknownNodeType { node_type: String },

    #[error("Missing config field '{field}' for node type '{node_type}'")]
    MissingConfig { node_type: String, field: String },

    #[error("Expression error in '{from}': {detail}")]
    ExpressionError { from: String, detail: String },

    #[error("YAML error: {0}")]
    Yaml(#[from] serde_yaml::Error),
}

impl From<AdlError> for ayas_core::error::AyasError {
    fn from(e: AdlError) -> Self {
        ayas_core::error::AyasError::Other(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_error_display() {
        let err = AdlError::Parse("unexpected token".into());
        assert_eq!(err.to_string(), "ADL parse error: unexpected token");
    }

    #[test]
    fn validation_error_display() {
        let err = AdlError::Validation("missing entry point".into());
        assert_eq!(
            err.to_string(),
            "ADL validation error: missing entry point"
        );
    }

    #[test]
    fn unknown_node_type_display() {
        let err = AdlError::UnknownNodeType {
            node_type: "custom_llm".into(),
        };
        assert_eq!(err.to_string(), "Unknown node type: 'custom_llm'");
    }

    #[test]
    fn missing_config_display() {
        let err = AdlError::MissingConfig {
            node_type: "transform".into(),
            field: "mapping".into(),
        };
        assert_eq!(
            err.to_string(),
            "Missing config field 'mapping' for node type 'transform'"
        );
    }

    #[test]
    fn expression_error_display() {
        let err = AdlError::ExpressionError {
            from: "router".into(),
            detail: "undefined variable".into(),
        };
        assert_eq!(
            err.to_string(),
            "Expression error in 'router': undefined variable"
        );
    }

    #[test]
    fn adl_error_to_ayas_error() {
        let adl_err = AdlError::Parse("bad yaml".into());
        let ayas_err: ayas_core::error::AyasError = adl_err.into();
        assert!(matches!(ayas_err, ayas_core::error::AyasError::Other(_)));
        assert!(ayas_err.to_string().contains("bad yaml"));
    }
}
