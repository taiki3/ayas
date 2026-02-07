pub mod client;
pub mod context;
pub mod error;
pub mod query;
pub mod traced;
pub mod types;
pub mod writer;

/// Prelude for convenient imports.
pub mod prelude {
    pub use crate::client::{SmithClient, SmithConfig};
    pub use crate::context::{child_config, trace_context};
    pub use crate::error::SmithError;
    pub use crate::query::SmithQuery;
    pub use crate::traced::{
        traced_model, traced_tool, TracedChatModel, TracedRunnable, TracedTool, TraceExt,
    };
    pub use crate::types::{
        LatencyStats, Run, RunFilter, RunStatus, RunType, TokenUsageSummary,
    };
}
