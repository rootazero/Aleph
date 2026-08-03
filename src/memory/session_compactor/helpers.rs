use super::SessionCompactor;
use crate::error::AlephError;
use crate::memory::extensions::insert_with_capture_filter;
use crate::memory::extensions::types::CaptureCtx;
use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};
use crate::providers::adapter::RequestPayload;
use crate::providers::message::UnifiedMessage;
use crate::sync_primitives::Arc;
use tracing::{info, warn};

impl SessionCompactor {
    /// Generate a summary for the given messages at the specified depth.
    pub(crate) async fn generate_summary(
        &self,
        messages: &[(String, String)],
        depth: u32,
        previous_context: Option<&str>,
    ) -> String {
        let ratio = self.config.token_estimate_ratio;
        let source_token_count: usize = messages
            .iter()
            .map(|(_, c)| super::context_window::estimate_tokens(c, ratio))
            .sum();

        if let Some(ref provider) = self.provider {
            let prompt = super::summary_engine::build_summary_prompt(
                messages,
                depth,
                previous_context,
                super::fallback::FallbackLevel::Normal,
            );
            match self.call_llm(provider.as_ref(), &prompt).await {
                Ok(text)
                    if !text.is_empty()
                        && super::context_window::estimate_tokens(&text, ratio)
                            < source_token_count =>
                {
                    return super::summary_engine::strip_analysis_block(&text);
                }
                Ok(_) => {
                    warn!(
                        depth,
                        source_tokens = source_token_count,
                        "LLM summary (normal) was empty or not shorter than input, escalating to aggressive"
                    );
                }
                Err(e) => {
                    warn!(
                        error = %e,
                        depth,
                        "LLM summary (normal) failed, escalating to aggressive"
                    );
                }
            }

            let prompt = super::summary_engine::build_summary_prompt(
                messages,
                depth,
                previous_context,
                super::fallback::FallbackLevel::Aggressive,
            );
            match self.call_llm(provider.as_ref(), &prompt).await {
                Ok(text)
                    if !text.is_empty()
                        && super::context_window::estimate_tokens(&text, ratio)
                            < source_token_count =>
                {
                    return super::summary_engine::strip_analysis_block(&text);
                }
                Ok(_) => {
                    warn!(
                        depth,
                        source_tokens = source_token_count,
                        "LLM summary (aggressive) was empty or not shorter than input, falling back to deterministic"
                    );
                }
                Err(e) => {
                    warn!(
                        error = %e,
                        depth,
                        "LLM summary (aggressive) failed, falling back to deterministic"
                    );
                }
            }

            warn!(depth, "Using deterministic fallback for summary generation");
        }

        tracing::warn!(target: "session_compactor", depth, "fallback");
        let target = super::fallback::target_tokens(
            source_token_count,
            super::fallback::FallbackLevel::Normal,
        );
        let max_chars = (target as f64 * ratio) as usize;
        super::fallback::deterministic_truncate(messages, max_chars)
    }

    /// Call the LLM provider with a summarization prompt.
    pub(crate) async fn call_llm(
        &self,
        provider: &dyn crate::providers::AiProvider,
        prompt: &str,
    ) -> crate::error::Result<String> {
        let msgs = [UnifiedMessage::user(prompt)];
        let system = "You are a precise summarizer. Output only the summary, no preamble or meta-commentary.";
        let payload = RequestPayload::new(&msgs).with_system(Some(system));
        let response = provider.process(payload).await?;
        Ok(response.text_content())
    }

    /// Count raw memories at a given depth for a session.
    ///
    /// `agent_id` must be the same resolved id under which `post_turn_compress`
    /// and `try_condense` write the d0/d1/d2 summaries. Reading under a fixed
    /// placeholder (e.g. "default") while writes use the real agent id would
    /// always return 0, resetting the per-depth seq counter to 0 every turn and
    /// colliding on the `(agent_id, path)` unique index — silently dropping
    /// summaries and never triggering condensation.
    pub(crate) async fn count_valid_facts_at_depth(
        &self,
        session_id: &str,
        agent_id: &str,
        depth: u32,
    ) -> Result<usize, AlephError> {
        let path_prefix = format!("aleph://session/{session_id}/d{depth}/");
        let raws = self
            .database
            .get_raw_by_path_prefix(&path_prefix, agent_id, 500)
            .await?;
        Ok(raws.len())
    }

    /// Store raw conversation chunk for post-compression semantic recovery.
    ///
    /// `agent_id` must be the same resolved id under which `post_turn_compress`
    /// writes the d0/d1/d2 summaries (see [`Self::count_valid_facts_at_depth`]).
    /// This used to hardcode `"default"`, which dropped every raw chunk into the
    /// default agent's scope — `compress_to_notes` then drained them under the
    /// WRONG agent and sedimented another agent's conversation into the default
    /// agent's note wiki.
    pub async fn store_raw_chunk(
        &self,
        session_id: &str,
        agent_id: &str,
        seq: usize,
        content: &str,
    ) -> Result<(), AlephError> {
        let path = format!("aleph://session/{session_id}/raw/{seq}");
        let raw = RawMemory::new(content.to_string(), RawMemorySource::SessionCompressed)
            .with_agent(agent_id)
            .with_session(session_id)
            .with_path(path);
        if let Some(ref registry) = self.capture_registry {
            let store: Arc<dyn crate::memory::store::raw_memory::RawMemoryStore> =
                // rust-doctor-disable-next-line excessive-clone
                self.database.clone();
            let ctx = CaptureCtx {
                agent_id: agent_id.to_string(),
                namespace: crate::memory::namespace::NamespaceScope::Owner,
                session_id: Some(session_id.to_string()),
                source_hint: "session_compressed".into(),
            };
            if let Err(e) = insert_with_capture_filter(&store, registry, &ctx, raw).await {
                warn!(error = %e, session = %session_id, seq, "Failed to store raw chunk to raw_memories");
            }
        } else if let Err(e) = self.database.insert_raw_memory(&raw).await {
            warn!(
                error = %e,
                session = %session_id,
                seq,
                "Failed to store raw chunk to raw_memories"
            );
        }
        Ok(())
    }

