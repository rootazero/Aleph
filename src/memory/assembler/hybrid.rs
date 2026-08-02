//! Default implementation of [`WorkingMemoryAssembler`] — hybrid retrieval +
//! LLM re-rank (B strategy) with deterministic skeleton fallback (C strategy).

use super::envelope::{
    EnvelopeItem, EnvelopeMeta, EnvelopeSlot, MemoryEnvelope, SlotKind, SCHEMA_VERSION,
};
use super::error::AssemblerError;
use super::fallback::{skeleton_pack, sort_by_pinned_relevance, Candidate};
use super::feedback_floor::FeedbackFloorLoader;
use super::gather::{GatherInputs, Gatherer};
use super::hydration::{estimate_tokens, truncate_utf8_safe};
use super::profile::UserProfileLoader;
use super::rerank::{build_prompt, parse_response};
use super::{AssemblyBudget, WorkingMemoryAssembler};
use crate::config::types::memory::AssemblerConfig;
use crate::error::AlephError;
use crate::memory::note_retrieval::NoteFactRetrieval;
use crate::memory::session_resume::reader::SnapshotReader;
use crate::memory::session_search_summary::FactSourceFilter;
use crate::memory::store::MemoryBackend;
use crate::memory::SqliteMemoryBackend;
use crate::providers::adapter::RequestPayload;
use crate::providers::message::UnifiedMessage;
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;
use async_trait::async_trait;
use std::collections::HashMap;
use tracing::info;

/// Abstraction over the LLM call used by the re-rank stage. The default
/// implementation wraps `Arc<dyn AiProvider>`; tests inject stubs.
#[async_trait]
pub trait LlmReranker: Send + Sync {
    async fn complete(&self, prompt: &str, model: Option<&str>) -> Result<String, AlephError>;
}

/// Default reranker that drives any [`AiProvider`] with a single user message.
pub struct AiProviderReranker {
    provider: Arc<dyn AiProvider>,
}

impl AiProviderReranker {
    pub fn new(provider: Arc<dyn AiProvider>) -> Arc<Self> {
        Arc::new(Self { provider })
    }
}

#[async_trait]
impl LlmReranker for AiProviderReranker {
    async fn complete(&self, prompt: &str, model: Option<&str>) -> Result<String, AlephError> {
        let messages = vec![UnifiedMessage::user(prompt.to_string())];
        let mut payload = RequestPayload::new(&messages).with_system(Some(
            "You respond only with strict JSON. No prose. No markdown fences.",
        ));
        if let Some(m) = model {
            payload.model = Some(m.to_string());
        }
        let response = self.provider.process(payload).await?;
        Ok(response.text_content())
    }
}

/// Spec B: reuse `AiProviderReranker` as a `SummaryLlm` by delegating to
/// `LlmReranker::complete` with no model override. This avoids a separate
/// wrapper struct and keeps the two traits in sync automatically.
#[async_trait]
impl crate::memory::session_search_summary::synthesizer::SummaryLlm for AiProviderReranker {
    async fn complete(&self, prompt: &str) -> Result<String, AlephError> {
        <Self as LlmReranker>::complete(self, prompt, None).await
    }
}

pub struct HybridAssembler {
    gatherer: Gatherer,
    reranker: Arc<dyn LlmReranker>,
    config: AssemblerConfig,
}

impl HybridAssembler {
    pub fn new(
        retrieval: Arc<NoteFactRetrieval<SqliteMemoryBackend>>,
        snapshots: Arc<SnapshotReader>,
        backend: MemoryBackend,
        profile: Arc<UserProfileLoader>,
        feedback_floor: Arc<FeedbackFloorLoader>,
        reranker: Arc<dyn LlmReranker>,
        config: AssemblerConfig,
    ) -> Self {
        Self {
            gatherer: Gatherer {
                retrieval,
                snapshots,
                backend,
                profile,
                feedback_floor,
                project_scoped: config.project_scoped,
            },
            reranker,
            config,
        }
    }

    fn now(&self) -> i64 {
        chrono::Utc::now().timestamp()
    }

    async fn run_rerank(
        &self,
        query: &str,
        candidates: &[Candidate],
        total_budget: u32,
    ) -> Result<Vec<EnvelopeSlot>, &'static str> {
        let prompt = build_prompt(query, candidates, total_budget);
        let timeout = std::time::Duration::from_millis(self.config.rerank_timeout_ms);

