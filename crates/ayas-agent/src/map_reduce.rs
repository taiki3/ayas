use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use ayas_checkpoint::prelude::{send_output, SendDirective};
use ayas_core::config::RunnableConfig;
use ayas_core::error::Result;
use ayas_graph::compiled::CompiledStateGraph;
use ayas_graph::constants::END;
use ayas_graph::edge::ConditionalEdge;
use ayas_graph::node::NodeFn;
use ayas_graph::state_graph::StateGraph;
use serde_json::{json, Value};

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// Create a map-reduce graph that processes a list of items in parallel.
///
/// The graph applies `map_fn` to each item in the input `items` array
/// concurrently (via the Send API), then gathers all results and applies
/// `reduce_fn` to produce a single output value.
///
/// # State schema
/// - `items`: input array to process (consumed by scatter node)
/// - `results`: `AppendChannel` — collects map outputs
/// - `output`: `LastValue` — final reduced result
///
/// # Input
/// ```json
/// { "items": [item1, item2, ...] }
/// ```
///
/// # Output
/// ```json
/// { "items": [...], "results": [...mapped...], "output": reduced_value }
/// ```
///
/// # Example
/// ```ignore
/// let graph = create_map_reduce_graph(
///     |item| Box::pin(async move { Ok(json!(item.as_i64().unwrap() * 2)) }),
///     |results| Box::pin(async move {
///         let sum: f64 = results.iter().filter_map(|v| v.as_f64()).sum();
///         Ok(json!(sum))
///     }),
/// )?;
/// let result = graph.invoke(json!({"items": [1, 2, 3]}), &config).await?;
/// assert_eq!(result["output"], json!(12.0));
/// ```
pub fn create_map_reduce_graph<F, R>(map_fn: F, reduce_fn: R) -> Result<CompiledStateGraph>
where
    F: Fn(Value) -> BoxFuture<Result<Value>> + Send + Sync + 'static + Clone,
    R: Fn(Vec<Value>) -> BoxFuture<Result<Value>> + Send + Sync + 'static,
{
    let mut graph = StateGraph::new();
    graph.add_append_channel("items");
    graph.add_append_channel("results");
    graph.add_last_value_channel("output", Value::Null);

    // Scatter node: reads `items` and emits a Send for each item to `map_node`
    graph.add_node(NodeFn::new(
        "scatter",
        |state: Value, _cfg: RunnableConfig| async move {
            let items = match state.get("items") {
                Some(Value::Array(arr)) => arr.clone(),
                _ => Vec::new(),
            };

            let sends: Vec<SendDirective> = items
                .into_iter()
                .map(|item| SendDirective::new("map_node", json!({ "__map_item__": item })))
                .collect();

            Ok(send_output(sends))
        },
    ))?;

    // Map node: applies the user-provided map_fn to each item
    let map_fn = Arc::new(map_fn);
    graph.add_node(NodeFn::new("map_node", move |state: Value, _cfg| {
        let map_fn = map_fn.clone();
        async move {
            let item = state
                .get("__map_item__")
                .cloned()
                .unwrap_or(Value::Null);
            let result = map_fn(item).await?;
            Ok(json!({"results": result}))
        }
    }))?;

    // Reduce node: applies the user-provided reduce_fn to the gathered results
    let reduce_fn = Arc::new(reduce_fn);
    graph.add_node(NodeFn::new("reduce", move |state: Value, _cfg| {
        let reduce_fn = reduce_fn.clone();
        async move {
            let results = match state.get("results") {
                Some(Value::Array(arr)) => arr.clone(),
                _ => Vec::new(),
            };
            let output = reduce_fn(results).await?;
            Ok(json!({"output": output}))
        }
    }))?;

    graph.set_entry_point("scatter");
    graph.add_edge("scatter", "reduce");
    // map_node is reachable via Send directive (need conditional edge for validation)
    graph.add_conditional_edges(ConditionalEdge::new(
        "reduce",
        |_: &Value| END.to_string(),
        None,
    ));

    graph.compile()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ayas_core::runnable::Runnable;

    #[tokio::test]
    async fn test_map_reduce_double_and_sum() {
        let graph = create_map_reduce_graph(
            |item| {
                Box::pin(async move {
                    let n = item.as_i64().unwrap_or(0);
                    Ok(json!(n * 2))
                })
            },
            |results| {
                Box::pin(async move {
                    let sum: i64 = results.iter().filter_map(|v| v.as_i64()).sum();
                    Ok(json!(sum))
                })
            },
        )
        .unwrap();

        let config = RunnableConfig::default();
        let result = graph
            .invoke(json!({"items": [1, 2, 3, 4, 5]}), &config)
            .await
            .unwrap();

        // map: [2, 4, 6, 8, 10], reduce: sum = 30
        assert_eq!(result["output"], json!(30));
    }

    #[tokio::test]
    async fn test_map_reduce_string_concat() {
        let graph = create_map_reduce_graph(
            |item| {
                Box::pin(async move {
                    let s = item.as_str().unwrap_or("").to_uppercase();
                    Ok(json!(s))
                })
            },
            |results| {
                Box::pin(async move {
                    let joined: String = results
                        .iter()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join(", ");
                    Ok(json!(joined))
                })
            },
        )
        .unwrap();

        let config = RunnableConfig::default();
        let result = graph
            .invoke(json!({"items": ["hello", "world"]}), &config)
            .await
            .unwrap();

        assert_eq!(result["output"], json!("HELLO, WORLD"));
    }

    #[tokio::test]
    async fn test_map_reduce_empty_items() {
        let graph = create_map_reduce_graph(
            |item| Box::pin(async move { Ok(item) }),
            |results| Box::pin(async move { Ok(json!(results.len())) }),
        )
        .unwrap();

        let config = RunnableConfig::default();
        let result = graph
            .invoke(json!({"items": []}), &config)
            .await
            .unwrap();

        assert_eq!(result["output"], json!(0));
    }

    #[tokio::test]
    async fn test_map_reduce_parallel_timing() {
        // 5 items each taking 50ms → should complete well under 250ms if parallel
        let graph = create_map_reduce_graph(
            |item| {
                Box::pin(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    let n = item.as_i64().unwrap_or(0);
                    Ok(json!(n * 10))
                })
            },
            |results| {
                Box::pin(async move {
                    let sum: i64 = results.iter().filter_map(|v| v.as_i64()).sum();
                    Ok(json!(sum))
                })
            },
        )
        .unwrap();

        let config = RunnableConfig::default();
        let start = std::time::Instant::now();
        let result = graph
            .invoke(json!({"items": [1, 2, 3, 4, 5]}), &config)
            .await
            .unwrap();
        let elapsed = start.elapsed();

        assert_eq!(result["output"], json!(150));
        assert!(
            elapsed.as_millis() < 250,
            "Expected parallel map under 250ms, took {}ms",
            elapsed.as_millis()
        );
    }
}
