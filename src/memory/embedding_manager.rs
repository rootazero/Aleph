//! Embedding manager — manages provider lifecycle and active provider switching.

use crate::config::types::memory::{EmbeddingProviderConfig, EmbeddingSettings};
use crate::error::AlephError;
use crate::memory::embedding_provider::{
    create_provider, EmbeddingProvider, RemoteEmbeddingProvider,
};
use crate::memory::embedding_resolver::resolve;
use crate::sync_primitives::Arc;
use tokio::sync::{Mutex, RwLock};
use tracing::{info, warn};

/// Internal queue item carrying the data needed to embed and persist a note.
struct PendingItem {
    agent_id: String,
    path: String,
    body: String,
}

/// Manages embedding provider lifecycle
pub struct EmbeddingManager {
    settings: Arc<RwLock<EmbeddingSettings>>,
    active_provider: Arc<RwLock<Option<Arc<dyn EmbeddingProvider>>>>,
    /// Pending notes awaiting asynchronous embedding.
    /// Producers push via `push_pending`; the background flush task drains
    /// in batches via `flush_pending` (wired in B5.2).
    pending: Mutex<Vec<PendingItem>>,
}

impl EmbeddingManager {
    /// Create a new `EmbeddingManager` from settings
    #[must_use]
    pub fn new(settings: EmbeddingSettings) -> Self {
        Self {
            settings: Arc::new(RwLock::new(settings)),
            active_provider: Arc::new(RwLock::new(None)),
            pending: Mutex::new(Vec::new()),
        }
    }

    /// Initialize the active provider from current settings.
    /// Returns Ok(()) even if no provider is configured (degrades gracefully).
    pub async fn init(&self) -> Result<(), AlephError> {
        // Resolve via the local-first `auto` resolver. A pinned id still wins
        // exactly; an empty/`auto` id prefers an enabled local (Ollama) provider
        // so embeddings stay on-device (data stays local) without manual config.
        let (effective_id, reason, config) = {
            let settings = self.settings.read().await;
            let decision = resolve(&settings);
            (
                // rust-doctor-disable-next-line excessive-clone
                decision.effective.map(|c| c.id.clone()),
                decision.reason,
                decision.effective.cloned(),
            )
        }; // settings lock released

        if let Some(config) = config {
            match create_provider(&config) {
                Ok(provider) => {
                    *self.active_provider.write().await = Some(provider);
                    info!(
                        provider_id = effective_id.as_deref().unwrap_or(""),
                        reason = reason.as_str(),
                        "Embedding provider initialized"
                    );
                }
                Err(e) => {
                    warn!(
                        provider_id = effective_id.as_deref().unwrap_or(""),
                        reason = reason.as_str(),
                        error = %e,
                        "Failed to initialize embedding provider"
                    );
                }
            }
        } else {
            warn!(
                reason = reason.as_str(),
                "No embedding provider resolved — vector (semantic) recall disabled; \
                 memory retrieval degrades to keyword (FTS) search. \
                 Pin `active_provider_id` or set it to `auto` for local-first selection."
            );
        }

        Ok(())
    }

    /// Get the currently active provider. Returns None if not configured.
    pub async fn get_active_provider(&self) -> Option<Arc<dyn EmbeddingProvider>> {
        // rust-doctor-disable-next-line excessive-clone
        self.active_provider.read().await.clone()
    }

    /// Get the active provider or return an error.
    pub async fn require_active_provider(&self) -> Result<Arc<dyn EmbeddingProvider>, AlephError> {
        self.get_active_provider().await.ok_or_else(|| {
            AlephError::config("No active embedding provider configured. Please configure one in Settings > Embedding Providers.".to_string())
        })
    }

