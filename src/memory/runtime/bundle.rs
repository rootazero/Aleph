use std::sync::Weak;

use crate::error::AlephError;
use crate::memory::store::sqlite::SqliteMemoryBackend;
use crate::sync_primitives::{Arc, Mutex};
use tokio::sync::watch;

use super::services::MemoryServices;
use super::{MemoryRuntime, MemoryRuntimeConfig, MemoryServicesConfig};

pub(super) struct MemoryBundle {
    pub(super) backend: Arc<SqliteMemoryBackend>,
    stop: watch::Sender<bool>,
    services: Mutex<Option<Arc<MemoryServices>>>,
}

impl MemoryBundle {
    pub(super) fn open(
        config: &MemoryRuntimeConfig,
        owner: Weak<MemoryRuntime>,
    ) -> Result<Arc<Self>, AlephError> {
        Ok(Arc::new(Self {
            backend: Arc::new(SqliteMemoryBackend::new(&config.metadata_path)?.with_runtime(owner)),
            stop: watch::channel(false).0,
            services: Mutex::new(None),
        }))
    }

    pub(super) async fn initialize(
        &self,
        config: &MemoryRuntimeConfig,
        inputs: Option<&MemoryServicesConfig>,
    ) -> Result<(), AlephError> {
        std::fs::create_dir_all(&config.note_memory_dir).map_err(|error| {
            AlephError::other(format!("memory notes initialization failed: {error}"))
        })?;
        if let Some(inputs) = inputs {
            let services = MemoryServices::initialize(config, inputs, self.backend.clone()).await?;
            *self
                .services
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = Some(services);
        }
        Ok(())
    }

    pub(super) fn services(&self) -> Result<Arc<MemoryServices>, AlephError> {
        self.services
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
            .ok_or_else(|| AlephError::other("memory services are not configured"))
    }

    pub(super) fn subscribe_stop(&self) -> watch::Receiver<bool> {
        self.stop.subscribe()
    }

    pub(super) fn request_stop(&self) {
        self.stop.send_replace(true);
    }

    pub(super) fn close(&self) -> Result<(), AlephError> {
        self.services
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        self.backend.close()
    }
}
