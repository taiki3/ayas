use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use ayas_core::config::RunnableConfig;
use ayas_core::error::Result;
use serde_json::Value;

type AsyncNodeFn =
    dyn Fn(Value, RunnableConfig) -> Pin<Box<dyn Future<Output = Result<Value>> + Send>>
        + Send
        + Sync;

/// A graph node that wraps an async function operating on JSON state.
///
/// Follows the same `Arc<dyn Fn>` pattern as `RunnableLambda`.
pub struct NodeFn {
    name: String,
    func: Arc<AsyncNodeFn>,
}

impl NodeFn {
    /// Create a new node with the given name and async function.
    pub fn new<F, Fut>(name: impl Into<String>, func: F) -> Self
    where
        F: Fn(Value, RunnableConfig) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Value>> + Send + 'static,
    {
        Self {
            name: name.into(),
            func: Arc::new(move |input, config| Box::pin(func(input, config))),
        }
    }

    /// Get the name of this node.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Invoke this node with the given state and config.
    pub async fn invoke(&self, state: Value, config: &RunnableConfig) -> Result<Value> {
        (self.func)(state, config.clone()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ayas_core::error::AyasError;
    use serde_json::json;

    #[tokio::test]
    async fn node_basic_invoke() {
        let node = NodeFn::new("add_key", |mut state: Value, _config| async move {
            state["added"] = json!(true);
            Ok(state)
        });

        let config = RunnableConfig::default();
        let result = node.invoke(json!({"x": 1}), &config).await.unwrap();
        assert_eq!(result, json!({"x": 1, "added": true}));
    }

    #[test]
    fn node_name_accessor() {
        let node = NodeFn::new("my_node", |state: Value, _config| async move { Ok(state) });
        assert_eq!(node.name(), "my_node");
    }

    #[tokio::test]
    async fn node_error_propagation() {
        let node = NodeFn::new("fail_node", |_state: Value, _config| async move {
            Err(AyasError::Other("node failed".into()))
        });

        let config = RunnableConfig::default();
        let result = node.invoke(json!({}), &config).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("node failed"));
    }
}
