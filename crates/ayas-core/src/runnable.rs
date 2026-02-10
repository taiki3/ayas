use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use serde_json::Value;

use crate::config::RunnableConfig;
use crate::error::Result;

/// Core abstraction for composable, async computation units.
///
/// Every component in the Ayas pipeline (prompts, models, parsers, tools, graphs)
/// implements this trait. Components can be composed using `.pipe()`.
#[async_trait]
pub trait Runnable: Send + Sync {
    type Input: Send + 'static;
    type Output: Send + 'static;

    /// Process a single input and return a result.
    async fn invoke(&self, input: Self::Input, config: &RunnableConfig) -> Result<Self::Output>;

    /// Process multiple inputs concurrently.
    ///
    /// Default implementation uses `tokio::JoinSet` to run all invocations in parallel.
    async fn batch(
        &self,
        inputs: Vec<Self::Input>,
        config: &RunnableConfig,
    ) -> Result<Vec<Self::Output>>
    where
        Self::Input: 'static,
        Self::Output: 'static,
    {
        let mut results = Vec::with_capacity(inputs.len());
        for input in inputs {
            results.push(self.invoke(input, config).await?);
        }
        Ok(results)
    }

    /// Stream output chunks for a single input.
    ///
    /// Default implementation yields a single item from `invoke`.
    async fn stream(
        &self,
        input: Self::Input,
        config: &RunnableConfig,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Self::Output>> + Send>>>
    where
        Self::Output: 'static,
    {
        let result = self.invoke(input, config).await?;
        Ok(Box::pin(futures::stream::once(async { Ok(result) })))
    }
}

/// Extension trait providing `.pipe()` and `.with_fallback()` for composing Runnables.
pub trait RunnableExt: Runnable + Sized {
    /// Compose this Runnable with another, creating a sequence where
    /// the output of `self` feeds into the input of `next`.
    fn pipe<R>(self, next: R) -> RunnableSequence<Self, R>
    where
        R: Runnable<Input = Self::Output>,
    {
        RunnableSequence {
            first: self,
            second: next,
        }
    }

    /// Wrap this Runnable with a fallback. If `self` fails, the `fallback`
    /// Runnable is invoked with the same input.
    fn with_fallback<R>(self, fallback: R) -> RunnableWithFallback<Self, R>
    where
        R: Runnable<Input = Self::Input, Output = Self::Output>,
        Self::Input: Clone,
    {
        RunnableWithFallback {
            primary: self,
            fallback,
        }
    }
}

impl<T: Runnable + Sized> RunnableExt for T {}

/// A Runnable composed of two sequential Runnables.
pub struct RunnableSequence<A, B> {
    pub(crate) first: A,
    pub(crate) second: B,
}

#[async_trait]
impl<A, B> Runnable for RunnableSequence<A, B>
where
    A: Runnable,
    B: Runnable<Input = A::Output>,
{
    type Input = A::Input;
    type Output = B::Output;

    async fn invoke(&self, input: Self::Input, config: &RunnableConfig) -> Result<Self::Output> {
        let intermediate = self.first.invoke(input, config).await?;
        self.second.invoke(intermediate, config).await
    }
}

/// A Runnable that passes its input through unchanged.
pub struct IdentityRunnable<T>(std::marker::PhantomData<T>);

impl<T> IdentityRunnable<T> {
    pub fn new() -> Self {
        Self(std::marker::PhantomData)
    }
}

impl<T> Default for IdentityRunnable<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl<T: Send + Sync + 'static> Runnable for IdentityRunnable<T> {
    type Input = T;
    type Output = T;

    async fn invoke(&self, input: Self::Input, _config: &RunnableConfig) -> Result<Self::Output> {
        Ok(input)
    }
}

// ---------------------------------------------------------------------------
// RunnableBranch
// ---------------------------------------------------------------------------

