use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Gemini,
    Claude,
    OpenAI,
}

impl Provider {
    pub fn default_models(&self) -> &[&str] {
        match self {
            Provider::Gemini => &[
                "gemini-3.0-flash",
                "gemini-3-flash-preview",
                "gemini-3-pro-preview",
                "gemini-2.5-flash",
                "gemini-2.5-pro",
            ],
            Provider::Claude => &[
                "claude-opus-4-6",
                "claude-sonnet-4-5-20250929",
                "claude-haiku-4-5-20251001",
            ],
            Provider::OpenAI => &[
                "gpt-5.3",
                "gpt-5.2",
                "gpt-4.1",
                "gpt-4.1-mini",
                "o4-mini",
                "o3-mini",
            ],
        }
    }
}

pub fn model_map() -> HashMap<Provider, Vec<&'static str>> {
    HashMap::from([
        (Provider::Gemini, Provider::Gemini.default_models().to_vec()),
        (Provider::Claude, Provider::Claude.default_models().to_vec()),
        (Provider::OpenAI, Provider::OpenAI.default_models().to_vec()),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_serialize_gemini() {
        let json = serde_json::to_string(&Provider::Gemini).unwrap();
        assert_eq!(json, "\"gemini\"");
    }

    #[test]
    fn provider_serialize_claude() {
        let json = serde_json::to_string(&Provider::Claude).unwrap();
        assert_eq!(json, "\"claude\"");
    }

    #[test]
    fn provider_serialize_openai() {
        let json = serde_json::to_string(&Provider::OpenAI).unwrap();
        assert_eq!(json, "\"openai\"");
    }

    #[test]
    fn provider_deserialize() {
        let p: Provider = serde_json::from_str("\"gemini\"").unwrap();
        assert_eq!(p, Provider::Gemini);
        let p: Provider = serde_json::from_str("\"claude\"").unwrap();
        assert_eq!(p, Provider::Claude);
        let p: Provider = serde_json::from_str("\"openai\"").unwrap();
        assert_eq!(p, Provider::OpenAI);
    }

    #[test]
    fn model_map_has_all_providers() {
        let map = model_map();
        assert!(map.contains_key(&Provider::Gemini));
        assert!(map.contains_key(&Provider::Claude));
        assert!(map.contains_key(&Provider::OpenAI));
        for (_, models) in &map {
            assert!(!models.is_empty());
        }
    }
}
