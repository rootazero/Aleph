//! Adapter: wraps an MCP client so a third-party plugin can be used
//! wherever MemoryExtension is expected.

use crate::error::AlephError;
use crate::memory::assembler::envelope::{EnvelopeItem, MemoryEnvelope};
use crate::memory::extensions::traits::MemoryExtension;
use crate::memory::extensions::types::{CaptureCtx, CaptureDecision, ProduceCtx, RetrieveCtx};
use crate::memory::store::raw_memory::RawMemory;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

/// Minimal trait the adapter needs to talk to a plugin. Tests use an
/// in-memory implementation; production wires it to the real MCP client.
#[async_trait]
pub trait McpCaller: Send + Sync {
    async fn call(&self, method: &str, args: Value) -> Result<Value, AlephError>;
}

pub struct McpMemoryExtension {
    name: String,
    caller: Arc<dyn McpCaller>,
}

impl McpMemoryExtension {
    pub fn new(name: impl Into<String>, caller: Arc<dyn McpCaller>) -> Self {
        Self {
            name: name.into(),
            caller,
        }
    }
}

#[async_trait]
impl MemoryExtension for McpMemoryExtension {
    fn name(&self) -> &str {
        &self.name
    }

    async fn on_retrieve(
        &self,
        ctx: &RetrieveCtx,
        envelope: &mut MemoryEnvelope,
    ) -> Result<(), AlephError> {
        let args = json!({
            "agent_id": ctx.agent_id,
            "query": ctx.query,
            "session_id": ctx.session_id,
            "envelope": envelope,
        });
        let resp = self.caller.call("memory.on_retrieve", args).await?;
        // Response shape: { "additions": [EnvelopeItem, ...] } — optional.
        if let Some(additions) = resp.get("additions").and_then(|v| v.as_array()) {
            for a in additions {
                if let Ok(item) = serde_json::from_value::<EnvelopeItem>(a.clone()) {
                    // Merge into first slot if it exists; otherwise drop.
                    // The plan's richer "create an Extension slot" semantic
                    // is deferred — a simpler merge works for v1.
                    if let Some(slot) = envelope.slots.first_mut() {
                        slot.items.push(item);
                    }
                }
            }
        }
        Ok(())
    }

    async fn on_capture(
        &self,
        ctx: &CaptureCtx,
        raw: &mut RawMemory,
    ) -> Result<CaptureDecision, AlephError> {
        let args = json!({
            "agent_id": ctx.agent_id,
            "session_id": ctx.session_id,
            "source_hint": ctx.source_hint,
            "raw": raw,
        });
        let resp = self.caller.call("memory.on_capture", args).await?;

        // Optional modified raw: { "modified": RawMemory } — apply before
        // returning the decision so Allow+modified propagates through the chain.
        if let Some(modified) = resp.get("modified") {
            if let Ok(new_raw) = serde_json::from_value::<RawMemory>(modified.clone()) {
                *raw = new_raw;
            }
        }

        match resp.get("decision").and_then(|v| v.as_str()) {
            Some("block") => {
                let reason = resp
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("plugin blocked")
                    .to_string();
                Ok(CaptureDecision::Block { reason })
            }
            _ => Ok(CaptureDecision::Allow),
        }
    }

