//! Async memory context provider — fetches relevant memories before prompt assembly.

use crate::config::types::memory::MemoryInjectionMode;
use crate::memory::assembler::hybrid::LlmReranker;
use crate::memory::assembler::WorkingMemoryAssembler;
use crate::memory::curated::{CuratedConfig, CuratedMemoryStore, CuratedSnapshot};
use crate::sync_primitives::Arc;
use async_trait::async_trait;
use dashmap::DashMap;
use std::collections::HashMap;
use tokio::sync::RwLock as TokioRwLock;

/// Configuration for memory context retrieval.
pub struct MemoryContextConfig {
    /// Maximum number of facts to retrieve.
    pub max_facts: usize,
    /// Minimum cosine similarity threshold.
    pub similarity_threshold: f32,
    /// Maximum characters for the formatted output.
    pub max_output_chars: usize,
}

impl Default for MemoryContextConfig {
    fn default() -> Self {
        Self {
            max_facts: 5,
            similarity_threshold: 0.3,
            max_output_chars: 8000, // ~2000 tokens
        }
    }
}

/// No-op reranker used when no [`AiProvider`] is supplied. Always errors →
/// `HybridAssembler` transparently falls back to the deterministic skeleton.
struct NoopReranker;

#[async_trait]
impl LlmReranker for NoopReranker {
    async fn complete(
        &self,
        _prompt: &str,
        _model: Option<&str>,
    ) -> Result<String, crate::error::AlephError> {
        Err(crate::error::AlephError::config(
            "NoopReranker: no AiProvider configured".to_string(),
        ))
    }
}

/// Per-(agent_id, `session_key`) curated snapshot cache. Frozen until
/// invalidation; see [`MemoryContextProvider::build_curated_message`].
type CuratedSnapshotCache = Arc<TokioRwLock<HashMap<(String, String), Arc<CuratedSnapshot>>>>;

/// Provides pre-fetched memory context for prompt injection.
pub struct MemoryContextProvider {
    pub(crate) assembler: Arc<dyn WorkingMemoryAssembler>,
    pub(crate) config: MemoryContextConfig,
    /// Controls whether memory is auto-injected (Context/Hybrid) or gated behind tools (Tools).
    pub(crate) injection_mode: MemoryInjectionMode,
    /// Plugin-contributed enhancements to the retrieved envelope.
    /// Default-empty registry means no plugins registered = no-op.
    pub(crate) extensions:
        crate::sync_primitives::Arc<crate::memory::extensions::MemoryExtensionRegistry>,
    /// Optional wiki orientation provider for injecting structural context.
    pub(crate) orientation: Option<Arc<dyn crate::memory::notes::orientation::NoteOrientation>>,
    /// Token budget for orientation snapshots.
    pub(crate) orientation_budget: crate::memory::notes::orientation::types::TokenBudget,
    /// Optional user-profile synthesizer for injecting profile context.
    pub(crate) profile: Option<Arc<dyn crate::memory::notes::profile::ProfileSynthesizer>>,
    /// Per-(agent_id, `session_key`) frozen snapshot. Built on first prompt
    /// build for the session; reused until evicted by compression / `SessionEnd`.
    pub(crate) curated_snapshots: CuratedSnapshotCache,
    /// Per-agent `CuratedMemoryStore`. Loaded lazily on first capture.
    pub(crate) curated_stores: Arc<DashMap<String, Arc<CuratedMemoryStore>>>,
    /// Char-budget config for both MEMORY.md and USER.md rendering.
    pub(crate) curated_config: CuratedConfig,
    /// Test-only override for the curated MEMORY.md root directory.
    /// Real path: `~/.aleph/agents/<agent_id>/MEMORY.md`. Tests redirect
    /// to a tempdir to keep filesystem state isolated.
    #[cfg(test)]
    pub(crate) curated_root_override: Option<std::path::PathBuf>,
}

mod constructor;
mod curated;
mod helpers;
mod memory;
mod orientation;
mod profile;

pub use helpers::*;

#[cfg(test)]
mod tests;