    /// Switch the active provider.
    ///
    /// Multi-dimension vector columns (`vec_768`, `vec_1024`, `vec_1536`) allow
    /// different providers to coexist — no need to clear the vector store.
    pub async fn switch_provider(&self, new_id: &str) -> Result<(), AlephError> {
        // Extract config, then drop the settings lock before creating the provider.
        // This avoids holding two write locks simultaneously.
        let config = {
            let settings = self.settings.read().await;
            settings
                .providers
                .iter()
                .find(|p| p.id == new_id)
                .ok_or_else(|| AlephError::config(format!("Provider not found: {new_id}")))?
                // rust-doctor-disable-next-line excessive-clone
                .clone()
        }; // settings lock released

        // Create provider before mutating settings — if this fails, nothing changes.
        let provider = create_provider(&config)?;

        // Now update both settings and active provider
        // rust-doctor-disable-next-line excessive-clone
        let old_id = self.settings.read().await.active_provider_id.clone();
        self.settings.write().await.active_provider_id = new_id.to_string();
        *self.active_provider.write().await = Some(provider);

        info!(old = %old_id, new = %new_id, "Embedding provider switched");

        Ok(())
    }

    /// Test a specific provider's connectivity
    pub async fn test_provider(&self, provider_id: &str) -> Result<(), AlephError> {
        let config = {
            let settings = self.settings.read().await;
            settings
                .providers
                .iter()
                .find(|p| p.id == provider_id)
                .ok_or_else(|| AlephError::config(format!("Provider not found: {provider_id}")))?
                // rust-doctor-disable-next-line excessive-clone
                .clone()
        }; // settings lock released here
        let provider = RemoteEmbeddingProvider::from_config(&config)?;
        provider.test_connection().await
    }

    /// Test a provider config without it being saved (for "test connection" button)
    pub async fn test_config(config: &EmbeddingProviderConfig) -> Result<(), AlephError> {
        let provider = RemoteEmbeddingProvider::from_config(config)?;
        provider.test_connection().await
    }

    /// Update the internal settings (called after config save)
    pub async fn update_settings(&self, settings: EmbeddingSettings) {
        *self.settings.write().await = settings;
    }

    /// Get a snapshot of current settings
    pub async fn get_settings(&self) -> EmbeddingSettings {
        // rust-doctor-disable-next-line excessive-clone
        self.settings.read().await.clone()
    }

    /// Enqueue a note for asynchronous embedding.
    ///
    /// Cheap — only the body text is copied. Actual `embed_batch` +
    /// `upsert_embedding` happens on the next `flush_pending` call.
    /// Producers (ingest tail, dream stage tails) are wired in B5.2.
    pub async fn push_pending(&self, agent_id: &str, path: &str, body: &str) {
        let mut q = self.pending.lock().await;
        q.push(PendingItem {
            agent_id: agent_id.to_string(),
            path: path.to_string(),
            body: body.to_string(),
        });
    }

    /// Current depth of the pending queue. Mostly for tests and observability.
    pub async fn pending_len(&self) -> usize {
        self.pending.lock().await.len()
    }

