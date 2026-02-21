use std::path::PathBuf;
use std::sync::Arc;

use ayas_checkpoint::memory::MemoryCheckpointStore;
use ayas_smith::clickhouse_store::ClickHouseStore;
use ayas_smith::client::{SmithClient, SmithConfig};
use ayas_smith::duckdb_store::DuckDbStore;
use ayas_smith::store::SmithStore;

use crate::session::SessionStore;

/// Shared application state.
#[derive(Clone)]
pub struct AppState {
    pub session_store: SessionStore,
    pub checkpoint_store: Arc<MemoryCheckpointStore>,
    pub smith_base_dir: PathBuf,
    pub smith_client: SmithClient,
    pub smith_store: Arc<dyn SmithStore>,
}

impl AppState {
    pub async fn new() -> Self {
        let smith_dir = if let Ok(dir) = std::env::var("AYAS_SMITH_DIR") {
            PathBuf::from(dir)
        } else {
            SmithConfig::default().base_dir
        };
        let smith_client = SmithClient::new(SmithConfig::default().with_base_dir(&smith_dir));

        let backend = std::env::var("AYAS_SMITH_BACKEND").unwrap_or_default();
        let smith_store: Arc<dyn SmithStore> = if backend == "clickhouse" {
            let store = ClickHouseStore::new();
            if let Err(e) = store.init().await {
                tracing::error!("Failed to initialize ClickHouse store: {e}");
            }
            Arc::new(store)
        } else {
            Arc::new(DuckDbStore::new(&smith_dir))
        };

        Self {
            session_store: SessionStore::new(),
            checkpoint_store: Arc::new(MemoryCheckpointStore::new()),
            smith_base_dir: smith_dir,
            smith_client,
            smith_store,
        }
    }

    /// Create with a specific smith base directory (for testing).
    /// Always uses DuckDbStore.
    pub fn with_smith_dir(smith_dir: PathBuf) -> Self {
        let smith_client = SmithClient::new(SmithConfig::default().with_base_dir(&smith_dir));
        Self {
            session_store: SessionStore::new(),
            checkpoint_store: Arc::new(MemoryCheckpointStore::new()),
            smith_store: Arc::new(DuckDbStore::new(&smith_dir)),
            smith_base_dir: smith_dir,
            smith_client,
        }
    }
}
