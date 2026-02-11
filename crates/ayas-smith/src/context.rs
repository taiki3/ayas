use chrono::{DateTime, Utc};

use ayas_core::config::RunnableConfig;
use uuid::Uuid;

const TRACE_ID_KEY: &str = "__smith_trace_id";
const PARENT_RUN_ID_KEY: &str = "__smith_parent_run_id";
const DOTTED_ORDER_KEY: &str = "__smith_dotted_order";

/// Task-local trace context for propagating trace hierarchy to
/// `TracedChatModel` and `TracedTool` which don't receive `RunnableConfig`.
#[derive(Debug, Clone)]
pub struct SmithTraceCtx {
    pub trace_id: Uuid,
    pub parent_run_id: Option<Uuid>,
    pub dotted_order: String,
}

tokio::task_local! {
    pub static SMITH_TRACE_CTX: SmithTraceCtx;
}

/// Build a dotted_order segment for hierarchical run ordering.
///
/// Format: `{YYYYMMDDTHHMMSS######Z}.{run_id_hex_prefix}`
/// Child format: `{parent_dotted_order}.{segment}`
pub fn build_dotted_order(
    start_time: DateTime<Utc>,
    run_id: Uuid,
    parent_dotted_order: Option<&str>,
) -> String {
    let ts = start_time.format("%Y%m%dT%H%M%S%6fZ");
    let id_hex = run_id.to_string().replace('-', "");
    let segment = format!("{ts}.{}", &id_hex[..8]);

    match parent_dotted_order {
        Some(parent) => format!("{parent}.{segment}"),
        None => segment,
    }
}

/// Extract the trace context from a RunnableConfig.
///
/// Returns `(trace_id, parent_run_id, parent_dotted_order)`.
/// If no trace context exists, uses the config's `run_id` as the root trace_id.
pub fn trace_context(config: &RunnableConfig) -> (Uuid, Option<Uuid>, Option<String>) {
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

    let dotted_order = config
        .configurable
        .get(DOTTED_ORDER_KEY)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    (trace_id, parent_run_id, dotted_order)
}

