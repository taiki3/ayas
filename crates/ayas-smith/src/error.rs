use thiserror::Error;

/// Smith-specific error type for tracing and observability operations.
#[derive(Debug, Error)]
pub enum SmithError {
    #[error("Parquet write error: {0}")]
    ParquetWrite(String),

    #[error("Arrow error: {0}")]
    Arrow(#[from] arrow::error::ArrowError),

    #[error("Parquet error: {0}")]
    Parquet(#[from] parquet::errors::ParquetError),

    #[error("DuckDB error: {0}")]
    DuckDb(#[from] duckdb::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Query error: {0}")]
    Query(String),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

impl From<SmithError> for ayas_core::error::AyasError {
    fn from(e: SmithError) -> Self {
        ayas_core::error::AyasError::Other(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, SmithError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parquet_write_error_display() {
        let err = SmithError::ParquetWrite("failed to write batch".into());
        assert_eq!(err.to_string(), "Parquet write error: failed to write batch");
    }

    #[test]
    fn io_error_display() {
        let err = SmithError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "file not found",
        ));
        assert!(err.to_string().contains("file not found"));
    }

    #[test]
    fn query_error_display() {
        let err = SmithError::Query("invalid SQL".into());
        assert_eq!(err.to_string(), "Query error: invalid SQL");
    }

    #[test]
    fn serialization_error_display() {
        let err: SmithError = serde_json::from_str::<serde_json::Value>("invalid")
            .unwrap_err()
            .into();
        assert!(err.to_string().contains("Serialization error"));
    }

    #[test]
    fn smith_error_to_ayas_error() {
        let smith_err = SmithError::ParquetWrite("disk full".into());
        let ayas_err: ayas_core::error::AyasError = smith_err.into();
        assert!(matches!(ayas_err, ayas_core::error::AyasError::Other(_)));
        assert!(ayas_err.to_string().contains("disk full"));
    }
}
