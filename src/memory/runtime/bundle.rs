use std::path::PathBuf;
use std::sync::Weak;

use crate::error::AlephError;
use crate::memory::store::sqlite::SqliteMemoryBackend;
use crate::sync_primitives::{Arc, Mutex};
use tokio::sync::watch;

use super::services::MemoryServices;
use super::{MemoryOperation, MemoryRuntime, MemoryRuntimeConfig, MemoryServicesConfig};

pub(super) struct MemoryBundle {
    pub(super) note_memory_dir: PathBuf,
    pub(super) backend: Arc<SqliteMemoryBackend>,
    stop: watch::Sender<bool>,
    services: Mutex<Option<Arc<MemoryServices>>>,
    watcher: tokio::sync::Mutex<Option<crate::memory::notes::watcher::NoteVaultWatcher>>,
}

impl MemoryBundle {
    pub(super) fn open(
        config: &MemoryRuntimeConfig,
        owner: Weak<MemoryRuntime>,
    ) -> Result<Arc<Self>, AlephError> {
        Ok(Arc::new(Self {
            note_memory_dir: config.note_memory_dir.clone(),
            backend: Arc::new(SqliteMemoryBackend::new(&config.metadata_path)?.with_runtime(owner)),
            stop: watch::channel(false).0,
            services: Mutex::new(None),
            watcher: tokio::sync::Mutex::new(None),
        }))
    }

    pub(super) async fn initialize(
        &self,
        config: &MemoryRuntimeConfig,
        inputs: Option<&MemoryServicesConfig>,
        runtime: &Arc<MemoryRuntime>,
    ) -> Result<(), AlephError> {
        std::fs::create_dir_all(&config.note_memory_dir).map_err(|error| {
            AlephError::other(format!("memory notes initialization failed: {error}"))
        })?;
        if let Some(inputs) = inputs {
            let services = MemoryServices::initialize(config, inputs, self.backend.clone()).await?;
            let indexer = services.indexer.clone();
            *self
                .services
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = Some(services);
            let mut watcher = self.watcher.lock().await;
            *watcher = Some(
                crate::memory::notes::watcher::prepare_owned_note_vault_watcher(
                    indexer,
                    Arc::downgrade(runtime),
                )?,
            );
            // Keep the native owner in the bundle even if registration fails.
            watcher
                .as_mut()
                .expect("watcher was installed")
                .start_watching()?;
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

    /// Install all real task handles before Running can become observable.
    pub(super) fn mount(
        &self,
        runtime: &Arc<MemoryRuntime>,
        operation: &MemoryOperation,
    ) -> Result<(), AlephError> {
        let services = self
            .services
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        let Some(services) = services else {
            return Ok(());
        };
        let orientation = services.orientation.clone();
        let agent = services.default_agent_id.clone();
        operation.spawn(async move {
            if let Err(error) = orientation.bootstrap(&agent).await {
                tracing::warn!(%error, "Wiki orientation bootstrap failed; continuing without it");
            }
        })?;
        let indexer = services.indexer.clone();
        let agent = services.default_agent_id.clone();
        let residue_age = services.config.compound_ingest.tx_residue_gc_seconds;
        operation.spawn(async move {
            tracing::info!(%agent, "Reconciling note index with disk");
            let stats = indexer.full_rebuild_all(&agent, residue_age).await;
            tracing::info!(
                corpora = stats.corpora,
                indexed = stats.total.indexed,
                skipped = stats.total.skipped,
                errors = stats.total.errors,
                pruned = stats.total.pruned,
                stale_vectors = stats.total.stale_vectors,
                tx_residue_removed = stats.tx_residue_removed,
                "Note index rebuild complete"
            );
            for (corpus, error) in &stats.failed {
                tracing::warn!(%corpus, %error, "Note corpus left unreconciled at startup");
            }
        })?;
        let embedding = services.embedding.clone();
        let backend = self.backend.clone();
        operation.spawn(async move {
            if let Some(provider) = embedding.get_active_provider().await {
                use crate::memory::embedding_signature;
                let current = embedding_signature::provider_signature(provider.as_ref());
                let stored = backend.get_embedding_signature().ok().flatten();
                if let embedding_signature::SignatureStatus::Mismatch { stored, current } =
                    embedding_signature::compare(stored.as_deref(), &current)
                {
                    tracing::warn!(
                        stored = %stored,
                        current = %current,
                        "Embedding model changed since notes were last embedded — run memory.reembed to keep retrieval accurate."
                    );
                }
            }
        })?;
        if let Some(service) = &services.compression {
            runtime.retain_worker(
                service
                    .clone()
                    .start_owned_background_task(runtime.clone(), self.subscribe_stop()),
            );
        }
        if let Some(dream) = &services.dream {
            runtime.retain_worker(
                dream
                    .clone()
                    .start_owned_background_task(runtime.clone(), self.subscribe_stop()),
            );
        }
        runtime.retain_worker(
            services
                .producer
                .clone()
                .spawn_owned(runtime.clone(), self.subscribe_stop()),
        );
        if services.config.reconciler.enabled {
            if let Some(reconciler) = &services.reconciler {
                runtime.retain_worker(reconciler.clone().spawn_owned_reconciler_daemon(
                    std::time::Duration::from_secs(services.config.reconciler.interval_secs),
                    runtime.clone(),
                    self.subscribe_stop(),
                ));
            }
        }
        Ok(())
    }

    pub(super) async fn drain_watcher(&self) -> Result<(), AlephError> {
        let mut watcher = self.watcher.lock().await;
        if let Some(watcher) = watcher.as_mut() {
            watcher.shutdown().await?;
        }
        Ok(())
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
