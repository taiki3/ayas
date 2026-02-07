use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Configuration passed through the Runnable chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunnableConfig {
    /// Tags for filtering and categorization.
    #[serde(default)]
    pub tags: Vec<String>,

    /// Arbitrary metadata key-value pairs.
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,

    /// Maximum recursion depth for graph execution.
    pub recursion_limit: usize,

    /// Unique identifier for this run.
    pub run_id: Uuid,

    /// Arbitrary configurable values accessible by runnables.
    #[serde(default)]
    pub configurable: HashMap<String, serde_json::Value>,
}

impl Default for RunnableConfig {
    fn default() -> Self {
        Self {
            tags: Vec::new(),
            metadata: HashMap::new(),
            recursion_limit: 25,
            run_id: Uuid::new_v4(),
            configurable: HashMap::new(),
        }
    }
}

impl RunnableConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }

    pub fn with_metadata(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.metadata.insert(key.into(), value);
        self
    }

    pub fn with_recursion_limit(mut self, limit: usize) -> Self {
        self.recursion_limit = limit;
        self
    }

    pub fn with_run_id(mut self, run_id: Uuid) -> Self {
        self.run_id = run_id;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let config = RunnableConfig::default();
        assert!(config.tags.is_empty());
        assert!(config.metadata.is_empty());
        assert_eq!(config.recursion_limit, 25);
        assert!(config.configurable.is_empty());
    }

    #[test]
    fn builder_methods() {
        let config = RunnableConfig::new()
            .with_tag("test")
            .with_tag("debug")
            .with_metadata("key", serde_json::json!("value"))
            .with_recursion_limit(50);

        assert_eq!(config.tags, vec!["test", "debug"]);
        assert_eq!(config.metadata["key"], serde_json::json!("value"));
        assert_eq!(config.recursion_limit, 50);
    }

    #[test]
    fn clone_independence() {
        let config1 = RunnableConfig::new().with_tag("original");
        let mut config2 = config1.clone();
        config2.tags.push("cloned".into());

        assert_eq!(config1.tags.len(), 1);
        assert_eq!(config2.tags.len(), 2);
    }

    #[test]
    fn run_id_uniqueness() {
        let config1 = RunnableConfig::new();
        let config2 = RunnableConfig::new();
        assert_ne!(config1.run_id, config2.run_id);
    }

    #[test]
    fn with_explicit_run_id() {
        let id = Uuid::new_v4();
        let config = RunnableConfig::new().with_run_id(id);
        assert_eq!(config.run_id, id);
    }

    #[test]
    fn serde_roundtrip() {
        let config = RunnableConfig::new()
            .with_tag("test")
            .with_metadata("foo", serde_json::json!(42));
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: RunnableConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.tags, config.tags);
        assert_eq!(deserialized.metadata, config.metadata);
        assert_eq!(deserialized.recursion_limit, config.recursion_limit);
        assert_eq!(deserialized.run_id, config.run_id);
    }
}
