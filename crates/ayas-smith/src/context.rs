use ayas_core::config::RunnableConfig;
use uuid::Uuid;

const TRACE_ID_KEY: &str = "__smith_trace_id";
const PARENT_RUN_ID_KEY: &str = "__smith_parent_run_id";

/// Extract the trace context from a RunnableConfig.
///
/// Returns `(trace_id, parent_run_id)`. If no trace context exists,
/// uses the config's `run_id` as the root trace_id with no parent.
pub fn trace_context(config: &RunnableConfig) -> (Uuid, Option<Uuid>) {
    let trace_id = config
        .configurable
        .get(TRACE_ID_KEY)
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<Uuid>().ok())
        .unwrap_or(config.run_id);

    let parent_run_id = config
        .configurable
        .get(PARENT_RUN_ID_KEY)
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<Uuid>().ok());

    (trace_id, parent_run_id)
}

/// Create a child RunnableConfig that propagates the trace context.
///
/// The child gets a new `run_id` and the parent context is set to `current_run_id`.
pub fn child_config(config: &RunnableConfig, current_run_id: Uuid, trace_id: Uuid) -> RunnableConfig {
    let mut child = config.clone();
    child.run_id = Uuid::new_v4();
    child.configurable.insert(
        TRACE_ID_KEY.into(),
        serde_json::Value::String(trace_id.to_string()),
    );
    child.configurable.insert(
        PARENT_RUN_ID_KEY.into(),
        serde_json::Value::String(current_run_id.to_string()),
    );
    child
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_context_default_config() {
        let config = RunnableConfig::default();
        let (trace_id, parent) = trace_context(&config);
        assert_eq!(trace_id, config.run_id);
        assert!(parent.is_none());
    }

    #[test]
    fn trace_context_with_existing_context() {
        let tid = Uuid::new_v4();
        let pid = Uuid::new_v4();
        let mut config = RunnableConfig::default();
        config.configurable.insert(
            TRACE_ID_KEY.into(),
            serde_json::Value::String(tid.to_string()),
        );
        config.configurable.insert(
            PARENT_RUN_ID_KEY.into(),
            serde_json::Value::String(pid.to_string()),
        );

        let (trace_id, parent) = trace_context(&config);
        assert_eq!(trace_id, tid);
        assert_eq!(parent, Some(pid));
    }

    #[test]
    fn child_config_propagates_trace() {
        let config = RunnableConfig::default();
        let current_run_id = Uuid::new_v4();
        let trace_id = Uuid::new_v4();

        let child = child_config(&config, current_run_id, trace_id);

        assert_ne!(child.run_id, config.run_id);

        let (child_trace, child_parent) = trace_context(&child);
        assert_eq!(child_trace, trace_id);
        assert_eq!(child_parent, Some(current_run_id));
    }

    #[test]
    fn child_config_preserves_tags_and_metadata() {
        let config = RunnableConfig::new()
            .with_tag("test")
            .with_metadata("key", serde_json::json!("value"));

        let child = child_config(&config, Uuid::new_v4(), Uuid::new_v4());

        assert_eq!(child.tags, vec!["test"]);
        assert_eq!(child.metadata["key"], serde_json::json!("value"));
    }

    #[test]
    fn child_config_preserves_recursion_limit() {
        let config = RunnableConfig::new().with_recursion_limit(50);
        let child = child_config(&config, Uuid::new_v4(), Uuid::new_v4());
        assert_eq!(child.recursion_limit, 50);
    }

    #[test]
    fn nested_child_configs() {
        let root_config = RunnableConfig::default();
        let root_run_id = Uuid::new_v4();
        let trace_id = root_config.run_id;

        // First child
        let child1 = child_config(&root_config, root_run_id, trace_id);
        let child1_run_id = Uuid::new_v4();

        // Second child (grandchild of root)
        let child2 = child_config(&child1, child1_run_id, trace_id);

        let (c2_trace, c2_parent) = trace_context(&child2);
        assert_eq!(c2_trace, trace_id);
        assert_eq!(c2_parent, Some(child1_run_id));
    }
}