    /// Drain up to `batch_size` items from the queue, embed them via the
    /// active provider, and upsert each vector via `store.upsert_embedding`.
    /// Returns the number of items successfully written.
    ///
    /// Behaviour on degraded paths:
    /// - No active provider: drained items are pushed back to the front of
    ///   the queue (preserving order) and `Ok(0)` is returned.
    /// - `embed_batch` error: drained items are dropped (logged at warn).
    ///   The next `reembed_all` migration will rebuild their vectors.
    /// - Per-row `upsert_embedding` error: that single item is skipped
    ///   (logged at warn); successful peers are still counted.
    pub async fn flush_pending<S>(&self, store: &S, batch_size: usize) -> Result<usize, AlephError>
    where
        S: crate::memory::notes::store::NoteStore + ?Sized + Send + Sync,
    {
        // 1. Drain up to batch_size items, releasing the lock immediately.
        let drained: Vec<PendingItem> = {
            let mut q = self.pending.lock().await;
            let take = q.len().min(batch_size);
            q.drain(..take).collect()
        };
        if drained.is_empty() {
            return Ok(0);
        }

        // 2. Look up the active provider — if none, requeue and return 0.
        let provider = match self.get_active_provider().await {
            Some(p) => p,
            None => {
                let requeued = drained.len();
                let mut q = self.pending.lock().await;
                // Re-prepend so order is preserved (drained items came from
                // the front of the queue).
                let mut combined = drained;
                combined.append(&mut *q);
                *q = combined;
                tracing::debug!(
                    requeued,
                    "flush_pending: no active provider — requeued items"
                );
                return Ok(0);
            }
        };

        // 3. Embed in one batch.
        let texts: Vec<&str> = drained.iter().map(|p| p.body.as_str()).collect();
        let vectors = match provider.embed_batch(&texts).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    batch = drained.len(),
                    error = %e,
                    "flush_pending: embed_batch failed; dropped from queue (reembed_all will catch up)"
                );
                return Ok(0);
            }
        };

        // 4. Persist each vector.
        let dim = provider.dimensions() as u32;
        let mut written = 0;
        for (item, vec) in drained.iter().zip(vectors.iter()) {
            // `body` carries the full on-disk note text (see the producer in
            // `ingest::batch`), so hashing it yields the same value the note
            // index stores as `content_hash` — which is what makes the vector's
            // recorded provenance comparable to the note's current version.
            let embedded_hash = crate::memory::notes::indexer::sha2_hash(&item.body);
            if let Err(e) = store
                .upsert_embedding(&item.path, &item.agent_id, vec, dim, &embedded_hash)
                .await
            {
                tracing::warn!(
                    path = %item.path,
                    error = %e,
                    "flush_pending: upsert_embedding failed; skipping"
                );
                continue;
            }
            written += 1;
        }
        Ok(written)
    }

    /// Test-only: install a provider directly as the active one, bypassing
    /// `init()` + remote config. Used by `tests_pending` to seat a Mock
    /// provider so `flush_pending` can be exercised end-to-end.
    #[cfg(test)]
    pub(crate) async fn install_provider_for_test(&self, provider: Arc<dyn EmbeddingProvider>) {
        *self.active_provider.write().await = Some(provider);
    }
}

#[cfg(test)]
mod tests_pending {
    use super::*;
    use crate::memory::embedding_provider::tests::MockEmbeddingProvider;
    use crate::memory::notes::store::NoteStore;
    use crate::memory::store::SqliteMemoryBackend;

    fn make_store() -> Arc<SqliteMemoryBackend> {
        let path = std::env::temp_dir().join(format!("aleph_emb_{}", uuid::Uuid::new_v4()));
        Arc::new(SqliteMemoryBackend::new(&path).unwrap())
    }

    #[tokio::test]
    async fn pending_queue_push_and_len() {
        let mgr = EmbeddingManager::new(EmbeddingSettings::default());
        assert_eq!(mgr.pending_len().await, 0);
        mgr.push_pending("default", "preference/a", "body a").await;
        mgr.push_pending("default", "preference/b", "body b").await;
        assert_eq!(mgr.pending_len().await, 2);
    }

    #[tokio::test]
    async fn flush_pending_with_no_provider_requeues() {
        let mgr = EmbeddingManager::new(EmbeddingSettings::default());
        // Note: no init(), no install_provider_for_test — active_provider is None.
        mgr.push_pending("default", "preference/a", "body a").await;
        let store = make_store();
        let flushed = mgr.flush_pending(store.as_ref(), 64).await.unwrap();
        assert_eq!(flushed, 0);
        assert_eq!(
            mgr.pending_len().await,
            1,
            "items must be requeued when no provider"
        );
    }

    #[tokio::test]
    async fn flush_pending_writes_vectors() {
        let mgr = EmbeddingManager::new(EmbeddingSettings::default());
        let mock: Arc<dyn EmbeddingProvider> =
            Arc::new(MockEmbeddingProvider::new(1024, "mock-1024"));
        mgr.install_provider_for_test(mock).await;

        let store = make_store();
        mgr.push_pending("default", "preference/a", "body a").await;
        mgr.push_pending("default", "preference/b", "body b").await;
        assert_eq!(mgr.pending_len().await, 2);

        let flushed = mgr.flush_pending(store.as_ref(), 64).await.unwrap();
        assert_eq!(flushed, 2);
        assert_eq!(mgr.pending_len().await, 0);

        let v_a = store
            .get_embedding("preference/a", "default", 1024)
            .await
            .unwrap();
        let v_b = store
            .get_embedding("preference/b", "default", 1024)
            .await
            .unwrap();
        assert!(v_a.is_some(), "preference/a vector should be persisted");
        assert!(v_b.is_some(), "preference/b vector should be persisted");
    }
}