    /// Fetch all raw memories at a given depth for a session.
    ///
    /// `agent_id` must match the id used by the writers (see
    /// [`Self::count_valid_facts_at_depth`]).
    pub(crate) async fn fetch_valid_facts_at_depth(
        &self,
        session_id: &str,
        agent_id: &str,
        depth: u32,
    ) -> Result<Vec<crate::memory::store::raw_memory::RawMemory>, AlephError> {
        let path_prefix = format!("aleph://session/{session_id}/d{depth}/");
        self.database
            .get_raw_by_path_prefix(&path_prefix, agent_id, 500)
            .await
    }

    /// Try to condense facts from `source_depth` into `target_depth`.
    pub(crate) async fn try_condense(
        &self,
        session_id: &str,
        agent_id: &str,
        source_depth: u32,
        target_depth: u32,
        _min_fanout: usize,
    ) -> Result<u32, AlephError> {
        tracing::info!(target: "session_compactor", source_depth, target_depth, "condense");

        let source_facts = self
            .fetch_valid_facts_at_depth(session_id, agent_id, source_depth)
            .await?;

        if source_facts.is_empty() {
            return Ok(0);
        }

        let messages: Vec<(String, String)> = source_facts
            .iter()
            // rust-doctor-disable-next-line excessive-clone
            .map(|r| ("assistant".to_string(), r.content.clone()))
            .collect();

        let summary_text = self.generate_summary(&messages, target_depth, None).await;

        let existing_target = self
            .count_valid_facts_at_depth(session_id, agent_id, target_depth)
            .await?;
        let seq = existing_target.min(u32::MAX as usize) as u32;

        let source_tokens: usize = messages
            .iter()
            .map(|(_, c)| {
                super::context_window::estimate_tokens(c, self.config.token_estimate_ratio)
            })
            .sum();

        let fact = super::summary_engine::summary_to_fact(
            session_id,
            target_depth,
            seq,
            summary_text,
            source_facts.len(),
            source_tokens,
            agent_id,
        );

        // rust-doctor-disable-next-line excessive-clone
        let raw = RawMemory::new(fact.content.clone(), RawMemorySource::SessionCompressed)
            .with_agent(agent_id)
            .with_session(session_id)
            // rust-doctor-disable-next-line excessive-clone
            .with_path(fact.path.clone());
        if let Some(ref registry) = self.capture_registry {
            let store: Arc<dyn crate::memory::store::raw_memory::RawMemoryStore> =
                // rust-doctor-disable-next-line excessive-clone
                self.database.clone();
            let ctx = CaptureCtx {
                agent_id: agent_id.to_string(),
                namespace: crate::memory::namespace::NamespaceScope::Owner,
                session_id: Some(session_id.to_string()),
                source_hint: "session_compressed".into(),
            };
            insert_with_capture_filter(&store, registry, &ctx, raw).await?;
        } else {
            self.database.insert_raw_memory(&raw).await?;
        }

        // rust-doctor-disable-next-line excessive-clone
        let source_ids: Vec<String> = source_facts.iter().map(|r| r.id.clone()).collect();
        if let Err(e) = self.database.mark_raw_as_processed(&source_ids).await {
            warn!(
                error = %e,
                source_count = source_ids.len(),
                "Failed to mark source raw memories as processed during condensation"
            );
        }

        info!(
            session = %session_id,
            source_depth,
            target_depth,
            source_count = source_facts.len(),
            "Condensed d{source_depth} → d{target_depth}"
        );

        Ok(1)
    }
}

#[cfg(test)]
mod tests {
    use crate::memory::session_compactor::{SessionCompactor, SessionCompactorConfig};
    use crate::memory::store::MemoryBackend;
    use crate::sync_primitives::Arc;

    /// Regression: `store_raw_chunk` used to hardcode agent `"default"` while
    /// the d0/d1/d2 writers use the resolved agent id — raw chunks from any
    /// non-default agent landed in the default agent's scope and were later
    /// compressed into the wrong agent's note wiki.
    #[tokio::test]
    async fn store_raw_chunk_writes_under_the_given_agent() {
        use crate::memory::store::raw_memory::RawMemoryStore;

        let temp = tempfile::tempdir().unwrap();
        let database: MemoryBackend =
            Arc::new(crate::memory::store::sqlite::SqliteMemoryBackend::new(temp.path()).unwrap());
        let compactor = SessionCompactor::new(database.clone(), SessionCompactorConfig::default());

        compactor
            .store_raw_chunk("sess-1", "alice", 0, "[user]: hello")
            .await
            .unwrap();

        let under_alice = database
            .get_raw_by_path_prefix("aleph://session/sess-1/raw/", "alice", 10)
            .await
            .unwrap();
        assert_eq!(
            under_alice.len(),
            1,
            "chunk lands in the resolved agent scope"
        );
        assert_eq!(under_alice[0].content, "[user]: hello");

        let under_default = database
            .get_raw_by_path_prefix("aleph://session/sess-1/raw/", "default", 10)
            .await
            .unwrap();
        assert!(
            under_default.is_empty(),
            "no chunk may leak into the hardcoded default scope"
        );
    }
}
