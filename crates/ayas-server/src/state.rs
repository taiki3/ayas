use std::path::PathBuf;
use std::sync::Arc;

use ayas_checkpoint::memory::MemoryCheckpointStore;

use crate::session::SessionStore;

/// Shared application state.
#[derive(Clone)]
pub struct AppState {
    pub session_store: SessionStore,
    pub checkpoint_store: Arc<MemoryCheckpointStore>,
    pub smith_base_dir: PathBuf,
}

impl AppState {
    pub fn new() -> Self {
        let smith_dir = if let Ok(dir) = std::env::var("AYAS_SMITH_DIR") {
            PathBuf::from(dir)
        } else {
            ayas_smith::client::SmithConfig::default().base_dir
        };
        Self {
            session_store: SessionStore::new(),
            checkpoint_store: Arc::new(MemoryCheckpointStore::new()),
            smith_base_dir: smith_dir,
        }
    }

    /// Create with a specific smith base directory (for testing).
    pub fn with_smith_dir(smith_dir: PathBuf) -> Self {
        Self {
            session_store: SessionStore::new(),
            checkpoint_store: Arc::new(MemoryCheckpointStore::new()),
            smith_base_dir: smith_dir,
        }
    }
}
