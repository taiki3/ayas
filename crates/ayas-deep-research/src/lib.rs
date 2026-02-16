pub mod client;
pub mod file_search;
pub mod gemini;
pub mod mock;
pub mod runnable;
pub mod types;

/// Prelude module for convenient imports.
pub mod prelude {
    pub use crate::client::InteractionsClient;
    pub use crate::file_search::{FileSearchClient, GeminiFileSearchClient, MockFileSearchClient};
    pub use crate::gemini::GeminiInteractionsClient;
    pub use crate::mock::MockInteractionsClient;
    pub use crate::runnable::{DeepResearchInput, DeepResearchOutput, DeepResearchRunnable};
    pub use crate::types::{
        AgentConfig, ContentPart, CreateInteractionRequest, FileSearchStore, Interaction,
        InteractionInput, InteractionOutput, InteractionStatus, Operation, OperationError,
        StreamDelta, StreamEvent, StreamEventType, ToolConfig, UploadedFile,
    };
}
