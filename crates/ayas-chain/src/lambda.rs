use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;

use ayas_core::config::RunnableConfig;
use ayas_core::error::Result;
use ayas_core::runnable::Runnable;

type AsyncFn<I, O> =
    dyn Fn(I, RunnableConfig) -> Pin<Box<dyn Future<Output = Result<O>> + Send>> + Send + Sync;

/// A Runnable that wraps an async closure.
pub struct RunnableLambda<I, O> {
    func: Arc<AsyncFn<I, O>>,
}

impl<I, O> RunnableLambda<I, O>
where
    I: Send + 'static,
    O: Send + 'static,
{
    /// Create a new `RunnableLambda` from an async function.
    ///
    /// The function receives the input and a cloned `RunnableConfig`.
    pub fn new<F, Fut>(func: F) -> Self
    where
        F: Fn(I, RunnableConfig) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<O>> + Send + 'static,
    {
        Self {
            func: Arc::new(move |input, config| Box::pin(func(input, config))),
        }
    }
}

#[async_trait]
impl<I, O> Runnable for RunnableLambda<I, O>
where
    I: Send + 'static,
    O: Send + 'static,
{
    type Input = I;
    type Output = O;

    async fn invoke(&self, input: Self::Input, config: &RunnableConfig) -> Result<Self::Output> {
        (self.func)(input, config.clone()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ayas_core::error::AyasError;

    #[tokio::test]
    async fn lambda_basic() {
        let double = RunnableLambda::new(|x: i32, _config| async move { Ok(x * 2) });
        let config = RunnableConfig::default();
        let result = double.invoke(5, &config).await.unwrap();
        assert_eq!(result, 10);
    }

    #[tokio::test]
    async fn lambda_with_string_transform() {
        let upper = RunnableLambda::new(|s: String, _config| async move { Ok(s.to_uppercase()) });
        let config = RunnableConfig::default();
        let result = upper.invoke("hello".into(), &config).await.unwrap();
        assert_eq!(result, "HELLO");
    }

    #[tokio::test]
    async fn lambda_error() {
        let fail = RunnableLambda::new(|_x: i32, _config| async move {
            Err(AyasError::Other("lambda failed".into()))
        });
        let config = RunnableConfig::default();
        let result: std::result::Result<i32, _> = fail.invoke(1, &config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn lambda_pipe() {
        use ayas_core::runnable::RunnableExt;

        let add_one = RunnableLambda::new(|x: i32, _config| async move { Ok(x + 1) });
        let to_string =
            RunnableLambda::new(|x: i32, _config| async move { Ok(format!("result: {x}")) });

        let chain = add_one.pipe(to_string);
        let config = RunnableConfig::default();
        let result = chain.invoke(9, &config).await.unwrap();
        assert_eq!(result, "result: 10");
    }

    #[tokio::test]
    async fn lambda_accesses_config() {
        let check_tag = RunnableLambda::new(|_x: i32, config: RunnableConfig| async move {
            if config.tags.contains(&"special".to_string()) {
                Ok(100)
            } else {
                Ok(0)
            }
        });
        let config = RunnableConfig::default().with_tag("special");
        let result = check_tag.invoke(1, &config).await.unwrap();
        assert_eq!(result, 100);
    }
}
