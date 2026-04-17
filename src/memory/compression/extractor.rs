//! Fact Extractor
//!
//! Minimal shim retained for the embedding accessor used by `CompressionService`.
//! All extraction entry points have moved to `CompoundIngestor` (Spec 6).

use crate::memory::EmbeddingProvider;
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;

/// Extracts facts from conversations using LLM.
///
/// All active extraction entry points are now in `CompoundIngestor`.
/// This struct is retained only to hold the `embedder` accessor used by
/// `CompressionService` during legacy-path note embedding.
#[allow(dead_code)]
pub struct FactExtractor {
    provider: Arc<dyn AiProvider>,
    embedder: Arc<dyn EmbeddingProvider>,
}

impl FactExtractor {
    /// Create a new fact extractor
    pub fn new(provider: Arc<dyn AiProvider>, embedder: Arc<dyn EmbeddingProvider>) -> Self {
        Self { provider, embedder }
    }

    /// Access the embedding provider.
    pub fn embedder(&self) -> &Arc<dyn EmbeddingProvider> {
        &self.embedder
    }
}