/// Create a child RunnableConfig that propagates the trace context.
///
/// The child gets a new `run_id` and the parent context is set to `current_run_id`.
pub fn child_config(
    config: &RunnableConfig,
    current_run_id: Uuid,
    trace_id: Uuid,
    dotted_order: &str,
) -> RunnableConfig {
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
    child.configurable.insert(
        DOTTED_ORDER_KEY.into(),
        serde_json::Value::String(dotted_order.to_string()),
    );
    child
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_context_default_config() {
        let config = RunnableConfig::default();
        let (trace_id, parent, dotted_order) = trace_context(&config);
        assert_eq!(trace_id, config.run_id);
        assert!(parent.is_none());
        assert!(dotted_order.is_none());
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

        let (trace_id, parent, _) = trace_context(&config);
        assert_eq!(trace_id, tid);
        assert_eq!(parent, Some(pid));
    }

    #[test]
    fn trace_context_with_dotted_order() {
        let mut config = RunnableConfig::default();
        config.configurable.insert(
            DOTTED_ORDER_KEY.into(),
            serde_json::Value::String("20250210T120000000000Z.abc12345".into()),
        );

        let (_, _, dotted_order) = trace_context(&config);
        assert_eq!(
            dotted_order.as_deref(),
            Some("20250210T120000000000Z.abc12345")
        );
    }

    #[test]
    fn child_config_propagates_trace() {
        let config = RunnableConfig::default();
        let current_run_id = Uuid::new_v4();
        let trace_id = Uuid::new_v4();
        let dotted_order = "20250210T120000000000Z.abc12345";

        let child = child_config(&config, current_run_id, trace_id, dotted_order);

        assert_ne!(child.run_id, config.run_id);

        let (child_trace, child_parent, child_dotted) = trace_context(&child);
        assert_eq!(child_trace, trace_id);
        assert_eq!(child_parent, Some(current_run_id));
        assert_eq!(child_dotted.as_deref(), Some(dotted_order));
    }

    #[test]
    fn child_config_preserves_tags_and_metadata() {
        let config = RunnableConfig::new()
            .with_tag("test")
            .with_metadata("key", serde_json::json!("value"));

        let child = child_config(&config, Uuid::new_v4(), Uuid::new_v4(), "root.child");

        assert_eq!(child.tags, vec!["test"]);
        assert_eq!(child.metadata["key"], serde_json::json!("value"));
    }

    #[test]
    fn child_config_preserves_recursion_limit() {
        let config = RunnableConfig::new().with_recursion_limit(50);
        let child = child_config(&config, Uuid::new_v4(), Uuid::new_v4(), "root.child");
        assert_eq!(child.recursion_limit, 50);
    }

    #[test]
    fn nested_child_configs() {
        let root_config = RunnableConfig::default();
        let root_run_id = Uuid::new_v4();
        let trace_id = root_config.run_id;

        let child1 = child_config(&root_config, root_run_id, trace_id, "root");
        let child1_run_id = Uuid::new_v4();

        let child2 = child_config(&child1, child1_run_id, trace_id, "root.child1");

        let (c2_trace, c2_parent, c2_dotted) = trace_context(&child2);
        assert_eq!(c2_trace, trace_id);
        assert_eq!(c2_parent, Some(child1_run_id));
        assert_eq!(c2_dotted.as_deref(), Some("root.child1"));
    }

    #[test]
    fn build_dotted_order_root() {
        let time = Utc::now();
        let id = Uuid::new_v4();
        let order = build_dotted_order(time, id, None);
        // Format: {timestamp}.{prefix}  — exactly one dot
        assert_eq!(order.matches('.').count(), 1);
        assert!(order.ends_with(&id.to_string().replace('-', "")[..8]));
    }

    #[test]
    fn build_dotted_order_child() {
        let time = Utc::now();
        let id = Uuid::new_v4();
        let parent = "20250210T120000000000Z.abc12345";
        let order = build_dotted_order(time, id, Some(parent));
        assert!(order.starts_with(parent));
        // parent has 1 dot, child adds 1 more for the segment separator + 1 inside = 3 total
        assert_eq!(order.matches('.').count(), 3);
    }

    #[test]
    fn build_dotted_order_grandchild() {
        let time1 = Utc::now();
        let id1 = Uuid::new_v4();
        let root = build_dotted_order(time1, id1, None);

        let time2 = Utc::now();
        let id2 = Uuid::new_v4();
        let child = build_dotted_order(time2, id2, Some(&root));

        let time3 = Utc::now();
        let id3 = Uuid::new_v4();
        let grandchild = build_dotted_order(time3, id3, Some(&child));

        assert!(grandchild.starts_with(&child));
        assert!(child.starts_with(&root));
        // 5 dots: root(1) + child_sep(1) + child_internal(1) + grandchild_sep(1) + grandchild_internal(1)
        assert_eq!(grandchild.matches('.').count(), 5);
    }

    #[tokio::test]
    async fn task_local_trace_context_propagation() {
        let trace_id = Uuid::new_v4();
        let parent_id = Uuid::new_v4();
        let ctx = SmithTraceCtx {
            trace_id,
            parent_run_id: Some(parent_id),
            dotted_order: "20250210T120000000000Z.abc12345".into(),
        };

        let result = SMITH_TRACE_CTX
            .scope(ctx, async {
                SMITH_TRACE_CTX
                    .try_with(|c| {
                        assert_eq!(c.trace_id, trace_id);
                        assert_eq!(c.parent_run_id, Some(parent_id));
                    })
                    .unwrap();
                42
            })
            .await;

        assert_eq!(result, 42);
    }

    #[tokio::test]
    async fn task_local_not_set_outside_scope() {
        let result = SMITH_TRACE_CTX.try_with(|_| ());
        assert!(result.is_err());
    }
}
