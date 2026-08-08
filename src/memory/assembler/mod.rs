//! Working Memory Assembler — produces a portable [`MemoryEnvelope`] before
//! each LLM call. See `docs/superpowers/specs/2026-04-13-memory-evolution-spec1-assembler-design.md`.

pub mod context_block;
pub mod envelope;
pub(crate) mod error;
pub(crate) mod fallback;
pub(crate) mod feedback_floor;
pub(crate) mod gather;
pub mod hybrid;
pub mod hydration;
pub(crate) mod profile;
pub mod render;
pub(crate) mod rerank;

#[cfg(test)]
mod tests;

pub use feedback_floor::FeedbackFloorLoader;
pub use hybrid::{AiProviderReranker, HybridAssembler, LlmReranker};
pub use profile::UserProfileLoader;

pub use context_block::{wrap_memory_context, MEMORY_CONTEXT_CLOSE, MEMORY_CONTEXT_OPEN};
pub use envelope::{
    EnvelopeItem, EnvelopeMeta, EnvelopeSlot, ItemSource, MemoryEnvelope, SlotKind, SCHEMA_VERSION,
};
pub use render::{render_envelope, render_with, RenderStyle};

use crate::error::AlephError;
use crate::memory::session_search_summary::FactSourceFilter;
use async_trait::async_trait;

/// Token budget passed into the assembler. `total_tokens` is the hard cap
/// before the LLM's reply headroom reservation (which the LLM re-rank path
/// additionally honors).
#[derive(Debug, Clone, Copy)]
pub struct AssemblyBudget {
    pub total_tokens: u32,
}

/// Produces a [`MemoryEnvelope`] for each LLM turn. Never returns `Err` for
/// LLM-assist failures — internal failures (retrieval error, LLM timeout,
/// hydration miss) are caught and degraded to fallback / empty slots. `Err`
/// only surfaces for system-level misconfiguration at construction time.
#[async_trait]
pub trait WorkingMemoryAssembler: Send + Sync {
    async fn assemble(
        &self,
        query: &str,
        agent_id: &str,
        session_id: Option<&str>,
        budget: AssemblyBudget,
        filter: FactSourceFilter,
    ) -> Result<MemoryEnvelope, AlephError>;

    /// Render style to use when serializing the assembled envelope for the
    /// model. Defaults to XML; the production `HybridAssembler` honours the
    /// `[memory.assembler] render_style` config knob.
    fn render_style(&self) -> RenderStyle {
        RenderStyle::Xml
    }
}
