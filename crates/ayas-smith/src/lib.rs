pub mod client;
#[cfg(feature = "clickhouse")]
pub mod clickhouse_store;
pub mod context;
pub mod duckdb_store;
pub mod error;
#[cfg(feature = "postgres")]
pub mod postgres_store;
pub mod query;
pub mod retry;
pub mod store;
pub mod traced;
pub mod tracing_layer;
pub mod types;
pub mod writer;

/// Prelude for convenient imports.
pub mod prelude {
    #[cfg(feature = "clickhouse")]
    pub use crate::clickhouse_store::ClickHouseStore;
    pub use crate::client::{RunGuard, SmithClient, SmithConfig};
    pub use crate::context::{child_config, trace_context};
    pub use crate::duckdb_store::DuckDbStore;
    pub use crate::error::SmithError;
    #[cfg(feature = "postgres")]
    pub use crate::postgres_store::PostgresSmithStore;
    pub use crate::query::SmithQuery;
    pub use crate::retry::with_retry;
    pub use crate::store::SmithStore;
    pub use crate::traced::{
        traced_model, traced_tool, TracedChatModel, TracedRunnable, TracedTool, TraceExt,
    };
    pub use crate::tracing_layer::SmithLayer;
    pub use crate::types::{
        Dataset, Example, Feedback, FeedbackFilter, LatencyStats, Project, Run, RunFilter,
        RunPatch, RunStatus, RunType, TokenUsageSummary,
    };
}
