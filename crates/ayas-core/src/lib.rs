pub mod config;
pub mod error;
pub mod message;
pub mod model;
pub mod runnable;
pub mod tool;

/// Prelude module for convenient imports.
pub mod prelude {
    pub use crate::config::RunnableConfig;
    pub use crate::error::{AyasError, Result};
    pub use crate::message::{ContentPart, ContentSource, Message, MessageContent, ToolCall};
    pub use crate::model::{CallOptions, ChatModel, ChatResult};
    pub use crate::runnable::{Runnable, RunnableExt};
    pub use crate::tool::{Tool, ToolDefinition};
}
