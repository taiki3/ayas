pub mod client;
pub mod gemini;
pub mod mock;
pub mod runnable;
pub mod types;

/// Prelude module for convenient imports.
pub mod prelude {
    pub use crate::client::InteractionsClient;
    pub use crate::gemini::GeminiInteractionsClient;
    pub use crate::mock::MockInteractionsClient;
    pub use crate::runnable::{DeepResearchInput, DeepResearchOutput, DeepResearchRunnable};
    pub use crate::types::{
        AgentConfig, ContentPart, CreateInteractionRequest, Interaction, InteractionInput,
        InteractionOutput, InteractionStatus, StreamDelta, StreamEvent, StreamEventType,
        ToolConfig,
    };
}
