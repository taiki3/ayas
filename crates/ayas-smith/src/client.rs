use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use uuid::Uuid;

use crate::duckdb_store::DuckDbStore;
use crate::error::Result;
use crate::store::SmithStore;
use crate::types::{Run, RunBuilder, RunStatus, RunType};
use crate::writer;

/// Configuration for the SmithClient.
#[derive(Debug, Clone)]
pub struct SmithConfig {
    /// Base directory for storing Parquet files.
    pub base_dir: PathBuf,
    /// Project name for grouping runs.
    pub project: String,
    /// Number of runs to buffer before writing.
    pub batch_size: usize,
    /// Maximum time between flushes.
    pub flush_interval: Duration,
    /// Whether tracing is enabled.
    pub enabled: bool,
    /// Bounded channel capacity for backpressure control.
    pub channel_capacity: usize,
}

impl Default for SmithConfig {
    fn default() -> Self {
        let base_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".ayas-smith");
        Self {
            base_dir,
            project: "default".into(),
            batch_size: 100,
            flush_interval: Duration::from_secs(5),
            enabled: true,
            channel_capacity: 10_000,
        }
    }
}

impl SmithConfig {
    pub fn with_base_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.base_dir = dir.into();
        self
    }

    pub fn with_project(mut self, project: impl Into<String>) -> Self {
        self.project = project.into();
        self
    }

    pub fn with_batch_size(mut self, size: usize) -> Self {
        self.batch_size = size;
        self
    }

    pub fn with_flush_interval(mut self, interval: Duration) -> Self {
        self.flush_interval = interval;
        self
    }

    pub fn with_channel_capacity(mut self, capacity: usize) -> Self {
        self.channel_capacity = capacity;
        self
    }

    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }
}

struct Inner {
    sender: flume::Sender<Run>,
    config: SmithConfig,
    store: Arc<dyn SmithStore>,
    drop_count: AtomicU64,
}

/// Client for submitting traced runs to background Parquet writer.
///
/// Runs are buffered and written in batches for minimal performance impact.
/// Uses a bounded channel for backpressure; full channel drops runs silently.
#[derive(Clone)]
pub struct SmithClient {
    inner: Option<Arc<Inner>>,
}

impl SmithClient {
    /// Create a new SmithClient with background writer task.
    pub fn new(config: SmithConfig) -> Self {
        if !config.enabled {
            return Self::noop();
        }

        let store = Arc::new(DuckDbStore::new(&config.base_dir));
        Self::with_store(config, store)
    }

    /// Create a new SmithClient with a custom store implementation.
    pub fn with_store(config: SmithConfig, store: Arc<dyn SmithStore>) -> Self {
        if !config.enabled {
            return Self::noop();
        }

        let (sender, receiver) = flume::bounded(config.channel_capacity);
        let inner = Arc::new(Inner {
            sender,
            config: config.clone(),
            store: store.clone(),
            drop_count: AtomicU64::new(0),
        });

        tokio::spawn(background_writer(receiver, config, store));

        Self { inner: Some(inner) }
    }

    /// Create a disabled client that drops all runs.
    pub fn noop() -> Self {
        Self { inner: None }
    }

    /// Check if this client is enabled (not noop).
    pub fn is_enabled(&self) -> bool {
        self.inner.is_some()
    }

    /// Submit a run to the background writer.
    /// Uses try_send for non-blocking send; drops the run if the channel is full.
    pub fn submit_run(&self, run: Run) {
        if let Some(inner) = &self.inner {
            if inner.sender.try_send(run).is_err() {
                let prev = inner.drop_count.fetch_add(1, Ordering::Relaxed);
                if prev == 0 {
                    eprintln!(
                        "ayas-smith: channel full or closed, runs are being dropped"
                    );
                }
            }
        }
    }

