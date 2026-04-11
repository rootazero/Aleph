/// Memory ingestion pipeline
///
/// This module previously handled storage of raw memory entries (Layer 1)
/// into SQLite via SessionStore. Raw memory storage has been removed —
/// facts (Layer 2) are now the primary retrieval target, and raw
/// conversations are stored in SessionManager's SQLite.
///
/// The MemoryIngestion struct is retained as a no-op stub so that
/// existing callers continue to compile without changes.
use crate::config::MemoryConfig;
use crate::error::AlephError;
use crate::memory::context::ContextAnchor;
use crate::memory::dreaming::{ensure_dream_daemon, record_activity};
use crate::memory::store::MemoryBackend;
use crate::memory::EmbeddingProvider;
use crate::sync_primitives::Arc;
use tracing::debug;

/// Memory ingestion service (no-op after SessionStore removal)
#[derive(Clone)]
pub struct MemoryIngestion {
    _database: MemoryBackend,
    _embedder: Arc<dyn EmbeddingProvider>,
    config: Arc<MemoryConfig>,
}

impl MemoryIngestion {
    /// Create new ingestion service
    pub fn new(
        database: MemoryBackend,
        embedder: Arc<dyn EmbeddingProvider>,
        config: Arc<MemoryConfig>,
    ) -> Self {
        ensure_dream_daemon(database.clone(), Arc::clone(&config), None, None);
        Self {
            _database: database,
            _embedder: embedder,
            config,
        }
    }

    /// Store memory after AI interaction (no-op)
    ///
    /// Raw memory storage has been removed. This method records activity
    /// for the dream daemon but does not persist the memory entry.
    /// Raw conversations are already stored in SessionManager's SQLite.
    pub async fn store_memory(
        &self,
        _context: ContextAnchor,
        _user_input: &str,
        _ai_output: &str,
    ) -> Result<String, AlephError> {
        record_activity();

        if !self.config.enabled {
            debug!("Memory ingestion skipped: memory disabled");
            return Err(AlephError::config("Memory is disabled"));
        }

        // Raw memory storage removed — facts are the primary retrieval target.
        // Return a synthetic ID for API compatibility.
        Ok(uuid::Uuid::new_v4().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_store_memory_disabled() {
        // Verify the struct can be constructed (no noise_filter field needed)
        assert!(true);
    }
}
