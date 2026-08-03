//! Session Compactor — intra-session context management.
//!
//! Prevents token overflow in long conversations by summarizing tool call
//! sequences and older turns, while preserving a fresh tail of recent
//! messages verbatim.
//!
//! The [`SessionCompactor`] orchestrates the full lifecycle:
//! - **`prepare_history()`** — assembles compressed history + fresh tail for the
//!   agent loop, injecting prior summaries as `<session_context>` XML blocks.
//! - **`post_turn_compress()`** — runs asynchronously after each agent turn to
//!   chunk compressible messages, generate d0 summaries, and trigger hierarchical
//!   condensation (d0→d1→d2) when fanout thresholds are met.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::memory::extensions::MemoryExtensionRegistry;
use crate::memory::store::raw_memory::RawMemoryStore;
use crate::memory::store::MemoryBackend;
use crate::providers::message::UnifiedMessage;
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;

pub mod context_window;
pub mod fallback;
pub mod summary_engine;
pub mod summary_source;

pub use summary_source::SessionSummarySource;

mod constructor;
mod helpers;
mod post_turn_compress;
mod prepare_history;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod chunker_tests;

// NOTE: `CompactorMetrics` used to live here — five atomics incremented on
// every compaction, with no `load()` anywhere and an accessor
// (`SessionCompactor::metrics()`) that had no callers. Removed per R10 YAGNI:
// zero real consumers ⇒ CUT, not CONNECT.
//
// Deliberately NOT "rescued" by wiring a reader. These counted *memory-layer*
// fact condensation (d0/d1/d2), not the `ContextCompactor` rewrites in
// `src/context/compact/` that actually break the provider prompt prefix — so a
// reader would have produced a plausible-looking number about the wrong
// subsystem. If a prefix-break counter is ever wanted, it belongs on
// `ContextCompactor::compact`'s outcome, where `CompactStrategy::{Skipped,
// CacheReuse}` are already distinguished from the rewriting strategies.

// ---------------------------------------------------------------------------
// SessionCompactorConfig
// ---------------------------------------------------------------------------

/// Configuration for the `SessionCompactor` subsystem.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionCompactorConfig {
    pub enabled: bool,
    pub fresh_tail_count: usize,
    pub leaf_chunk_tokens: usize,
    pub d1_min_fanout: usize,
    pub d2_min_fanout: usize,
    pub max_summary_depth: u32,
    pub token_estimate_ratio: f64,
    pub session_fact_retention_hours: u64,
    pub promote_confidence_threshold: f32,
}

impl Default for SessionCompactorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            fresh_tail_count: 20,
            leaf_chunk_tokens: 1000,
            d1_min_fanout: 4,
            d2_min_fanout: 3,
            max_summary_depth: 2,
            token_estimate_ratio: 3.5,
            session_fact_retention_hours: 24,
            promote_confidence_threshold: 0.8,
        }
    }
}

// ---------------------------------------------------------------------------
// SessionCompactor
// ---------------------------------------------------------------------------

/// Central orchestrator for intra-session context compression.
///
/// Ties together [`context_window`], [`summary_engine`], and [`fallback`]
/// to keep long conversations within the model's token budget.
pub struct SessionCompactor {
    pub(crate) database: MemoryBackend,
    pub(crate) provider: Option<Arc<dyn AiProvider>>,
    pub(crate) config: SessionCompactorConfig,
    pub(crate) indexer: Option<crate::memory::transcript_indexer::TranscriptIndexer>,
    pub(crate) raw_memory_writer: Option<Arc<dyn RawMemoryStore>>,
    pub(crate) capture_registry: Option<Arc<MemoryExtensionRegistry>>,
    /// Mirrors `MemoryConfig.project_scoped`. When on, post-turn session
    /// memory (pre-compress + d0/d1/d2 summaries + transcript index) is
    /// written under the active project's composed agent id so a session's
    /// recall stays project-local. Default-off → base id → unchanged.
    pub(crate) project_scoped: bool,
}

// ---------------------------------------------------------------------------
// CompressResult
// ---------------------------------------------------------------------------

/// Summary of what `post_turn_compress` produced.
#[derive(Debug, Clone, Default)]
pub struct CompressResult {
    pub d0_created: u32,
    pub d1_created: u32,
    pub d2_created: u32,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract the depth level from a session compactor VFS path.
///
/// Path format: `aleph://session/{id}/d{depth}/{seq}`
pub(crate) fn extract_depth(path: &str) -> u32 {
    path.split("/d")
        .nth(1)
        .and_then(|s| s.split('/').next())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// Convert an [`AgentInstance`] session message to a [`UnifiedMessage`].
pub(crate) fn session_message_to_unified(
    msg: &crate::gateway::agent_instance::SessionMessage,
) -> UnifiedMessage {
    use crate::gateway::agent_instance::MessageRole;
    match msg.role {
        MessageRole::User => UnifiedMessage::user(&msg.content),
        MessageRole::Assistant => UnifiedMessage::assistant(&msg.content),
        MessageRole::System => UnifiedMessage::user(format!("[system] {}", msg.content)),
        MessageRole::Tool => UnifiedMessage::user(format!("[tool] {}", msg.content)),
    }
}