    /// Number of runs dropped because the channel was full or closed.
    pub fn drop_count(&self) -> u64 {
        self.inner
            .as_ref()
            .map(|i| i.drop_count.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    /// Get the project name for this client.
    pub fn project(&self) -> &str {
        self.inner
            .as_ref()
            .map(|i| i.config.project.as_str())
            .unwrap_or("default")
    }

    /// Get the base directory for this client.
    pub fn base_dir(&self) -> Option<&PathBuf> {
        self.inner.as_ref().map(|i| &i.config.base_dir)
    }

    /// Get the underlying store, if enabled.
    pub fn store(&self) -> Option<&Arc<dyn SmithStore>> {
        self.inner.as_ref().map(|i| &i.store)
    }
}

async fn background_writer(
    receiver: flume::Receiver<Run>,
    config: SmithConfig,
    store: Arc<dyn SmithStore>,
) {
    let mut buffer: Vec<Run> = Vec::new();

    loop {
        let flush_timeout = tokio::time::sleep(config.flush_interval);
        tokio::pin!(flush_timeout);

        tokio::select! {
            msg = receiver.recv_async() => {
                match msg {
                    Ok(run) => {
                        buffer.push(run);
                        if buffer.len() >= config.batch_size {
                            flush_buffer(&mut buffer, &*store).await;
                        }
                    }
                    Err(_) => {
                        // Channel closed, flush remaining and exit
                        flush_buffer(&mut buffer, &*store).await;
                        return;
                    }
                }
            }
            _ = &mut flush_timeout => {
                if !buffer.is_empty() {
                    flush_buffer(&mut buffer, &*store).await;
                }
            }
        }
    }
}

async fn flush_buffer(buffer: &mut Vec<Run>, store: &dyn SmithStore) {
    if buffer.is_empty() {
        return;
    }

    let runs = std::mem::take(buffer);

    // Fail-safe: errors in writing don't propagate
    if let Err(e) = store.put_runs(&runs).await {
        eprintln!("ayas-smith: failed to write runs: {e}");
    }
}

/// Flush and write runs synchronously (for testing).
pub fn flush_runs(runs: &[Run], base_dir: &std::path::Path, project: &str) -> Result<PathBuf> {
    let batch_id = Uuid::new_v4().to_string()[..8].to_string();
    let path = writer::parquet_path(base_dir, project, &batch_id);
    writer::write_runs(runs, &path)?;
    Ok(path)
}

// ---------------------------------------------------------------------------
// RunGuard: RAII guard for 2-phase run lifecycle
// ---------------------------------------------------------------------------

/// RAII guard that manages a run's lifecycle.
///
/// On creation, submits a `Running` run. On explicit `finish_ok`/`finish_err`,
/// submits the completed run. If dropped without finishing (e.g., during a panic),
/// automatically submits an `Error` run.
pub struct RunGuard {
    client: SmithClient,
    skeleton: Run,
    finished: AtomicBool,
}

impl RunGuard {
    /// Create a new RunGuard from a RunBuilder. Submits the initial Running run.
    pub fn new(client: SmithClient, builder: RunBuilder) -> Self {
        let skeleton = builder.start();
        client.submit_run(skeleton.clone());
        Self {
            client,
            skeleton,
            finished: AtomicBool::new(false),
        }
    }

    /// Convenience: create a guard with common parameters.
    pub fn start(
        client: SmithClient,
        name: impl Into<String>,
        run_type: RunType,
    ) -> Self {
        let builder = Run::builder(name, run_type).project(client.project());
        Self::new(client, builder)
    }

    /// Get the run_id of this guard's run.
    pub fn run_id(&self) -> Uuid {
        self.skeleton.run_id
    }

    /// Get the trace_id of this guard's run.
    pub fn trace_id(&self) -> Uuid {
        self.skeleton.trace_id
    }

    /// Complete the run successfully with the given output.
    pub fn finish_ok(self, output: impl Into<String>) {
        self.finished.store(true, Ordering::Release);
        self.submit_final(RunStatus::Success, Some(output.into()), None);
    }

    /// Complete the run with an error.
    pub fn finish_err(self, error: impl Into<String>) {
        self.finished.store(true, Ordering::Release);
        self.submit_final(RunStatus::Error, None, Some(error.into()));
    }

    /// Complete the run successfully with LLM token usage.
    pub fn finish_llm(
        self,
        output: impl Into<String>,
        input_tokens: i64,
        output_tokens: i64,
        total_tokens: i64,
    ) {
        self.finished.store(true, Ordering::Release);
        let end_time = Utc::now();
        let latency_ms = (end_time - self.skeleton.start_time).num_milliseconds();
        let mut run = self.skeleton.clone();
        run.end_time = Some(end_time);
        run.status = RunStatus::Success;
        run.output = Some(output.into());
        run.input_tokens = Some(input_tokens);
        run.output_tokens = Some(output_tokens);
        run.total_tokens = Some(total_tokens);
        run.latency_ms = Some(latency_ms);
        self.client.submit_run(run);
    }

    fn submit_final(&self, status: RunStatus, output: Option<String>, error: Option<String>) {
        let end_time = Utc::now();
        let latency_ms = (end_time - self.skeleton.start_time).num_milliseconds();
        let mut run = self.skeleton.clone();
        run.end_time = Some(end_time);
        run.status = status;
        run.output = output;
        run.error = error;
        run.latency_ms = Some(latency_ms);
        self.client.submit_run(run);
    }
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        if !self.finished.load(Ordering::Acquire) {
            self.finished.store(true, Ordering::Release);
            let end_time = Utc::now();
            let latency_ms = (end_time - self.skeleton.start_time).num_milliseconds();
            let mut run = self.skeleton.clone();
            run.end_time = Some(end_time);
            run.status = RunStatus::Error;
            run.error = Some(
                "run guard dropped without explicit finish (possible panic)".into(),
            );
            run.latency_ms = Some(latency_ms);
            self.client.submit_run(run);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::RunType;

    #[test]
    fn smith_config_default() {
        let config = SmithConfig::default();
        assert!(config.base_dir.to_string_lossy().contains(".ayas-smith"));
        assert_eq!(config.project, "default");
        assert_eq!(config.batch_size, 100);
        assert_eq!(config.channel_capacity, 10_000);
        assert!(config.enabled);
    }

    #[test]
    fn smith_config_builder() {
        let config = SmithConfig::default()
            .with_base_dir("/tmp/test")
            .with_project("my-proj")
            .with_batch_size(50)
            .with_flush_interval(Duration::from_secs(10))
            .with_channel_capacity(5_000);

        assert_eq!(config.base_dir, PathBuf::from("/tmp/test"));
        assert_eq!(config.project, "my-proj");
        assert_eq!(config.batch_size, 50);
        assert_eq!(config.flush_interval, Duration::from_secs(10));
        assert_eq!(config.channel_capacity, 5_000);
    }

    #[test]
    fn smith_config_disabled() {
        let config = SmithConfig::default().disabled();
        assert!(!config.enabled);
    }

    #[test]
    fn noop_client() {
        let client = SmithClient::noop();
        assert!(!client.is_enabled());
        // Should not panic
        let run = Run::builder("test", RunType::Chain).finish_ok("ok");
        client.submit_run(run);
    }

    #[tokio::test]
    async fn disabled_config_creates_noop() {
        let config = SmithConfig::default().disabled();
        let client = SmithClient::new(config);
        assert!(!client.is_enabled());
    }

    #[tokio::test]
    async fn client_is_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let config = SmithConfig::default().with_base_dir(dir.path());
        let client = SmithClient::new(config);
        assert!(client.is_enabled());
    }

    #[tokio::test]
    async fn client_clone_shares_channel() {
        let dir = tempfile::tempdir().unwrap();
        let config = SmithConfig::default().with_base_dir(dir.path());
        let client1 = SmithClient::new(config);
        let client2 = client1.clone();
        assert!(client1.is_enabled());
        assert!(client2.is_enabled());
    }

    #[tokio::test]
    async fn submit_and_flush() {
        let dir = tempfile::tempdir().unwrap();
        let config = SmithConfig::default()
            .with_base_dir(dir.path())
            .with_project("test-proj")
            .with_batch_size(2)
            .with_flush_interval(Duration::from_millis(50));

        let client = SmithClient::new(config);

        for i in 0..3 {
            let run = Run::builder(format!("run-{i}"), RunType::Chain)
                .project("test-proj")
                .finish_ok("ok");
            client.submit_run(run);
        }

        // Wait for flush
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Verify parquet files were written
        let project_dir = dir.path().join("test-proj");
        assert!(project_dir.exists());
    }

    #[tokio::test]
    async fn bounded_channel_drops_when_full() {
        let dir = tempfile::tempdir().unwrap();
        let config = SmithConfig::default()
            .with_base_dir(dir.path())
            .with_channel_capacity(2)
            .with_batch_size(1000) // large batch size so runs stay in channel
            .with_flush_interval(Duration::from_secs(60));

        let client = SmithClient::new(config);

        // Submit more runs than channel capacity
        for i in 0..10 {
            let run = Run::builder(format!("run-{i}"), RunType::Chain).finish_ok("ok");
            client.submit_run(run);
        }

        // Some runs should have been dropped
        assert!(client.drop_count() > 0);
    }

    #[test]
    fn flush_runs_sync() {
        let dir = tempfile::tempdir().unwrap();
        let runs: Vec<Run> = (0..3)
            .map(|i| {
                Run::builder(format!("run-{i}"), RunType::Tool)
                    .project("sync-test")
                    .finish_ok("done")
            })
            .collect();

        let path = flush_runs(&runs, dir.path(), "sync-test").unwrap();
        assert!(path.exists());
    }

    #[test]
    fn client_project_name() {
        let client = SmithClient::noop();
        assert_eq!(client.project(), "default");
    }

    // --- RunGuard tests ---

    #[tokio::test]
    async fn run_guard_finish_ok() {
        let dir = tempfile::tempdir().unwrap();
        let config = SmithConfig::default()
            .with_base_dir(dir.path())
            .with_project("guard-test")
            .with_batch_size(1)
            .with_flush_interval(Duration::from_millis(50));

        let client = SmithClient::new(config);

        let guard = RunGuard::start(client.clone(), "test-chain", RunType::Chain);
        let run_id = guard.run_id();
        guard.finish_ok("result");

        // Wait for flush
        tokio::time::sleep(Duration::from_millis(300)).await;

        // Should have 2 entries (Running + Success), query dedup picks Success
        let store = client.store().unwrap();
        let run = store.get_run(run_id, "guard-test").await.unwrap();
        assert!(run.is_some());
        let run = run.unwrap();
        assert_eq!(run.status, RunStatus::Success);
        assert_eq!(run.output.as_deref(), Some("result"));
    }

    #[tokio::test]
    async fn run_guard_finish_err() {
        let dir = tempfile::tempdir().unwrap();
        let config = SmithConfig::default()
            .with_base_dir(dir.path())
            .with_project("guard-test")
            .with_batch_size(1)
            .with_flush_interval(Duration::from_millis(50));

        let client = SmithClient::new(config);

        let guard = RunGuard::start(client.clone(), "fail-chain", RunType::Chain);
        let run_id = guard.run_id();
        guard.finish_err("something went wrong");

        tokio::time::sleep(Duration::from_millis(300)).await;

        let store = client.store().unwrap();
        let run = store.get_run(run_id, "guard-test").await.unwrap();
        assert!(run.is_some());
        let run = run.unwrap();
        assert_eq!(run.status, RunStatus::Error);
        assert_eq!(run.error.as_deref(), Some("something went wrong"));
    }

    #[tokio::test]
    async fn run_guard_drop_sends_error() {
        let dir = tempfile::tempdir().unwrap();
        let config = SmithConfig::default()
            .with_base_dir(dir.path())
            .with_project("guard-test")
            .with_batch_size(1)
            .with_flush_interval(Duration::from_millis(50));

        let client = SmithClient::new(config);
        let run_id;

        {
            let guard = RunGuard::start(client.clone(), "drop-chain", RunType::Chain);
            run_id = guard.run_id();
            // Guard dropped without finish
        }

        tokio::time::sleep(Duration::from_millis(300)).await;

        let store = client.store().unwrap();
        let run = store.get_run(run_id, "guard-test").await.unwrap();
        assert!(run.is_some());
        let run = run.unwrap();
        assert_eq!(run.status, RunStatus::Error);
        assert!(run.error.as_deref().unwrap().contains("dropped without explicit finish"));
    }
}