/// Conditional routing: evaluates conditions in order, routes input to
/// the first matching branch, or to a default Runnable.
pub struct RunnableBranch<I, O> {
    branches: Vec<(
        Box<dyn Fn(&I) -> bool + Send + Sync>,
        Box<dyn Runnable<Input = I, Output = O>>,
    )>,
    default: Box<dyn Runnable<Input = I, Output = O>>,
}

impl<I, O> RunnableBranch<I, O> {
    pub fn new(
        branches: Vec<(
            Box<dyn Fn(&I) -> bool + Send + Sync>,
            Box<dyn Runnable<Input = I, Output = O>>,
        )>,
        default: Box<dyn Runnable<Input = I, Output = O>>,
    ) -> Self {
        Self { branches, default }
    }
}

#[async_trait]
impl<I, O> Runnable for RunnableBranch<I, O>
where
    I: Send + 'static,
    O: Send + 'static,
{
    type Input = I;
    type Output = O;

    async fn invoke(&self, input: Self::Input, config: &RunnableConfig) -> Result<Self::Output> {
        for (condition, runnable) in &self.branches {
            if condition(&input) {
                return runnable.invoke(input, config).await;
            }
        }
        self.default.invoke(input, config).await
    }
}

// ---------------------------------------------------------------------------
// RunnableWithFallback
// ---------------------------------------------------------------------------

/// Wraps a primary Runnable and a fallback. If the primary fails, the
/// fallback is invoked with the same (cloned) input.
pub struct RunnableWithFallback<A, B> {
    pub(crate) primary: A,
    pub(crate) fallback: B,
}

#[async_trait]
impl<A, B> Runnable for RunnableWithFallback<A, B>
where
    A: Runnable,
    B: Runnable<Input = A::Input, Output = A::Output>,
    A::Input: Clone,
{
    type Input = A::Input;
    type Output = A::Output;

    async fn invoke(&self, input: Self::Input, config: &RunnableConfig) -> Result<Self::Output> {
        let input_clone = input.clone();
        match self.primary.invoke(input, config).await {
            Ok(output) => Ok(output),
            Err(_) => self.fallback.invoke(input_clone, config).await,
        }
    }
}

// ---------------------------------------------------------------------------
// RunnablePassthrough
// ---------------------------------------------------------------------------

/// Passes `serde_json::Value` input through unchanged, optionally computing
/// and merging additional fields via `assign`.
pub struct RunnablePassthrough {
    assignments: Vec<(
        String,
        Box<dyn Runnable<Input = Value, Output = Value>>,
    )>,
}

impl RunnablePassthrough {
    pub fn new() -> Self {
        Self {
            assignments: vec![],
        }
    }

    /// Add a computed field: the given Runnable receives the original input
    /// and its output is inserted under `key` in the result object.
    pub fn assign(
        mut self,
        key: impl Into<String>,
        runnable: Box<dyn Runnable<Input = Value, Output = Value>>,
    ) -> Self {
        self.assignments.push((key.into(), runnable));
        self
    }
}

impl Default for RunnablePassthrough {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Runnable for RunnablePassthrough {
    type Input = Value;
    type Output = Value;

