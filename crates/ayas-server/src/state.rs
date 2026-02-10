use std::path::PathBuf;
use std::sync::Arc;

use ayas_checkpoint::memory::MemoryCheckpointStore;
use ayas_smith::client::{SmithClient, SmithConfig};

use crate::session::SessionStore;

/// Shared application state.
#[derive(Clone)]
pub struct AppState {
    pub session_store: SessionStore,
    pub checkpoint_store: Arc<MemoryCheckpointStore>,
    pub smith_base_dir: PathBuf,
    pub smith_client: SmithClient,
}

impl AppState {
    pub fn new() -> Self {
        let smith_dir = if let Ok(dir) = std::env::var("AYAS_SMITH_DIR") {
            PathBuf::from(dir)
        } else {
            SmithConfig::default().base_dir
        };
        let smith_client = SmithClient::new(
            SmithConfig::default().with_base_dir(&smith_dir),
        );
        Self {
            session_store: SessionStore::new(),
            checkpoint_store: Arc::new(MemoryCheckpointStore::new()),
            smith_base_dir: smith_dir,
            smith_client,
        }
    }

    /// Create with a specific smith base directory (for testing).
    pub fn with_smith_dir(smith_dir: PathBuf) -> Self {
        let smith_client = SmithClient::new(
            SmithConfig::default().with_base_dir(&smith_dir),
        );
        Self {
            session_store: SessionStore::new(),
            checkpoint_store: Arc::new(MemoryCheckpointStore::new()),
            smith_base_dir: smith_dir,
            smith_client,
        }
    }
}
