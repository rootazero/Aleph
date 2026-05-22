use super::MemoryContextProvider;
use crate::config::types::memory::MemoryInjectionMode;
use crate::memory::assembler::render::{render_with, RenderStyle};
use crate::memory::assembler::AssemblyBudget;
use crate::providers::message::UnifiedMessage;

impl MemoryContextProvider {
    /// Build a memory user-message for injection into the prompt.
    ///
    /// Returns `Ok(None)` when injection is disabled (`Tools` mode) or when
    /// the assembler returned an empty envelope. Otherwise returns
    /// `Ok(Some(UnifiedMessage::user(render_with(&env, RenderStyle::Xml))))`.
    pub async fn build_memory_user_message(
        &self,
        agent_id: &str,
        query: &str,
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
        let budget = AssemblyBudget {
            total_tokens: (self.config.max_output_chars as u64)
                .saturating_mul(2)
                .saturating_div(3) as u32,
        };
        let mut envelope = self
            .assembler
            .assemble(
                query,
                agent_id,
                None,
                budget,
                crate::memory::session_search_summary::FactSourceFilter::Any,
            )
            .await?;

        let ext_ctx = crate::memory::extensions::RetrieveCtx {
            agent_id: agent_id.to_string(),
            namespace: crate::memory::namespace::NamespaceScope::Owner,
            query: query.to_string(),
            session_id: None,
        };
        if let Err(e) = self
            .extensions
            .dispatch_on_retrieve(&ext_ctx, &mut envelope)
            .await
        {
            tracing::warn!("memory extensions on_retrieve pipeline failed: {e}");
        }

        let rendered = render_with(&envelope, RenderStyle::Xml);
        if rendered.trim().is_empty() {
            return Ok(None);
        }
        Ok(Some(UnifiedMessage::user(rendered)))
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
