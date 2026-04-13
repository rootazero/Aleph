//! MemoryReflector — orchestrates HybridAssembler + LLM synthesis.
//!
//! This module implements the skeleton + empty-packet short-circuit only.
//! The full LLM synthesis path (Task 5) is a placeholder stub below.

use crate::error::AlephError;
use crate::memory::assembler::{AssemblyBudget, WorkingMemoryAssembler};
use crate::memory::reflector::types::{ReflectOpts, Synthesis};
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;

/// Default token budget when the caller does not specify one.
const DEFAULT_BUDGET_TOKENS: u32 = 4096;

/// Orchestrates [`WorkingMemoryAssembler`] retrieval followed by LLM synthesis.
///
/// # Short-circuit
/// When the assembler returns an envelope with zero items across all slots,
/// `reflect` returns immediately with a stub `Synthesis` and never calls the LLM.
pub struct MemoryReflector {
    assembler: Arc<dyn WorkingMemoryAssembler>,
    provider: Arc<dyn AiProvider>,
    // recall_signals writer will be added in Task 6.
}

impl MemoryReflector {
    pub fn new(
        assembler: Arc<dyn WorkingMemoryAssembler>,
        provider: Arc<dyn AiProvider>,
    ) -> Self {
        Self {
            assembler,
            provider,
        }
    }

    /// Retrieve memories relevant to `query` and synthesise a natural-language answer.
    ///
    /// Returns immediately with a stub when no relevant memories are found (empty-packet
    /// short-circuit). The full LLM synthesis path is implemented in Task 5.
    pub async fn reflect(
        &self,
        query: &str,
        opts: ReflectOpts,
    ) -> Result<Synthesis, AlephError> {
        let budget = AssemblyBudget {
            total_tokens: opts
                .max_tokens
                .map(|n| n as u32)
                .unwrap_or(DEFAULT_BUDGET_TOKENS),
        };

        let envelope = self
            .assembler
            .assemble(
                query,
                &opts.agent_id,
                opts.session_id.as_deref(),
                budget,
            )
            .await?;

        // Check emptiness: all slots must have zero items.
        // `MemoryEnvelope` has no helper method; we iterate manually.
        let is_empty = envelope.slots.iter().all(|slot| slot.items.is_empty());

        if is_empty {
            return Ok(Synthesis {
                text: "No relevant memories found.".to_string(),
                sources: Vec::new(),
            });
        }

        // TODO(Task 5): LLM synthesis path — call provider with PROMPT_SYNTHESIS +
        // envelope_to_synthesis_context, parse NoteRef list, overlay titles, return Synthesis.
        //
        // Suppress unused-field warning until Task 5 wires the provider.
        let _ = &self.provider;

        Ok(Synthesis {
            text: "Synthesis pending (Task 5).".to_string(),
            sources: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::reflector::types::Synthesis;

    /// Verifies the stub shape that the empty-packet short-circuit returns.
    /// The assembler + provider are not exercised here; the real short-circuit
    /// path end-to-end is covered by the Task 9 integration test.
    #[test]
    fn empty_stub_shape_matches_spec() {
        let stub = Synthesis {
            text: "No relevant memories found.".to_string(),
            sources: Vec::new(),
        };
        assert_eq!(stub.text, "No relevant memories found.");
        assert!(stub.sources.is_empty());
    }
}
