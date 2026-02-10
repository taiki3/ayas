use ayas_core::config::RunnableConfig;
use serde_json::{json, Value};

use crate::interrupt::config_keys;

/// Extension trait for RunnableConfig to easily set checkpoint-related values.
pub trait CheckpointConfigExt {
    fn with_thread_id(self, thread_id: impl Into<String>) -> Self;
    fn with_checkpoint_id(self, checkpoint_id: impl Into<String>) -> Self;
    fn with_resume_value(self, value: Value) -> Self;
    fn thread_id(&self) -> Option<String>;
    fn checkpoint_id(&self) -> Option<String>;
    fn resume_value(&self) -> Option<Value>;
}

impl CheckpointConfigExt for RunnableConfig {
    fn with_thread_id(mut self, thread_id: impl Into<String>) -> Self {
        self.configurable
            .insert(config_keys::THREAD_ID.into(), json!(thread_id.into()));
        self
    }

    fn with_checkpoint_id(mut self, checkpoint_id: impl Into<String>) -> Self {
        self.configurable
            .insert(config_keys::CHECKPOINT_ID.into(), json!(checkpoint_id.into()));
        self
    }

    fn with_resume_value(mut self, value: Value) -> Self {
        self.configurable
            .insert(config_keys::RESUME_VALUE.into(), value);
        self
    }

    fn thread_id(&self) -> Option<String> {
        self.configurable
            .get(config_keys::THREAD_ID)
            .and_then(|v| v.as_str())
            .map(String::from)
    }

    fn checkpoint_id(&self) -> Option<String> {
        self.configurable
            .get(config_keys::CHECKPOINT_ID)
            .and_then(|v| v.as_str())
            .map(String::from)
    }

    fn resume_value(&self) -> Option<Value> {
        self.configurable.get(config_keys::RESUME_VALUE).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_and_get_thread_id() {
        let config = RunnableConfig::default().with_thread_id("thread-1");
        assert_eq!(config.thread_id(), Some("thread-1".to_string()));
    }

    #[test]
    fn set_and_get_checkpoint_id() {
        let config = RunnableConfig::default().with_checkpoint_id("cp-42");
        assert_eq!(config.checkpoint_id(), Some("cp-42".to_string()));
    }

    #[test]
    fn set_and_get_resume_value() {
        let value = json!({"approved": true, "comment": "looks good"});
        let config = RunnableConfig::default().with_resume_value(value.clone());
        assert_eq!(config.resume_value(), Some(value));
    }

    #[test]
    fn missing_values_return_none() {
        let config = RunnableConfig::default();
        assert_eq!(config.thread_id(), None);
        assert_eq!(config.checkpoint_id(), None);
        assert_eq!(config.resume_value(), None);
    }

    #[test]
    fn roundtrip_all_fields() {
        let config = RunnableConfig::default()
            .with_thread_id("t-1")
            .with_checkpoint_id("cp-1")
            .with_resume_value(json!("yes"));

        assert_eq!(config.thread_id(), Some("t-1".to_string()));
        assert_eq!(config.checkpoint_id(), Some("cp-1".to_string()));
        assert_eq!(config.resume_value(), Some(json!("yes")));
    }

    #[test]
    fn chaining_preserves_other_config() {
        let config = RunnableConfig::default()
            .with_tag("test")
            .with_thread_id("t-1")
            .with_checkpoint_id("cp-1");

        assert_eq!(config.tags, vec!["test"]);
        assert_eq!(config.thread_id(), Some("t-1".to_string()));
        assert_eq!(config.checkpoint_id(), Some("cp-1".to_string()));
    }

    #[test]
    fn overwrite_thread_id() {
        let config = RunnableConfig::default()
            .with_thread_id("old")
            .with_thread_id("new");
        assert_eq!(config.thread_id(), Some("new".to_string()));
    }

    #[test]
    fn resume_value_with_complex_json() {
        let value = json!({
            "approved": true,
            "edits": [
                {"field": "title", "new_value": "Updated Title"},
                {"field": "body", "new_value": "Updated Body"}
            ]
        });
        let config = RunnableConfig::default().with_resume_value(value.clone());
        assert_eq!(config.resume_value(), Some(value));
    }
}
