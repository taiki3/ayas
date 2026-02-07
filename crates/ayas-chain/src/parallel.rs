use async_trait::async_trait;

use ayas_core::config::RunnableConfig;
use ayas_core::error::Result;
use ayas_core::runnable::Runnable;

/// A Runnable that executes two branches in parallel and returns both results.
///
/// Both branches receive a clone of the same input.
pub struct RunnableParallel<A, B> {
    pub branch_a: A,
    pub branch_b: B,
}

impl<A, B> RunnableParallel<A, B> {
    pub fn new(branch_a: A, branch_b: B) -> Self {
        Self { branch_a, branch_b }
    }
}

#[async_trait]
impl<A, B, I> Runnable for RunnableParallel<A, B>
where
    I: Clone + Send + Sync + 'static,
    A: Runnable<Input = I> + 'static,
    B: Runnable<Input = I> + 'static,
    A::Output: 'static,
    B::Output: 'static,
{
    type Input = I;
    type Output = (A::Output, B::Output);

    async fn invoke(&self, input: Self::Input, config: &RunnableConfig) -> Result<Self::Output> {
        let input_a = input.clone();
        let input_b = input;

        let (result_a, result_b) = tokio::join!(
            self.branch_a.invoke(input_a, config),
            self.branch_b.invoke(input_b, config),
        );

        Ok((result_a?, result_b?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lambda::RunnableLambda;
    use ayas_core::runnable::RunnableExt;

    #[tokio::test]
    async fn parallel_basic() {
        let double = RunnableLambda::new(|x: i32, _| async move { Ok(x * 2) });
        let triple = RunnableLambda::new(|x: i32, _| async move { Ok(x * 3) });
        let parallel = RunnableParallel::new(double, triple);

        let config = RunnableConfig::default();
        let (a, b) = parallel.invoke(5, &config).await.unwrap();
        assert_eq!(a, 10);
        assert_eq!(b, 15);
    }

    #[tokio::test]
    async fn parallel_with_different_latencies() {
        let slow = RunnableLambda::new(|x: i32, _| async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            Ok(x + 100)
        });
        let fast = RunnableLambda::new(|x: i32, _| async move { Ok(x + 1) });
        let parallel = RunnableParallel::new(slow, fast);

        let config = RunnableConfig::default();
        let (a, b) = parallel.invoke(0, &config).await.unwrap();
        assert_eq!(a, 100);
        assert_eq!(b, 1);
    }

    #[tokio::test]
    async fn parallel_error_propagation() {
        use ayas_core::error::AyasError;

        let ok_branch = RunnableLambda::new(|x: i32, _| async move { Ok(x) });
        let err_branch = RunnableLambda::new(|_x: i32, _| async move {
            Err::<i32, _>(AyasError::Other("branch failed".into()))
        });
        let parallel = RunnableParallel::new(ok_branch, err_branch);

        let config = RunnableConfig::default();
        let result = parallel.invoke(1, &config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn parallel_then_pipe() {
        let double = RunnableLambda::new(|x: i32, _| async move { Ok(x * 2) });
        let triple = RunnableLambda::new(|x: i32, _| async move { Ok(x * 3) });
        let parallel = RunnableParallel::new(double, triple);

        let sum = RunnableLambda::new(|(a, b): (i32, i32), _| async move { Ok(a + b) });

        let chain = parallel.pipe(sum);
        let config = RunnableConfig::default();
        // (4*2) + (4*3) = 8 + 12 = 20
        let result = chain.invoke(4, &config).await.unwrap();
        assert_eq!(result, 20);
    }
}