    async fn produce(&self, ctx: &ProduceCtx) -> Result<Vec<RawMemory>, AlephError> {
        let args = json!({
            "agent_id": ctx.agent_id,
            "tick": ctx.tick,
        });
        let resp = self.caller.call("memory.produce", args).await?;
        let raws = resp
            .get("raw_memories")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        raws.into_iter()
            .map(|v| {
                serde_json::from_value::<RawMemory>(v)
                    .map_err(|e| AlephError::other(format!("malformed raw_memory: {e}")))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::assembler::envelope::{EnvelopeMeta, EnvelopeSlot, SlotKind};
    use crate::memory::namespace::NamespaceScope;
    use crate::memory::store::raw_memory::RawMemorySource;
    use std::collections::HashMap;
    use std::sync::Mutex;

    struct CannedCaller {
        canned: Mutex<HashMap<String, Value>>,
    }

    impl CannedCaller {
        fn new(canned: Vec<(&str, Value)>) -> Self {
            let mut m = HashMap::new();
            for (k, v) in canned {
                m.insert(k.to_string(), v);
            }
            Self { canned: Mutex::new(m) }
        }
    }

    #[async_trait]
    impl McpCaller for CannedCaller {
        async fn call(&self, method: &str, _args: Value) -> Result<Value, AlephError> {
            Ok(self
                .canned
                .lock()
                .unwrap()
                .get(method)
                .cloned()
                .unwrap_or_else(|| json!({})))
        }
    }

    fn raw() -> RawMemory {
        RawMemory::new("hi".into(), RawMemorySource::Transcript)
    }

    fn empty_envelope() -> MemoryEnvelope {
        MemoryEnvelope {
            schema_version: "1".into(),
            generated_at: 0,
            query: "q".into(),
            agent_id: "a".into(),
            session_id: None,
            slots: vec![EnvelopeSlot {
                kind: SlotKind::RelevantNotes,
                items: vec![],
                tokens_used: 0,
                tokens_budget: 1000,
            }],
            meta: EnvelopeMeta {
                strategy: "hybrid".into(),
                candidates_considered: 0,
                used_fallback: false,
                fallback_reason: None,
                llm_rerank_latency_ms: None,
                total_latency_ms: 0,
            },
        }
    }

    #[tokio::test]
    async fn on_capture_block_maps_correctly() {
        let caller = Arc::new(CannedCaller::new(vec![(
            "memory.on_capture",
            json!({"decision": "block", "reason": "pii"}),
        )]));
        let ext = McpMemoryExtension::new("t", caller);
        let mut r = raw();
        let ctx = CaptureCtx {
            agent_id: "a".into(),
            namespace: NamespaceScope::Owner,
            session_id: None,
            source_hint: "transcript".into(),
        };
        let d = ext.on_capture(&ctx, &mut r).await.unwrap();
        match d {
            CaptureDecision::Block { reason } => assert_eq!(reason, "pii"),
            _ => panic!("expected block"),
        }
    }

    #[tokio::test]
    async fn on_capture_empty_response_allows() {
        let caller = Arc::new(CannedCaller::new(vec![(
            "memory.on_capture",
            json!({}),
        )]));
        let ext = McpMemoryExtension::new("t", caller);
        let mut r = raw();
        let ctx = CaptureCtx {
            agent_id: "a".into(),
            namespace: NamespaceScope::Owner,
            session_id: None,
            source_hint: "transcript".into(),
        };
        let d = ext.on_capture(&ctx, &mut r).await.unwrap();
        assert!(matches!(d, CaptureDecision::Allow));
    }

    #[tokio::test]
    async fn produce_empty_response_returns_empty() {
        let caller = Arc::new(CannedCaller::new(vec![(
            "memory.produce",
            json!({}),
        )]));
        let ext = McpMemoryExtension::new("t", caller);
        let ctx = ProduceCtx {
            agent_id: "a".into(),
            namespace: NamespaceScope::Owner,
            tick: 0,
        };
        let out = ext.produce(&ctx).await.unwrap();
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn on_retrieve_empty_additions_is_noop() {
        let mut env = empty_envelope();
        let caller = Arc::new(CannedCaller::new(vec![(
            "memory.on_retrieve",
            json!({}),
        )]));
        let ext = McpMemoryExtension::new("t", caller);
        let ctx = RetrieveCtx {
            agent_id: "a".into(),
            namespace: NamespaceScope::Owner,
            query: "q".into(),
            session_id: None,
        };
        let before = env.slots.len();
        ext.on_retrieve(&ctx, &mut env).await.unwrap();
        assert_eq!(env.slots.len(), before, "no slots added or removed");
    }
}