    async fn invoke(&self, input: Self::Input, config: &RunnableConfig) -> Result<Self::Output> {
        if self.assignments.is_empty() {
            return Ok(input);
        }

        let mut output = input.clone();
        for (key, runnable) in &self.assignments {
            let value = runnable.invoke(input.clone(), config).await?;
            if let Value::Object(ref mut map) = output {
                map.insert(key.clone(), value);
            }
        }
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::AyasError;

    struct AddOne;

    #[async_trait]
    impl Runnable for AddOne {
        type Input = i32;
        type Output = i32;

        async fn invoke(&self, input: i32, _config: &RunnableConfig) -> Result<i32> {
            Ok(input + 1)
        }
    }

    struct MultiplyTwo;

    #[async_trait]
    impl Runnable for MultiplyTwo {
        type Input = i32;
        type Output = i32;

        async fn invoke(&self, input: i32, _config: &RunnableConfig) -> Result<i32> {
            Ok(input * 2)
        }
    }

    struct FailRunnable;

    #[async_trait]
    impl Runnable for FailRunnable {
        type Input = i32;
        type Output = i32;

        async fn invoke(&self, _input: i32, _config: &RunnableConfig) -> Result<i32> {
            Err(AyasError::Other("intentional failure".into()))
        }
    }

    struct ToString;

    #[async_trait]
    impl Runnable for ToString {
        type Input = i32;
        type Output = String;

        async fn invoke(&self, input: i32, _config: &RunnableConfig) -> Result<String> {
            Ok(input.to_string())
        }
    }

    #[tokio::test]
    async fn identity_runnable() {
        let r = IdentityRunnable::<i32>::new();
        let config = RunnableConfig::default();
        let result = r.invoke(42, &config).await.unwrap();
        assert_eq!(result, 42);
    }

    #[tokio::test]
    async fn pipe_two_runnables() {
        let chain = AddOne.pipe(MultiplyTwo);
        let config = RunnableConfig::default();
        // (5 + 1) * 2 = 12
        let result = chain.invoke(5, &config).await.unwrap();
        assert_eq!(result, 12);
    }

    #[tokio::test]
    async fn pipe_three_runnables() {
        let chain = AddOne.pipe(MultiplyTwo).pipe(AddOne);
        let config = RunnableConfig::default();
        // ((3 + 1) * 2) + 1 = 9
        let result = chain.invoke(3, &config).await.unwrap();
        assert_eq!(result, 9);
    }

    #[tokio::test]
    async fn pipe_with_type_change() {
        let chain = AddOne.pipe(ToString);
        let config = RunnableConfig::default();
        let result = chain.invoke(9, &config).await.unwrap();
        assert_eq!(result, "10");
    }

    #[tokio::test]
    async fn pipe_error_propagation() {
        let chain = AddOne.pipe(FailRunnable);
        let config = RunnableConfig::default();
        let result = chain.invoke(5, &config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn batch_default_implementation() {
        let r = AddOne;
        let config = RunnableConfig::default();
        let results = r.batch(vec![1, 2, 3], &config).await.unwrap();
        assert_eq!(results, vec![2, 3, 4]);
    }

    #[tokio::test]
    async fn stream_default_implementation() {
        use futures::StreamExt;

        let r = AddOne;
        let config = RunnableConfig::default();
        let mut stream = r.stream(5, &config).await.unwrap();
        let item = stream.next().await.unwrap().unwrap();
        assert_eq!(item, 6);
        assert!(stream.next().await.is_none());
    }

    // -----------------------------------------------------------------------
    // RunnableBranch tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn branch_matches_first_condition() {
        let branch = RunnableBranch::new(
            vec![
                (
                    Box::new(|x: &i32| *x > 10),
                    Box::new(MultiplyTwo) as Box<dyn Runnable<Input = i32, Output = i32>>,
                ),
                (
                    Box::new(|x: &i32| *x > 5),
                    Box::new(AddOne) as Box<dyn Runnable<Input = i32, Output = i32>>,
                ),
            ],
            Box::new(IdentityRunnable::<i32>::new()),
        );
        let config = RunnableConfig::default();
        // 20 > 10 → MultiplyTwo → 40
        let result = branch.invoke(20, &config).await.unwrap();
        assert_eq!(result, 40);
    }

    #[tokio::test]
    async fn branch_matches_second_condition() {
        let branch = RunnableBranch::new(
            vec![
                (
                    Box::new(|x: &i32| *x > 10),
                    Box::new(MultiplyTwo) as Box<dyn Runnable<Input = i32, Output = i32>>,
                ),
                (
                    Box::new(|x: &i32| *x > 5),
                    Box::new(AddOne) as Box<dyn Runnable<Input = i32, Output = i32>>,
                ),
            ],
            Box::new(IdentityRunnable::<i32>::new()),
        );
        let config = RunnableConfig::default();
        // 7 > 5 but not > 10 → AddOne → 8
        let result = branch.invoke(7, &config).await.unwrap();
        assert_eq!(result, 8);
    }

    #[tokio::test]
    async fn branch_falls_through_to_default() {
        let branch = RunnableBranch::new(
            vec![(
                Box::new(|x: &i32| *x > 100) as Box<dyn Fn(&i32) -> bool + Send + Sync>,
                Box::new(MultiplyTwo) as Box<dyn Runnable<Input = i32, Output = i32>>,
            )],
            Box::new(AddOne),
        );
        let config = RunnableConfig::default();
        // 3 not > 100 → default (AddOne) → 4
        let result = branch.invoke(3, &config).await.unwrap();
        assert_eq!(result, 4);
    }

    // -----------------------------------------------------------------------
    // RunnableWithFallback tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn fallback_not_used_on_success() {
        let r = AddOne.with_fallback(MultiplyTwo);
        let config = RunnableConfig::default();
        // AddOne succeeds: 5 + 1 = 6
        let result = r.invoke(5, &config).await.unwrap();
        assert_eq!(result, 6);
    }

    #[tokio::test]
    async fn fallback_used_on_primary_failure() {
        let r = FailRunnable.with_fallback(AddOne);
        let config = RunnableConfig::default();
        // FailRunnable fails → fallback AddOne: 5 + 1 = 6
        let result = r.invoke(5, &config).await.unwrap();
        assert_eq!(result, 6);
    }

    #[tokio::test]
    async fn fallback_both_fail() {
        let r = FailRunnable.with_fallback(FailRunnable);
        let config = RunnableConfig::default();
        let result = r.invoke(5, &config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn fallback_in_pipe_chain() {
        // Pipe AddOne (succeeds) into FailRunnable.with_fallback(MultiplyTwo)
        let chain = AddOne.pipe(FailRunnable.with_fallback(MultiplyTwo));
        let config = RunnableConfig::default();
        // AddOne: 5 → 6, then FailRunnable fails → MultiplyTwo: 6 * 2 = 12
        let result = chain.invoke(5, &config).await.unwrap();
        assert_eq!(result, 12);
    }

    // -----------------------------------------------------------------------
    // RunnablePassthrough tests
    // -----------------------------------------------------------------------

    struct ExtractName;

    #[async_trait]
    impl Runnable for ExtractName {
        type Input = Value;
        type Output = Value;

        async fn invoke(&self, input: Value, _config: &RunnableConfig) -> Result<Value> {
            let name = input
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            Ok(Value::String(format!("Hello, {}!", name)))
        }
    }

    #[tokio::test]
    async fn passthrough_no_assignments() {
        let r = RunnablePassthrough::new();
        let config = RunnableConfig::default();
        let input = serde_json::json!({"name": "Alice", "age": 30});
        let result = r.invoke(input.clone(), &config).await.unwrap();
        assert_eq!(result, input);
    }

    #[tokio::test]
    async fn passthrough_with_assign() {
        let r = RunnablePassthrough::new().assign("greeting", Box::new(ExtractName));
        let config = RunnableConfig::default();
        let input = serde_json::json!({"name": "Alice"});
        let result = r.invoke(input, &config).await.unwrap();
        assert_eq!(result["name"], "Alice");
        assert_eq!(result["greeting"], "Hello, Alice!");
    }

    #[tokio::test]
    async fn passthrough_with_multiple_assigns() {
        struct UpperName;

        #[async_trait]
        impl Runnable for UpperName {
            type Input = Value;
            type Output = Value;

            async fn invoke(&self, input: Value, _config: &RunnableConfig) -> Result<Value> {
                let name = input
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                Ok(Value::String(name.to_uppercase()))
            }
        }

        let r = RunnablePassthrough::new()
            .assign("greeting", Box::new(ExtractName))
            .assign("upper_name", Box::new(UpperName));
        let config = RunnableConfig::default();
        let input = serde_json::json!({"name": "Bob"});
        let result = r.invoke(input, &config).await.unwrap();
        assert_eq!(result["name"], "Bob");
        assert_eq!(result["greeting"], "Hello, Bob!");
        assert_eq!(result["upper_name"], "BOB");
    }
}
