//! Default implementation of [`WorkingMemoryAssembler`] — hybrid retrieval +
//! LLM re-rank with deterministic skeleton fallback. Task 9 lands the
//! fallback-only path; Task 10 will wire in the LLM re-rank.

use super::envelope::{EnvelopeMeta, EnvelopeSlot, MemoryEnvelope, SCHEMA_VERSION};
use super::fallback::{skeleton_pack, Candidate};
use super::gather::{GatherInputs, Gatherer};
use super::hydration::{estimate_tokens, truncate_utf8_safe};
use super::profile::UserProfileLoader;
use super::{AssemblyBudget, WorkingMemoryAssembler};
use crate::config::types::memory::AssemblerConfig;
use crate::error::AlephError;
use crate::memory::note_retrieval::NoteFactRetrieval;
use crate::memory::session_resume::reader::SnapshotReader;
use crate::memory::store::MemoryBackend;
use crate::memory::SqliteMemoryBackend;
use crate::sync_primitives::Arc;
use async_trait::async_trait;
use tracing::info;

pub struct HybridAssembler {
    gatherer: Gatherer,
    config: AssemblerConfig,
}

impl HybridAssembler {
    pub fn new(
        retrieval: Arc<NoteFactRetrieval<SqliteMemoryBackend>>,
        snapshots: Arc<SnapshotReader>,
        backend: MemoryBackend,
        profile: Arc<UserProfileLoader>,
        config: AssemblerConfig,
    ) -> Self {
        Self {
            gatherer: Gatherer {
                retrieval,
                snapshots,
                backend,
                profile,
            },
            config,
        }
    }

    fn now(&self) -> i64 {
        chrono::Utc::now().timestamp()
    }
}

#[async_trait]
impl WorkingMemoryAssembler for HybridAssembler {
    async fn assemble(
        &self,
        query: &str,
        agent_id: &str,
        session_id: Option<&str>,
        _budget: AssemblyBudget,
    ) -> Result<MemoryEnvelope, AlephError> {
        let start = std::time::Instant::now();

        // Stage 1: gather
        let gathered = self
            .gatherer
            .gather(&GatherInputs {
                query: query.to_string(),
                agent_id: agent_id.to_string(),
                session_id: session_id.map(str::to_string),
                pool_limit: self.config.candidate_pool_limit,
            })
            .await;
        let candidates_considered = gathered.len();

        // Stage 2 will be added in Task 10 — always fallback for now.
        let mut slots = fallback_slots(&gathered, &self.config, self.now());
        hydrate(&mut slots);

        let total_latency = start.elapsed().as_millis() as u64;
        let envelope = MemoryEnvelope {
            schema_version: SCHEMA_VERSION.to_string(),
            generated_at: self.now(),
            query: query.to_string(),
            agent_id: agent_id.to_string(),
            session_id: session_id.map(str::to_string),
            slots,
            meta: EnvelopeMeta {
                strategy: "skeleton_fallback_v1".into(),
                candidates_considered,
                used_fallback: true,
                fallback_reason: Some("stage2_pending".into()),
                llm_rerank_latency_ms: None,
                total_latency_ms: total_latency,
            },
        };

        emit_tracing(&envelope, query);
        Ok(envelope)
    }
}

fn fallback_slots(
    candidates: &[Candidate],
    config: &AssemblerConfig,
    now: i64,
) -> Vec<EnvelopeSlot> {
    skeleton_pack(candidates, &config.fallback_skeleton, now)
}

fn hydrate(slots: &mut [EnvelopeSlot]) {
    for slot in slots.iter_mut() {
        let mut used = 0u32;
        let budget_chars = slot.tokens_budget.saturating_mul(4) as usize;
        for item in slot.items.iter_mut() {
            let remaining_chars = budget_chars.saturating_sub((used as usize).saturating_mul(4));
            let truncated = truncate_utf8_safe(&item.content, remaining_chars);
            item.tokens = estimate_tokens(&truncated);
            item.content = truncated;
            used = used.saturating_add(item.tokens);
            if used >= slot.tokens_budget {
                break;
            }
        }
        slot.tokens_used = used;
        slot.items.retain(|i| i.tokens > 0 || !i.content.is_empty());
    }
}

fn emit_tracing(env: &MemoryEnvelope, query: &str) {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(query.as_bytes());
    let query_hash = format!("{:x}", h.finalize());
    let total_tokens: u32 = env.slots.iter().map(|s| s.tokens_used).sum();
    info!(
        target: "memory.assembler",
        query_hash = %query_hash,
        agent_id = %env.agent_id,
        session_id = ?env.session_id,
        strategy = %env.meta.strategy,
        used_fallback = env.meta.used_fallback,
        fallback_reason = ?env.meta.fallback_reason,
        candidates = env.meta.candidates_considered,
        llm_rerank_ms = ?env.meta.llm_rerank_latency_ms,
        total_ms = env.meta.total_latency_ms,
        slot_count = env.slots.len(),
        total_tokens,
        "assembly completed"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::assembler::envelope::{EnvelopeItem, ItemSource, SlotKind};

    #[test]
    fn hydrate_truncates_content_to_slot_budget() {
        let mut slots = vec![EnvelopeSlot {
            kind: SlotKind::RelevantNotes,
            items: vec![EnvelopeItem {
                id: "note://a".into(),
                title: "a".into(),
                content: "x".repeat(10_000),
                source: ItemSource::Note {
                    path: "a".into(),
                    category: "wiki".into(),
                },
                relevance: 1.0,
                tokens: 0,
                updated_at: 0,
                extra: Default::default(),
            }],
            tokens_used: 0,
            tokens_budget: 100,
        }];
        hydrate(&mut slots);
        assert!(slots[0].items[0].content.len() <= 400);
        assert!(slots[0].tokens_used <= 100);
    }
}
