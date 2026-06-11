use super::{CompressResult, SessionCompactor};
use crate::error::AlephError;
use crate::gateway::agent_instance::AgentInstance;
use crate::gateway::router::SessionKey;
use crate::memory::extensions::insert_with_capture_filter;
use crate::memory::extensions::types::CaptureCtx;
use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};
use crate::sync_primitives::{Arc, Ordering};
use tracing::{info, warn};

impl SessionCompactor {
    /// Run compression after an agent loop turn completes.
    pub async fn post_turn_compress(
        &self,
        agent: &AgentInstance,
        session_key: &SessionKey,
    ) -> Result<CompressResult, AlephError> {
        if !self.config.enabled {
            return Ok(CompressResult::default());
        }

        let session_id = session_key.to_key_string();
        tracing::info!(target: "session_compactor", session = %session_id, "compress_start");
        // Single chokepoint: resolve the (optionally project-scoped) storage
        // agent id here, while still synchronous in the run-loop task where the
        // `current_project_root()` task-local is live. Every downstream session
        // write (pre-compress, d0/d1/d2 summaries, transcript index) — including
        // the spawned pre-compress task — inherits this resolved id, so the
        // task-local is never read across a `tokio::spawn` boundary.
        let agent_id = crate::memory::project_scope::scoped_or_base(
            agent.id(),
            self.project_scoped,
            crate::projects::current_project_root().as_deref(),
        );
        let ratio = self.config.token_estimate_ratio;

        let raw_messages = agent.get_history(session_key, None).await;

        let pairs: Vec<(String, String)> = raw_messages
            .iter()
            .map(|m| {
                let role = match m.role {
                    crate::gateway::agent_instance::MessageRole::User => "user",
                    crate::gateway::agent_instance::MessageRole::Assistant => "assistant",
                    crate::gateway::agent_instance::MessageRole::System => "system",
                    crate::gateway::agent_instance::MessageRole::Tool => "tool",
                };
                (role.to_string(), m.content.clone())
            })
            .collect();

        let split =
            super::context_window::partition_fresh_tail_pairs(&pairs, self.config.fresh_tail_count);
        if split == 0 {
            return Ok(CompressResult::default());
        }

        let compressible = &pairs[..split];

        let compressible_messages: Vec<crate::providers::message::UnifiedMessage> = compressible
            .iter()
            .map(|(role, content)| {
                if role == "assistant" {
                    crate::providers::message::UnifiedMessage::assistant(content)
                } else {
                    crate::providers::message::UnifiedMessage::user(content)
                }
            })
            .collect();

        let units = crate::context::compact::tool_aware_chunker::parse_semantic_units(
            &compressible_messages,
        );
        let chunker = crate::context::compact::tool_aware_chunker::ToolAwareChunker::new(
            self.config.leaf_chunk_tokens,
            ratio,
        );
        let semantic_chunks =
            chunker.chunk(&units, &compressible_messages, compressible_messages.len());

        if semantic_chunks.is_empty() {
            return Ok(CompressResult::default());
        }

        let existing_d0 = self
            .count_valid_facts_at_depth(&session_id, &agent_id, 0)
            .await?;
        let mut next_seq = existing_d0.min(u32::MAX as usize) as u32;
        let mut d0_created = 0u32;

        for semantic_chunk in &semantic_chunks {
            let chunk: Vec<(String, String)> = semantic_chunk
                .message_indices()
                .into_iter()
                .filter_map(|idx| compressible.get(idx).cloned())
                .collect();

            if chunk.is_empty() {
                continue;
            }

            if let Some(ref writer) = self.raw_memory_writer {
                let raw_text: String = chunk
                    .iter()
                    .map(|(role, text)| format!("[{role}]: {text}"))
                    .collect::<Vec<_>>()
                    .join("\n\n");
                if !raw_text.is_empty() {
                    let raw = RawMemory::new(raw_text, RawMemorySource::PreCompress)
                        .with_agent(&agent_id)
                        .with_session(&session_id);
                    let writer = writer.clone();
                    let registry_opt = self.capture_registry.clone();
                    let cap_agent = agent_id.clone();
                    let cap_session = session_id.clone();
                    tokio::spawn(async move {
                        if let Some(registry) = registry_opt {
                            let ctx = CaptureCtx {
                                agent_id: cap_agent,
                                namespace: crate::memory::namespace::NamespaceScope::Owner,
                                session_id: Some(cap_session),
                                source_hint: "pre_compress".into(),
                            };
                            if let Err(e) =
                                insert_with_capture_filter(&writer, &registry, &ctx, raw).await
                            {
                                tracing::warn!("pre_compress raw_memory write failed: {e}");
                            }
                        } else if let Err(e) = writer.insert_raw_memory(&raw).await {
                            tracing::warn!("pre_compress raw_memory write failed: {e}");
                        }
                    });
                }
            }

            let summary_text = self.generate_summary(&chunk, 0, None).await;
            let source_tokens: usize = chunk
                .iter()
                .map(|(_, c)| super::context_window::estimate_tokens(c, ratio))
                .sum();

            let fact = super::summary_engine::summary_to_fact(
                &session_id,
                0,
                next_seq,
                summary_text,
                chunk.len(),
                source_tokens,
                &agent_id,
            );

            let raw = RawMemory::new(fact.content.clone(), RawMemorySource::SessionCompressed)
                .with_agent(&agent_id)
                .with_session(&session_id)
                .with_path(fact.path.clone());
            let insert_ok = if let Some(ref registry) = self.capture_registry {
                let store: Arc<dyn crate::memory::store::raw_memory::RawMemoryStore> =
                    self.database.clone();
                let ctx = CaptureCtx {
                    agent_id: agent_id.clone(),
                    namespace: crate::memory::namespace::NamespaceScope::Owner,
                    session_id: Some(session_id.clone()),
                    source_hint: "session_compressed".into(),
                };
                match insert_with_capture_filter(&store, registry, &ctx, raw).await {
                    Ok(_) => true,
                    Err(e) => {
                        warn!(
                            error = %e,
                            session = %session_id,
                            seq = next_seq,
                            "Failed to store d0 summary to raw_memories"
                        );
                        false
                    }
                }
            } else {
                match self.database.insert_raw_memory(&raw).await {
                    Ok(()) => true,
                    Err(e) => {
                        warn!(
                            error = %e,
                            session = %session_id,
                            seq = next_seq,
                            "Failed to store d0 summary to raw_memories"
                        );
                        false
                    }
                }
            };
            if insert_ok {
                d0_created += 1;
                self.metrics
                    .d0_summaries_created
                    .fetch_add(1, Ordering::Relaxed);

                let raw_content: String = chunk
                    .iter()
                    .map(|(role, text)| format!("[{role}]: {text}"))
                    .collect::<Vec<_>>()
                    .join("\n\n");
                self.store_raw_chunk(&session_id, next_seq as usize, &raw_content)
                    .await?;

                if let Some(ref indexer) = self.indexer {
                    indexer
                        .index_turn_text(
                            &session_id,
                            next_seq,
                            &raw_content,
                            "",
                            "owner",
                            &agent_id,
                        )
                        .await;
                }
            }

            next_seq += 1;
        }

        info!(
            session = %session_id,
            d0_created,
            chunks = semantic_chunks.len(),
            compressible_messages = compressible.len(),
            "Post-turn compression: d0 summaries generated"
        );

        let mut d1_created = 0u32;
        let total_d0 = self
            .count_valid_facts_at_depth(&session_id, &agent_id, 0)
            .await?;
        if total_d0 >= self.config.d1_min_fanout {
            d1_created = self
                .try_condense(&session_id, &agent_id, 0, 1, self.config.d1_min_fanout)
                .await?;
            if d1_created > 0 {
                self.metrics
                    .d1_condensations
                    .fetch_add(u64::from(d1_created), Ordering::Relaxed);
            }
        }

        let mut d2_created = 0u32;
        if self.config.max_summary_depth >= 2 {
            let total_d1 = self
                .count_valid_facts_at_depth(&session_id, &agent_id, 1)
                .await?;
            if total_d1 >= self.config.d2_min_fanout {
                d2_created = self
                    .try_condense(&session_id, &agent_id, 1, 2, self.config.d2_min_fanout)
                    .await?;
                if d2_created > 0 {
                    self.metrics
                        .d2_condensations
                        .fetch_add(u64::from(d2_created), Ordering::Relaxed);
                }
            }
        }

        Ok(CompressResult {
            d0_created,
            d1_created,
            d2_created,
        })
    }
}
