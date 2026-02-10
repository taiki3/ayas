pub mod channel;
pub mod compiled;
pub mod constants;
pub mod edge;
pub mod node;
pub mod state_graph;
pub mod stream;
pub mod subgraph;

/// Prelude module for convenient imports.
pub mod prelude {
    pub use ayas_checkpoint::prelude::GraphOutput;

    pub use crate::channel::{
        AggregateOp, AppendChannel, BinaryOperatorAggregate, Channel, ChannelSpec, EphemeralValue,
        LastValue, TopicChannel,
    };
    pub use crate::compiled::{CompiledStateGraph, StepInfo};
    pub use crate::constants::{END, START};
    pub use crate::edge::{ConditionalEdge, Edge};
    pub use crate::node::NodeFn;
    pub use crate::state_graph::StateGraph;
    pub use crate::stream::StreamEvent;
    pub use crate::subgraph::subgraph_node;
}
