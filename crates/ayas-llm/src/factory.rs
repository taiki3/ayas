use ayas_core::model::ChatModel;

use crate::claude::ClaudeChatModel;
use crate::gemini::GeminiChatModel;
use crate::openai::OpenAIChatModel;
use crate::provider::Provider;

/// Create a ChatModel instance for the given provider.
pub fn create_chat_model(
    provider: &Provider,
    api_key: String,
    model_id: String,
) -> Box<dyn ChatModel> {
    match provider {
        Provider::Gemini => Box::new(GeminiChatModel::new(api_key, model_id)),
        Provider::Claude => Box::new(ClaudeChatModel::new(api_key, model_id)),
        Provider::OpenAI => Box::new(OpenAIChatModel::new(api_key, model_id)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_gemini_model() {
        let model = create_chat_model(&Provider::Gemini, "key".into(), "gemini-2.0-flash".into());
        assert_eq!(model.model_name(), "gemini-2.0-flash");
    }

    #[test]
    fn create_claude_model() {
        let model = create_chat_model(
            &Provider::Claude,
            "key".into(),
            "claude-sonnet-4-5-20250929".into(),
        );
        assert_eq!(model.model_name(), "claude-sonnet-4-5-20250929");
    }

    #[test]
    fn create_openai_model() {
        let model = create_chat_model(&Provider::OpenAI, "key".into(), "gpt-4o-mini".into());
        assert_eq!(model.model_name(), "gpt-4o-mini");
    }
}
