pub mod map_reduce;
pub mod react;
pub mod supervisor;
pub mod tool_calling;

/// Prelude module for convenient imports.
pub mod prelude {
    pub use crate::map_reduce::create_map_reduce_graph;
    pub use crate::react::create_react_agent;
    pub use crate::supervisor::{create_supervisor_agent, WorkerConfig};
    pub use crate::tool_calling::create_tool_calling_agent;
}