        let raw = match tokio::time::timeout(
            timeout,
            self.reranker
                .complete(&prompt, self.config.rerank_model.as_deref()),
        )
        .await
        {
            Ok(Ok(text)) => text,
            Ok(Err(_)) => return Err("llm_error"),
            Err(_) => return Err("llm_timeout"),
        };

        let decisions = match parse_response(&raw, candidates, total_budget) {
            Ok(v) => v,
            Err(AssemblerError::RerankEmpty) => return Err("llm_empty_slots"),
            Err(AssemblerError::RerankParse(_)) => return Err("llm_parse_error"),
        };

        let by_id: HashMap<&str, &Candidate> =
            candidates.iter().map(|c| (c.id.as_str(), c)).collect();
        let mut slots: Vec<EnvelopeSlot> = Vec::new();

        // The two pinned slots below are added ON TOP of the LLM-chosen slots,
        // which `parse_response` has already scaled to 70% of `total_budget`.
        // Charging them their full static `fallback_skeleton` figures therefore
        // overshot the envelope (with headroom 300: 210 + 500 + 200 = 910).
        // Bound the pinned pair to the remaining 30% so the whole envelope
        // stays within budget.
        let pinned_cap = u32::try_from((f64::from(total_budget) * 0.3) as u64).unwrap_or(u32::MAX);
        let clamp_pinned = |configured: u32| -> u32 {
            if configured == 0 {
                0
            } else {
                configured.min(pinned_cap.max(1))
            }
        };

        // Feedback (user-taught rules) is pre-populated first — exempt from the
        // LLM re-rank's discretion. Ordering matters as much as inclusion:
        // `hydrate` charges the slot budget strictly in item order and drops
        // whatever truncates to empty, and `gather` pushes retrieval matches
        // into the pool BEFORE the always-on High/Critical floor. Collecting in
        // pool order therefore put the floor entry last in line for the budget,
        // so query-matched feedback notes could evict the standing rule the
        // floor exists to guarantee. Sorting by pinned relevance (floor entries
        // carry 1.0; RRF scores are «1.0) makes the eviction unrepresentable.
        // Same comparator the deterministic path already uses in `skeleton_pack`.
        let now = self.now();
        let mut feedback_cands: Vec<&Candidate> = candidates
            .iter()
            .filter(|c| c.slot_hint == SlotKind::Feedback)
            .collect();
        sort_by_pinned_relevance(&mut feedback_cands, now);
        if !feedback_cands.is_empty() {
            let items = feedback_cands
                .into_iter()
                .map(candidate_to_item)
                .collect::<Vec<_>>();
            slots.push(EnvelopeSlot {
                kind: SlotKind::Feedback,
                items,
                tokens_used: 0,
                tokens_budget: clamp_pinned(self.config.fallback_skeleton.feedback_tokens),
            });
        }

        // UserProfile always appended first if present in candidates. Same
        // budget-order hazard as Feedback above.
        let mut profile_cands: Vec<&Candidate> = candidates
            .iter()
            .filter(|c| c.slot_hint == SlotKind::UserProfile)
            .collect();
        sort_by_pinned_relevance(&mut profile_cands, now);
        if !profile_cands.is_empty() {
            let items = profile_cands
                .into_iter()
                .map(candidate_to_item)
                .collect::<Vec<_>>();
            slots.push(EnvelopeSlot {
                kind: SlotKind::UserProfile,
                items,
                tokens_used: 0,
                tokens_budget: clamp_pinned(self.config.fallback_skeleton.user_profile_tokens),
            });
        }

        for (kind, ids, budget) in decisions {
            let items = ids
                .into_iter()
                .filter_map(|id| by_id.get(id.as_str()).copied())
                .map(candidate_to_item)
                .collect::<Vec<_>>();
            if items.is_empty() {
                continue;
            }
            slots.push(EnvelopeSlot {
                kind,
                items,
                tokens_used: 0,
                tokens_budget: budget,
            });
        }
        Ok(slots)
    }

    #[allow(clippy::too_many_arguments)]
    fn pack_envelope(
        &self,
        query: &str,
        agent_id: &str,
        session_id: Option<&str>,
        candidates_considered: usize,
        slots: Vec<EnvelopeSlot>,
        strategy: &'static str,
        used_fallback: bool,
        fallback_reason: Option<String>,
        llm_rerank_latency_ms: Option<u64>,
        total_latency_ms: u64,
    ) -> MemoryEnvelope {
        let mut slots = slots;
        hydrate(&mut slots);
        MemoryEnvelope {
            schema_version: SCHEMA_VERSION.to_string(),
            generated_at: self.now(),
            query: query.to_string(),
            agent_id: agent_id.to_string(),
            session_id: session_id.map(str::to_string),
            slots,
            meta: EnvelopeMeta {
                strategy: strategy.into(),
                candidates_considered,
                used_fallback,
                fallback_reason,
                llm_rerank_latency_ms,
                total_latency_ms,
            },
        }
    }
}

