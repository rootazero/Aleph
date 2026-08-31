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

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex as TokioMutex;

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
///
/// `session_fact_retention_hours` and `promote_confidence_threshold` were
/// removed 2026-08-04: both were reachable from `[memory.session_compactor]`
/// and read by nothing, so an operator could set either and observe no effect —
/// and `docs/reference/memory/RAW_MEMORY.md` described the first as governing
/// retention, which it never did. There is still no time-based eviction of
/// `raw_memories`; if one is built it should introduce its own knob rather than
/// resurrect a name that spent this long meaning nothing. Removal is safe for
/// existing configs: this struct has no `deny_unknown_fields`, so TOML still
/// carrying the keys continues to parse (same precedent as `context_threshold`,
/// FEATURE_LOCATOR §2.14).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionCompactorConfig {
    pub enabled: bool,
    pub fresh_tail_count: usize,
    pub leaf_chunk_tokens: usize,
    pub d1_min_fanout: usize,
    pub d2_min_fanout: usize,
    pub max_summary_depth: u32,
    pub token_estimate_ratio: f64,
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
    /// Per-`(session_id, agent_id)` lock map. Two concurrent runs sharing
    /// the same agent id would otherwise derive the same `next_seq` from
    /// `count_valid_facts_at_depth` and race the UNIQUE constraint on the
    /// `path = d{depth}/{next_seq}` insert. The lock is acquired once at
    /// the start of `post_turn_compress` and held across the whole function
    /// body so `next_seq` derivation, insert, and any dependent reads stay
    /// serialised. Mirrors the `ingest_locks` pattern used by
    /// `CompressionService`.
    pub(crate) compress_locks: TokioMutex<HashMap<(String, String), Arc<TokioMutex<()>>>>,
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
///
/// Uses the LAST `/d` occurrence so a session id that itself contains `/d`
/// (e.g. `agent:main:main` keys) can never shadow the real depth segment —
/// same semantic as the parallel helper in
/// `session_search_summary::end_hook::extract_depth_from_path`.
pub(crate) fn extract_depth(path: &str) -> u32 {
    path.rfind("/d")
        .and_then(|idx| path[idx + 2..].split_once('/'))
        .and_then(|(d, _)| d.parse::<u32>().ok())
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
