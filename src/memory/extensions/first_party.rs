//! First-party memory extensions shipped with Aleph.
//!
//! These implement `MemoryExtension` directly (in-process) instead of
//! going through the MCP adapter. They validate that the dispatch
//! plumbing works end-to-end before any external plugin runs.

use crate::error::AlephError;
use crate::memory::assembler::envelope::MemoryEnvelope;
use crate::memory::extensions::traits::MemoryExtension;
use crate::memory::extensions::types::RetrieveCtx;
use async_trait::async_trait;

/// POC first-party extension: drop envelope items whose `relevance` is
/// below the configured floor. Serves as a smoke test of the on_retrieve
/// dispatch pipeline; also a mild trimming optimization.
pub struct EnvelopeRelevanceFloorExtension {
    floor: f32,
}

impl EnvelopeRelevanceFloorExtension {
    pub fn new(floor: f32) -> Self {
        Self { floor: floor.clamp(0.0, 1.0) }
    }
}

#[async_trait]
impl MemoryExtension for EnvelopeRelevanceFloorExtension {
    fn name(&self) -> &str {
        "aleph.envelope_relevance_floor"
    }

    async fn on_retrieve(
        &self,
        _ctx: &RetrieveCtx,
        envelope: &mut MemoryEnvelope,
    ) -> Result<(), AlephError> {
        let floor = self.floor;
        for slot in envelope.slots.iter_mut() {
            slot.items.retain(|item| item.relevance >= floor);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::assembler::envelope::{
        EnvelopeItem, EnvelopeMeta, EnvelopeSlot, ItemSource, SlotKind,
    };
    use crate::memory::extensions::types::RetrieveCtx;
    use crate::memory::namespace::NamespaceScope;

    // Helper: build an envelope with items having the given relevances in a
    // single RelevantNotes slot.
    fn env_with_relevances(rs: &[f32]) -> MemoryEnvelope {
        let items: Vec<EnvelopeItem> = rs
            .iter()
            .enumerate()
            .map(|(i, r)| EnvelopeItem {
                id: format!("id-{i}"),
                title: format!("t-{i}"),
                content: "c".into(),
                relevance: *r,
                tokens: 1,
                updated_at: 0,
                source: ItemSource::Note {
                    path: format!("wiki/{i}"),
                    category: "wiki".into(),
                },
                extra: serde_json::Map::new(),
            })
            .collect();
        let item_count = items.len() as u32;
        MemoryEnvelope {
            schema_version: "1".into(),
            generated_at: 0,
            query: "q".into(),
            agent_id: "a".into(),
            session_id: None,
            slots: vec![EnvelopeSlot {
                kind: SlotKind::RelevantNotes,
                items,
                tokens_used: item_count,
                tokens_budget: 100,
            }],
            meta: EnvelopeMeta {
                strategy: "test".into(),
                candidates_considered: 0,
                used_fallback: false,
                fallback_reason: None,
                llm_rerank_latency_ms: None,
                total_latency_ms: 0,
            },
        }
    }

    fn ctx() -> RetrieveCtx {
        RetrieveCtx {
            agent_id: "a".into(),
            namespace: NamespaceScope::Owner,
            query: "q".into(),
            session_id: None,
        }
    }

    #[tokio::test]
    async fn drops_items_below_floor() {
        let ext = EnvelopeRelevanceFloorExtension::new(0.5);
        let mut env = env_with_relevances(&[0.1, 0.4, 0.5, 0.9]);
        ext.on_retrieve(&ctx(), &mut env).await.unwrap();
        let kept: Vec<_> = env.slots[0].items.iter().map(|i| i.relevance).collect();
        assert_eq!(kept, vec![0.5, 0.9]);
    }

    #[tokio::test]
    async fn floor_zero_drops_nothing() {
        let ext = EnvelopeRelevanceFloorExtension::new(0.0);
        let mut env = env_with_relevances(&[0.0, 0.5, 1.0]);
        ext.on_retrieve(&ctx(), &mut env).await.unwrap();
        assert_eq!(env.slots[0].items.len(), 3);
    }

    #[tokio::test]
    async fn empty_slot_survives() {
        let ext = EnvelopeRelevanceFloorExtension::new(0.5);
        let mut env = env_with_relevances(&[]);
        ext.on_retrieve(&ctx(), &mut env).await.unwrap();
        assert_eq!(env.slots[0].items.len(), 0);
    }

    #[tokio::test]
    async fn floor_above_one_clamps_to_one() {
        let ext = EnvelopeRelevanceFloorExtension::new(2.0);
        let mut env = env_with_relevances(&[0.5, 1.0]);
        ext.on_retrieve(&ctx(), &mut env).await.unwrap();
        // With floor clamped to 1.0, only items with relevance >= 1.0 survive.
        let kept: Vec<_> = env.slots[0].items.iter().map(|i| i.relevance).collect();
        assert_eq!(kept, vec![1.0]);
    }
}
