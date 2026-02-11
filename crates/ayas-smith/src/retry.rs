use std::future::Future;
use std::time::Duration;

/// Execute an async operation with exponential backoff and jitter.
///
/// Retries up to `max_retries` times on failure with delays of
/// 100ms, 200ms, 400ms... plus a small jitter.
///
/// Currently unused for local Parquet writes, but available for
/// future HTTP-based SmithStore implementations.
pub async fn with_retry<F, Fut, T, E>(max_retries: u32, f: F) -> Result<T, E>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    let mut retries = 0;
    loop {
        match f().await {
            Ok(v) => return Ok(v),
            Err(e) if retries < max_retries => {
                retries += 1;
                let base_ms = 100u64 * (1u64 << (retries - 1));
                // Simple jitter using current time nanoseconds (avoids rand dependency)
                let jitter_ms = (std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .subsec_nanos()
                    % 100) as u64;
                tokio::time::sleep(Duration::from_millis(base_ms + jitter_ms)).await;
            }
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[tokio::test]
    async fn retry_succeeds_first_try() {
        let result: Result<i32, &str> = with_retry(3, || async { Ok(42) }).await;
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn retry_succeeds_after_failures() {
        let attempts = AtomicU32::new(0);
        let result: Result<i32, &str> = with_retry(3, || {
            let n = attempts.fetch_add(1, Ordering::SeqCst);
            async move {
                if n < 2 {
                    Err("not yet")
                } else {
                    Ok(42)
                }
            }
        })
        .await;

        assert_eq!(result.unwrap(), 42);
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn retry_exhausts_all_retries() {
        let attempts = AtomicU32::new(0);
        let result: Result<i32, &str> = with_retry(2, || {
            attempts.fetch_add(1, Ordering::SeqCst);
            async { Err("always fails") }
        })
        .await;

        assert!(result.is_err());
        // 1 initial + 2 retries = 3 total attempts
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn retry_zero_retries_tries_once() {
        let attempts = AtomicU32::new(0);
        let result: Result<i32, &str> = with_retry(0, || {
            attempts.fetch_add(1, Ordering::SeqCst);
            async { Err("fail") }
        })
        .await;

        assert!(result.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }
}
