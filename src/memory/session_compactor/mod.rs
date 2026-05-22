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

use crate::sync_primitives::AtomicU64;

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
pub mod tool_compactor;

pub use summary_source::SessionSummarySource;

mod constructor;
mod helpers;
mod post_turn_compress;
mod prepare_history;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod chunker_tests;

// ---------------------------------------------------------------------------
// CompactorMetrics
// ---------------------------------------------------------------------------

/// Atomic counters for observing session compactor activity.
///
/// All fields use `Relaxed` ordering — these are best-effort counters with no
/// cross-thread ordering requirements.
#[derive(Debug, Default)]
pub struct CompactorMetrics {
    pub tool_compactions: AtomicU64,
    pub d0_summaries_created: AtomicU64,
    pub d1_condensations: AtomicU64,
    pub d2_condensations: AtomicU64,
    pub fallback_count: AtomicU64,
    pub prepare_history_calls: AtomicU64,
}

// ---------------------------------------------------------------------------
// SessionCompactorConfig
// ---------------------------------------------------------------------------

/// Configuration for the SessionCompactor subsystem.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionCompactorConfig {
    pub enabled: bool,
    pub fresh_tail_count: usize,
    pub context_threshold: f64,
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
            context_threshold: 0.75,
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
    pub(crate) metrics: Arc<CompactorMetrics>,
    pub(crate) indexer: Option<crate::memory::transcript_indexer::TranscriptIndexer>,
    pub(crate) raw_memory_writer: Option<Arc<dyn RawMemoryStore>>,
    pub(crate) capture_registry: Option<Arc<MemoryExtensionRegistry>>,
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

// ---------------------------------------------------------------------------
// CompactionStrategy impl
// ---------------------------------------------------------------------------

use crate::context::compact::{
    CompactionContext, CompactionResult, CompactionStrategy, PressureLevel, TokenEstimate,
};
use std::future::Future;
use std::pin::Pin;

impl CompactionStrategy for SessionCompactor {
    fn name(&self) -> &str {
        "session_compactor"
    }

    fn estimate_savings(&self, ctx: &CompactionContext) -> TokenEstimate {
        let total = ctx.pressure.used_tokens;
        let fresh_ratio = ctx.fresh_tail_count as f64 / ctx.messages.len().max(1) as f64;
        let compressible = (total as f64 * (1.0 - fresh_ratio)) as usize;
        TokenEstimate {
            estimated_savings: (compressible as f64 * 0.65) as usize,
            confidence: 0.6,
        }
    }

    fn execute<'a>(
        &'a self,
        ctx: &'a mut CompactionContext,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<CompactionResult>> + Send + 'a>> {
        Box::pin(async move {
            let before = ctx.pressure.ratio;
            Ok(CompactionResult {
                freed_tokens: 0,
                compacted_count: 0,
                strategy_name: self.name().to_string(),
                pressure_before: before,
                pressure_after: before,
            })
        })
    }

    fn is_applicable(&self, ctx: &CompactionContext) -> bool {
        ctx.pressure_level >= PressureLevel::Warning && self.config.enabled
    }
}
