use super::MemoryContextProvider;
use crate::config::types::memory::MemoryInjectionMode;
use crate::memory::assembler::context_block::wrap_memory_context;
use crate::memory::assembler::render::render_with;
use crate::memory::assembler::AssemblyBudget;
use crate::providers::message::UnifiedMessage;

/// Floor below which we never trim the memory-injection budget as long as the
/// context has *any* meaningful room — a high-value note or two must still
/// reach the model under pressure rather than being dropped wholesale
/// ("memory always keeps a seat at the table"). Only when the available
/// headroom is itself below this floor do we hand over whatever little fits.
const MIN_MEMORY_INJECTION_TOKENS: u32 = 512;

/// Reconcile the configured memory-injection budget with the live context
/// headroom (pure, so it is unit-testable in isolation from the assembler).
///
/// * `target` — the configured desired budget (char-budget → token-budget).
/// * `available` — `Some(headroom)` when a `[context_budget]` is active and the
///   caller computed how many tokens the window can spare for memory this turn;
///   `None` when no context budget is configured.
///
/// Returns `target` unchanged when there is no context budget, or when headroom
/// comfortably exceeds the target (the common low-pressure case — byte-for-byte
/// the legacy behaviour). Under pressure it backs the budget off toward the
/// available headroom, but never below `min(MIN_MEMORY_INJECTION_TOKENS,
/// available)` so memory keeps a seat. It never grows past `target`, keeping
/// token usage bounded. This is the coordination none of the reference agents
/// (hermes / openclaw / Pi / opensquilla) perform — they inject memory at a
/// fixed size regardless of how full the conversation already is.
fn effective_memory_budget(target: u32, available: Option<u32>) -> u32 {
    match available {
        None => target,
        Some(avail) => {
            let floor = MIN_MEMORY_INJECTION_TOKENS.min(avail);
            target.clamp(floor, avail)
        }
    }
}

impl MemoryContextProvider {
    /// Build a memory user-message for injection into the prompt.
    ///
    /// Returns `Ok(None)` when injection is disabled (`Tools` mode) or when
    /// the assembler returned an empty envelope. Otherwise returns the rendered
    /// envelope wrapped by [`wrap_memory_context`] in a guarded
    /// `<memory-context>` fence carrying a "reference data, not user input"
    /// system note.
    ///
    /// `session_id` is the current session key: the assembler threads it into
    /// its gather stage, where the prior-session snapshot source excludes it —
    /// without it a resumed session gets its OWN end-of-session snapshot
    /// injected back as "previous session" context. Pass `None` only when no
    /// session is active (retrieval then excludes nothing).
    ///
    /// `available_tokens` is the context-budget-aware headroom for memory this
    /// turn (see [`effective_memory_budget`]). Pass `None` to use the full
    /// configured budget (legacy behaviour / no `[context_budget]`).
    pub async fn build_memory_user_message(
        &self,
        agent_id: &str,
        query: &str,
        session_id: Option<&str>,
        available_tokens: Option<u32>,
    ) -> Result<Option<UnifiedMessage>, crate::error::AlephError> {
        match self.injection_mode {
            MemoryInjectionMode::Tools => return Ok(None),
            MemoryInjectionMode::Context | MemoryInjectionMode::Hybrid => {}
        }

        // Convert char-budget to token-budget using the WORST-CASE chars/token
        // ratio (CJK ≈ 1.5 chars/tok). The English-only ratio (~4 chars/tok)
        // would severely under-allocate the token budget for CJK content,
        // causing the assembler to include far fewer memory entries than the
        // configured char budget actually allows. The 2/3 form is integer
        // math for `max_output_chars / 1.5`.
        let target_tokens = (self.config.max_output_chars as u64)
            .saturating_mul(2)
            .saturating_div(3)
            .min(u64::from(u32::MAX)) as u32;
        // Coordinate with the per-turn context budget: memory is delivered as
        // a transient trailing recall message that rides the fresh tail
        // (compaction-protected), so an oversized injection still forces the
        // in-loop compactor to over-trim the rest of recent history. Back the
        // budget off to the headroom the window can spare, floored so memory
        // still lands.
        let budget = AssemblyBudget {
            total_tokens: effective_memory_budget(target_tokens, available_tokens),
        };
        let mut envelope = self
            .assembler
            .assemble(
                query,
                agent_id,
                session_id,
                budget,
                crate::memory::session_search_summary::FactSourceFilter::Any,
            )
            .await?;

        let ext_ctx = crate::memory::extensions::RetrieveCtx {
            agent_id: agent_id.to_string(),
            namespace: crate::memory::namespace::NamespaceScope::Owner,
            query: query.to_string(),
            session_id: session_id.map(str::to_string),
        };
        if let Err(e) = self
            .extensions
            .dispatch_on_retrieve(&ext_ctx, &mut envelope)
            .await
        {
            tracing::warn!("memory extensions on_retrieve pipeline failed: {e}");
        }

        let rendered = render_with(&envelope, self.assembler.render_style());
        if rendered.trim().is_empty() {
            return Ok(None);
        }
        // Wrap recalled memory in the guarded `<memory-context>` fence + system
        // note so the model treats it as reference data, not new user input
        // (hermes-parity prompt-injection hardening). The same fence tags are
        // stripped from streamed output by the gateway drain's
        // `StreamingContextScrubber` (see `execution_engine::event_drain`).
        Ok(Some(UnifiedMessage::user(wrap_memory_context(&rendered))))
    }
}

// Spec A Task 18 — bridge MemoryContextProvider into the compression
// pipeline so cached `<CuratedMemory>` snapshots are evicted after
// compression rewrites MEMORY.md / USER.md on disk.
impl crate::memory::compression::PostCompressionHook for MemoryContextProvider {
    fn on_compression_complete<'a>(
        &'a self,
        agent_id: &'a str,
    ) -> futures::future::BoxFuture<'a, ()> {
        Box::pin(async move {
            self.invalidate_curated_for_agent(agent_id).await;
        })
    }
}

#[cfg(test)]
mod budget_tests {
    use super::{effective_memory_budget, MIN_MEMORY_INJECTION_TOKENS};

    #[test]
    fn no_context_budget_uses_full_target() {
        // None headroom (no `[context_budget]`) → legacy behaviour, byte-identical.
        assert_eq!(effective_memory_budget(5333, None), 5333);
    }

    #[test]
    fn ample_headroom_keeps_target_unchanged() {
        // Plenty of room → never grows past the configured target (token control).
        assert_eq!(effective_memory_budget(5333, Some(50_000)), 5333);
        assert_eq!(effective_memory_budget(5333, Some(5333)), 5333);
    }

    #[test]
    fn tight_headroom_backs_off_to_available() {
        // Between the floor and the target → trim memory to fit the window.
        assert_eq!(effective_memory_budget(5333, Some(3000)), 3000);
    }

    #[test]
    fn very_tight_headroom_floors_at_what_fits() {
        // Below the floor but non-zero → hand over whatever little fits.
        assert_eq!(effective_memory_budget(5333, Some(300)), 300);
        // Above the floor → memory keeps its guaranteed seat even if target is huge.
        assert_eq!(
            effective_memory_budget(50_000, Some(MIN_MEMORY_INJECTION_TOKENS)),
            MIN_MEMORY_INJECTION_TOKENS
        );
    }

    #[test]
    fn zero_headroom_yields_zero() {
        // Window already past the warning line → inject nothing this turn; the
        // assembler returns an empty envelope and memory degrades to `None`.
        assert_eq!(effective_memory_budget(5333, Some(0)), 0);
    }
}
