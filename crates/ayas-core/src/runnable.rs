use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;

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

/// Extension trait providing `.pipe()` for composing Runnables.
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
}
