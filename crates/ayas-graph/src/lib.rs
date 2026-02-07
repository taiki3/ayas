pub mod channel;
pub mod compiled;
pub mod constants;
pub mod edge;
pub mod node;
pub mod state_graph;

/// Prelude module for convenient imports.
pub mod prelude {
    pub use crate::channel::{AppendChannel, Channel, LastValue};
    pub use crate::compiled::CompiledStateGraph;
    pub use crate::constants::{END, START};
    pub use crate::edge::{ConditionalEdge, Edge};
    pub use crate::node::NodeFn;
    pub use crate::state_graph::StateGraph;
}