#[async_trait]
impl WorkingMemoryAssembler for HybridAssembler {
    async fn assemble(
        &self,
        query: &str,
        agent_id: &str,
        session_id: Option<&str>,
        budget: AssemblyBudget,
        filter: FactSourceFilter,
    ) -> Result<MemoryEnvelope, AlephError> {
        let start = std::time::Instant::now();

        // Readiness gate (Pillar 2): if a prior session's end-flush is still
        // consolidating this agent's memory, briefly wait so this session sees
        // the linked notes. Returns immediately when no flush is in progress.
        crate::memory::flush::global_registry()
            .await_ready(agent_id, std::time::Duration::from_secs(2))
            .await;

        if !self.config.enabled {
            let env = self.pack_envelope(
                query,
                agent_id,
                session_id,
                0,
                Vec::new(),
                "disabled",
                true,
                Some("assembler_disabled".into()),
                None,
                start.elapsed().as_millis() as u64,
            );
            emit_tracing(&env, query);
            return Ok(env);
        }

        // Stage 1: gather (with source filter applied post-gather)
        let gathered = self
            .gatherer
            .gather(&GatherInputs {
                query: query.to_string(),
                agent_id: agent_id.to_string(),
                session_id: session_id.map(str::to_string),
                pool_limit: self.config.candidate_pool_limit,
                filter,
            })
            .await;
        let candidates_considered = gathered.len();

        // Fast-path: tiny pool or forced fallback skips LLM.
        let too_small = candidates_considered < 3;
        if self.config.force_fallback || too_small {
            let reason = if self.config.force_fallback {
                "forced"
            } else {
                "tiny_pool"
            };
            let slots = skeleton_pack(
                &gathered,
                &self.config.fallback_skeleton,
                budget.total_tokens,
                self.now(),
            );
            let env = self.pack_envelope(
                query,
                agent_id,
                session_id,
                candidates_considered,
                slots,
                "skeleton_fallback_v1",
                true,
                Some(reason.into()),
                None,
                start.elapsed().as_millis() as u64,
            );
            emit_tracing(&env, query);
            return Ok(env);
        }

        // Stage 2: LLM rerank
        let rerank_start = std::time::Instant::now();
        let rerank_outcome = self.run_rerank(query, &gathered, budget.total_tokens).await;
        let rerank_latency = rerank_start.elapsed().as_millis() as u64;

        match rerank_outcome {
            Ok(slots) => {
                let env = self.pack_envelope(
                    query,
                    agent_id,
                    session_id,
                    candidates_considered,
                    slots,
                    "hybrid_v1",
                    false,
                    None,
                    Some(rerank_latency),
                    start.elapsed().as_millis() as u64,
                );
                emit_tracing(&env, query);
                Ok(env)
            }
            Err(reason) => {
                let slots = skeleton_pack(
                    &gathered,
                    &self.config.fallback_skeleton,
                    budget.total_tokens,
                    self.now(),
                );
                let env = self.pack_envelope(
                    query,
                    agent_id,
                    session_id,
                    candidates_considered,
                    slots,
                    "skeleton_fallback_v1",
                    true,
                    Some(reason.into()),
                    Some(rerank_latency),
                    start.elapsed().as_millis() as u64,
                );
                emit_tracing(&env, query);
                Ok(env)
            }
        }
    }
}

fn candidate_to_item(c: &Candidate) -> EnvelopeItem {
    EnvelopeItem {
        // rust-doctor-disable-next-line excessive-clone
        id: c.id.clone(),
        // rust-doctor-disable-next-line excessive-clone
        title: c.title.clone(),
        // rust-doctor-disable-next-line excessive-clone
        content: c.full_content.clone(),
        // rust-doctor-disable-next-line excessive-clone
        source: c.source.clone(),
        relevance: c.relevance,
        tokens: 0,
        updated_at: c.updated_at,
        extra: Default::default(),
    }
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
            // No early break: once the budget is exhausted, remaining_chars is
            // 0, so later items truncate to empty and are dropped by the
            // retain below. Breaking instead would leave them with their full
            // untruncated content (tokens == 0), which the retain keeps —
            // silently blowing the slot budget.
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
    use crate::memory::assembler::envelope::{EnvelopeItem, ItemSource};

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
                    category: "reference".into(),
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
