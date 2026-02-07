use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use uuid::Uuid;

use crate::error::Result;
use crate::types::Run;
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

    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }
}

struct Inner {
    sender: mpsc::UnboundedSender<Run>,
    config: SmithConfig,
}

/// Client for submitting traced runs to background Parquet writer.
///
/// Runs are buffered and written in batches for minimal performance impact.
/// Submission failures are silently ignored (fail-safe).
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

        let (sender, receiver) = mpsc::unbounded_channel();
        let inner = Arc::new(Inner {
            sender,
            config: config.clone(),
        });

        tokio::spawn(background_writer(receiver, config));

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

    /// Submit a run to the background writer. Silently ignores failures.
    pub fn submit_run(&self, run: Run) {
        if let Some(inner) = &self.inner {
            let _ = inner.sender.send(run);
        }
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
}

async fn background_writer(mut receiver: mpsc::UnboundedReceiver<Run>, config: SmithConfig) {
    let mut buffer: Vec<Run> = Vec::new();

    loop {
        let flush_timeout = tokio::time::sleep(config.flush_interval);
        tokio::pin!(flush_timeout);

        tokio::select! {
            msg = receiver.recv() => {
                match msg {
                    Some(run) => {
                        buffer.push(run);
                        if buffer.len() >= config.batch_size {
                            flush_buffer(&mut buffer, &config);
                        }
                    }
                    None => {
                        // Channel closed, flush remaining and exit
                        flush_buffer(&mut buffer, &config);
                        return;
                    }
                }
            }
            _ = &mut flush_timeout => {
                if !buffer.is_empty() {
                    flush_buffer(&mut buffer, &config);
                }
            }
        }
    }
}

fn flush_buffer(buffer: &mut Vec<Run>, config: &SmithConfig) {
    if buffer.is_empty() {
        return;
    }

    let batch_id = Uuid::new_v4().to_string()[..8].to_string();
    let path = writer::parquet_path(&config.base_dir, &config.project, &batch_id);
    let runs = std::mem::take(buffer);

    // Fail-safe: errors in writing don't propagate
    if let Err(e) = writer::write_runs(&runs, &path) {
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
        assert!(config.enabled);
    }

    #[test]
    fn smith_config_builder() {
        let config = SmithConfig::default()
            .with_base_dir("/tmp/test")
            .with_project("my-proj")
            .with_batch_size(50)
            .with_flush_interval(Duration::from_secs(10));

        assert_eq!(config.base_dir, PathBuf::from("/tmp/test"));
        assert_eq!(config.project, "my-proj");
        assert_eq!(config.batch_size, 50);
        assert_eq!(config.flush_interval, Duration::from_secs(10));
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
}
