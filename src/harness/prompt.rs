//! Prompt Assembly Seam — Stage 3 of the 12-module harness roadmap.
//!
//! `PromptBuilder` is the single seam through which `AgentHarness` produces
//! the per-turn `Vec<UnifiedMessage>` handed to the provider. Default
//! behavior matches the legacy private `build_prompt` byte-for-byte;
//! downstream stages (#11 Subagent, #10 Verification) inject custom
//! builders that compose memory hints, chain context, or judge prompts
//! without patching `agent.rs`.

use async_trait::async_trait;

use crate::providers::message::UnifiedMessage;
use crate::session::events::SessionEventRecord;

/// Input to `PromptBuilder::assemble`. Carries the slice of session events
/// and the tail boundary computed by `tail_start_index`. Future stages may
/// extend this struct with memory hints, skill suggestions, or chain
/// context — additions must be additive (existing builders keep working).
#[derive(Debug)]
pub struct TurnContext<'a> {
    pub events: &'a [SessionEventRecord],
    pub tail_start: usize,
}

impl<'a> TurnContext<'a> {
    pub fn new(events: &'a [SessionEventRecord], tail_start: usize) -> Self {
        Self { events, tail_start }
    }
}

/// Pluggable per-turn message assembler. Implementations must be
/// `Send + Sync` so `Arc<dyn PromptBuilder>` lives in `HarnessDeps`.
#[async_trait]
pub trait PromptBuilder: Send + Sync {
    /// Produce the `Vec<UnifiedMessage>` for the next provider call.
    /// Errors propagate as `HarnessError::Session` (or future variants).
    async fn assemble(
        &self,
        ctx: &TurnContext<'_>,
    ) -> Result<Vec<UnifiedMessage>, crate::harness::trait_def::HarnessError>;
}

/// Default builder — byte-equivalent to the pre-Stage-3 private
/// `build_prompt` function (former `agent.rs:846`).
#[derive(Debug, Default, Clone)]
pub struct DefaultPromptBuilder;

#[async_trait]
impl PromptBuilder for DefaultPromptBuilder {
    async fn assemble(
        &self,
        ctx: &TurnContext<'_>,
    ) -> Result<Vec<UnifiedMessage>, crate::harness::trait_def::HarnessError> {
        // Body filled in Task 3.
        let _ = ctx;
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn default_builder_compiles_and_runs() {
        let events: Vec<SessionEventRecord> = Vec::new();
        let ctx = TurnContext::new(&events, 0);
        let builder = DefaultPromptBuilder;
        let out = builder.assemble(&ctx).await.expect("assemble ok");
        assert!(out.is_empty(), "empty events → empty output");
    }
}
