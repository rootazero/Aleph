use std::sync::Weak;

use crate::error::AlephError;
use crate::memory::store::sqlite::SqliteMemoryBackend;
use crate::sync_primitives::Arc;
use tokio::sync::watch;

use super::{MemoryRuntime, MemoryRuntimeConfig};

pub(super) struct MemoryBundle {
    pub(super) backend: Arc<SqliteMemoryBackend>,
    stop: watch::Sender<bool>,
}

impl MemoryBundle {
    pub(super) fn open(
        config: &MemoryRuntimeConfig,
        owner: Weak<MemoryRuntime>,
    ) -> Result<Arc<Self>, AlephError> {
        Ok(Arc::new(Self {
            backend: Arc::new(SqliteMemoryBackend::new(&config.metadata_path)?.with_runtime(owner)),
            stop: watch::channel(false).0,
        }))
    }

    pub(super) fn initialize(&self, config: &MemoryRuntimeConfig) -> Result<(), AlephError> {
        std::fs::create_dir_all(&config.note_memory_dir).map_err(|error| {
            AlephError::other(format!("memory notes initialization failed: {error}"))
        })
    }

    pub(super) fn request_stop(&self) {
        self.stop.send_replace(true);
    }

    pub(super) fn close(&self) -> Result<(), AlephError> {
        self.backend.close()
    }
}
