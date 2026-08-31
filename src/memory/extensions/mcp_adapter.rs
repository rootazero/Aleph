//! Adapter: wraps an MCP client so a third-party plugin can be used
//! wherever `MemoryExtension` is expected.

use crate::error::AlephError;
use crate::memory::assembler::envelope::{EnvelopeItem, MemoryEnvelope};
use crate::memory::extensions::traits::MemoryExtension;
use crate::memory::extensions::types::{
    CaptureCtx, CaptureDecision, DelegationCtx, PreCompressCtx, ProduceCtx, RetrieveCtx,
    SessionSwitchCtx,
};
use crate::memory::store::raw_memory::RawMemory;
use crate::sync_primitives::Arc;
use arc_swap::ArcSwap;
use async_trait::async_trait;
use serde_json::{json, Value};

/// Minimal trait the adapter needs to talk to a plugin. Tests use an
/// in-memory implementation; production wires it to the real MCP client.
#[async_trait]
pub trait McpCaller: Send + Sync {
    async fn call(&self, method: &str, args: Value) -> Result<Value, AlephError>;
}

pub struct McpMemoryExtension {
    name: String,
    /// `Some` when created unbound by the plugin loader (drives the boot-time
    /// rebind to a real `ManagerBackedMcpCaller`). `None` for test-constructed
    /// extensions that are handed a concrete caller up front.
    server_id: Option<String>,
    /// Swappable so the boot-time bind can replace `UnboundMcpCaller` with the
    /// real MCP-backed caller without re-registering. Dispatch reads via `.load()`.
    ///
    /// Double-`Arc`: `arc-swap`'s `RefCnt` is only implemented for `Arc<T: Sized>`,
    /// so a trait object must be stored as `ArcSwap<Arc<dyn _>>` (the inner
    /// `Arc<dyn _>` is a sized fat pointer). Auto-deref makes `.load().call(..)`
    /// reach through both `Arc`s transparently.
    caller: ArcSwap<Arc<dyn McpCaller>>,
}

impl McpMemoryExtension {
    /// Construct with a concrete caller (already bound). `server_id` is `None`,
    /// so the boot-time bind pass skips it.
    pub fn new(name: impl Into<String>, caller: Arc<dyn McpCaller>) -> Self {
        Self {
            name: name.into(),
            server_id: None,
            caller: ArcSwap::from(Arc::new(caller)),
        }
    }

    /// Construct unbound: backed by `UnboundMcpCaller` until `rebind` replaces
    /// it. `server_id` (when `Some`) is the MCP server that
    /// `bind_memory_callers` will route this plugin's hook calls to.
    #[must_use]
    pub fn new_unbound(name: String, server_id: Option<String>) -> Self {
        // rust-doctor-disable-next-line excessive-clone
        let caller: Arc<dyn McpCaller> = Arc::new(UnboundMcpCaller::new(name.clone()));
        Self {
            name,
            server_id,
            caller: ArcSwap::from(Arc::new(caller)),
        }
    }

    /// Swap the underlying caller. Visible to every subsequent hook dispatch.
    pub fn rebind(&self, caller: Arc<dyn McpCaller>) {
        self.caller.store(Arc::new(caller));
    }

    /// The MCP server id this extension's hooks route to, if resolved at
    /// registration. `None` means "leave bound to whatever caller it has".
    pub fn server_id(&self) -> Option<&str> {
        self.server_id.as_deref()
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
        let resp = self.caller.load().call("memory.on_retrieve", args).await?;
        // Response shape: { "additions": [EnvelopeItem, ...] } — optional.
        if let Some(additions) = resp.get("additions").and_then(|v| v.as_array()) {
            for a in additions {
                // rust-doctor-disable-next-line excessive-clone
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
        let resp = self.caller.load().call("memory.on_capture", args).await?;

        // Optional modified raw: { "modified": RawMemory } — apply before
        // returning the decision so Allow+modified propagates through the chain.
        //
        // SECURITY: a compromised or buggy plugin must not be able to forge
        // another user's memory write. Identity fields (id, agent_id,
        // session_id, path, source, created_at) are preserved verbatim from
        // the incoming `raw`; only `content` (and `attachment_text`) may be
        // overwritten, and the rewritten content is length-capped to bound
        // the prompt-injection payload.
        const MAX_PLUGIN_REWRITTEN_CONTENT_BYTES: usize = 16 * 1024;
        if let Some(modified) = resp.get("modified") {
            if let Ok(new_raw) = serde_json::from_value::<RawMemory>(modified.clone()) {
                if new_raw.content.len() <= MAX_PLUGIN_REWRITTEN_CONTENT_BYTES {
                    raw.content = new_raw.content;
                } else {
                    tracing::warn!(
                        plugin = %self.name,
                        len = new_raw.content.len(),
                        "plugin rewrite exceeded MAX_PLUGIN_REWRITTEN_CONTENT_BYTES; \
                         ignoring rewrite"
                    );
                }
                if new_raw.attachment_text.is_some() {
                    raw.attachment_text = new_raw.attachment_text;
                }
                // Identity fields (id, agent_id, session_id, path, source,
                // created_at, is_processed) intentionally NOT overwritten.
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
        let resp = self.caller.load().call("memory.produce", args).await?;
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

    async fn on_session_switch(&self, ctx: &SessionSwitchCtx) -> Result<(), AlephError> {
        let args = json!({
            "agent_id": ctx.agent_id,
            "new_session_id": ctx.new_session_id,
            "parent_session_id": ctx.parent_session_id,
            "reason": ctx.reason,
            "reset": ctx.reset(),
        });
        // Acknowledge errors but don't try to parse — this is notify-only.
        let _ = self
            .caller
            .load()
            .call("memory.on_session_switch", args)
            .await?;
        Ok(())
    }

    async fn on_pre_compress(&self, ctx: &PreCompressCtx) -> Result<String, AlephError> {
        let args = json!({
            "agent_id": ctx.agent_id,
            "session_id": ctx.session_id,
            "messages_count": ctx.messages_count,
            "oldest_at": ctx.oldest_at,
            "newest_at": ctx.newest_at,
        });
        let resp = self
            .caller
            .load()
            .call("memory.on_pre_compress", args)
            .await?;
        // Response shape: { "text": "..." } — optional. Empty string ≡ no contribution.
        Ok(resp
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string())
    }

    async fn on_delegation(&self, ctx: &DelegationCtx) -> Result<(), AlephError> {
        let args = json!({
            "agent_id": ctx.agent_id,
            "parent_session_id": ctx.parent_session_id,
            "child_session_id": ctx.child_session_id,
            "task": ctx.task,
            "result_summary": ctx.result_summary,
        });
        let _ = self
            .caller
            .load()
            .call("memory.on_delegation", args)
            .await?;
        Ok(())
    }
}

/// Placeholder `McpCaller` used when a plugin is registered before its
/// concrete MCP client is bound. Calls fail with a clear diagnostic error.
/// The dispatch layer's per-extension timeout + warn-and-skip policy degrades
/// gracefully, so an unbound plugin never panics or stalls the pipeline.
///
/// `ExtensionManager::bind_memory_callers` replaces this with a real binding
/// at server startup (and on hot-load) by calling `McpMemoryExtension::rebind`
/// once the `McpManager` handle is available.
pub struct UnboundMcpCaller {
    plugin_name: String,
}

impl UnboundMcpCaller {
    pub fn new(plugin_name: impl Into<String>) -> Self {
        Self {
            plugin_name: plugin_name.into(),
        }
    }
}

#[async_trait]
impl McpCaller for UnboundMcpCaller {
    async fn call(&self, method: &str, _args: Value) -> Result<Value, AlephError> {
        Err(AlephError::other(format!(
            "memory plugin '{}' is registered but its MCP client is not yet bound \
             (method={method}); bind_memory_callers wires the real McpManager",
            self.plugin_name
        )))
    }
}

/// Real `McpCaller` backed by the live MCP manager. Routes each hook method
/// call to the plugin's MCP server via `McpManagerHandle::get_client` →
/// `McpClient::call_tool`. Constructed at boot by `bind_memory_callers` once
/// the manager handle is available.
pub struct ManagerBackedMcpCaller {
    handle: crate::mcp::McpManagerHandle,
    server_id: String,
}

impl ManagerBackedMcpCaller {
    pub fn new(handle: crate::mcp::McpManagerHandle, server_id: impl Into<String>) -> Self {
        Self {
            handle,
            server_id: server_id.into(),
        }
    }

    /// Map an `McpToolResult` to the inner JSON the hook adapters expect.
    /// Success → the `content` Value; failure → an error. Extracted so the
    /// mapping is unit-testable without a live handle.
    pub(crate) fn map_result(res: crate::mcp::types::McpToolResult) -> Result<Value, AlephError> {
        if res.success {
            Ok(res.content)
        } else {
            Err(AlephError::other(res.error.unwrap_or_else(|| {
                "mcp memory tool call failed".to_string()
            })))
        }
    }
}

#[async_trait]
impl McpCaller for ManagerBackedMcpCaller {
    async fn call(&self, method: &str, args: Value) -> Result<Value, AlephError> {
        let client = self
            .handle
            .get_client(&self.server_id)
            .await?
            .ok_or_else(|| {
                AlephError::other(format!(
                    "memory MCP server '{}' not running (method={method})",
                    self.server_id
                ))
            })?;
        let res = client
            .call_tool(method, args)
            .await
            .map_err(|e| AlephError::other(format!("mcp call_tool '{method}' failed: {e}")))?;
        Self::map_result(res)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::assembler::envelope::{EnvelopeMeta, EnvelopeSlot, SlotKind};
    use crate::memory::namespace::NamespaceScope;
    use crate::memory::store::raw_memory::RawMemorySource;
    use std::collections::HashMap;

    struct CannedCaller {
        canned: crate::sync_primitives::Mutex<HashMap<String, Value>>,
    }

    impl CannedCaller {
        fn new(canned: Vec<(&str, Value)>) -> Self {
            let mut m = HashMap::new();
            for (k, v) in canned {
                m.insert(k.to_string(), v);
            }
            Self {
                canned: crate::sync_primitives::Mutex::new(m),
            }
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
        let caller = Arc::new(CannedCaller::new(vec![("memory.on_capture", json!({}))]));
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
        let caller = Arc::new(CannedCaller::new(vec![("memory.produce", json!({}))]));
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
        let caller = Arc::new(CannedCaller::new(vec![("memory.on_retrieve", json!({}))]));
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

    use crate::memory::extensions::types::{
        DelegationCtx, PreCompressCtx, SessionSwitchCtx, SessionSwitchReason,
    };

    #[tokio::test]
    async fn on_session_switch_notify_returns_ok() {
        let caller = Arc::new(CannedCaller::new(vec![(
            "memory.on_session_switch",
            json!({}),
        )]));
        let ext = McpMemoryExtension::new("t", caller);
        let ctx = SessionSwitchCtx {
            agent_id: "a".into(),
            namespace: NamespaceScope::Owner,
            new_session_id: "s2".into(),
            parent_session_id: Some("s1".into()),
            reason: SessionSwitchReason::Branch,
        };
        ext.on_session_switch(&ctx).await.unwrap();
    }

    #[tokio::test]
    async fn on_pre_compress_returns_text_field_when_present() {
        let caller = Arc::new(CannedCaller::new(vec![(
            "memory.on_pre_compress",
            json!({"text": "extension contribution"}),
        )]));
        let ext = McpMemoryExtension::new("t", caller);
        let ctx = PreCompressCtx {
            agent_id: "a".into(),
            namespace: NamespaceScope::Owner,
            session_id: Some("s".into()),
            messages_count: 5,
            oldest_at: None,
            newest_at: None,
        };
        let out = ext.on_pre_compress(&ctx).await.unwrap();
        assert_eq!(out, "extension contribution");
    }

    #[tokio::test]
    async fn on_pre_compress_missing_text_field_returns_empty() {
        let caller = Arc::new(CannedCaller::new(vec![(
            "memory.on_pre_compress",
            json!({}),
        )]));
        let ext = McpMemoryExtension::new("t", caller);
        let ctx = PreCompressCtx {
            agent_id: "a".into(),
            namespace: NamespaceScope::Owner,
            session_id: None,
            messages_count: 0,
            oldest_at: None,
            newest_at: None,
        };
        let out = ext.on_pre_compress(&ctx).await.unwrap();
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn on_delegation_notify_returns_ok() {
        let caller = Arc::new(CannedCaller::new(vec![("memory.on_delegation", json!({}))]));
        let ext = McpMemoryExtension::new("t", caller);
        let ctx = DelegationCtx {
            agent_id: "a".into(),
            namespace: NamespaceScope::Owner,
            parent_session_id: "p".into(),
            child_session_id: "c".into(),
            task: "do thing".into(),
            result_summary: "did thing".into(),
        };
        ext.on_delegation(&ctx).await.unwrap();
    }

    #[tokio::test]
    async fn rebind_swaps_caller_visible_to_hooks() {
        // Starts unbound → on_capture errors. After rebind to a canned caller
        // that allows, on_capture returns Allow. Proves ArcSwap visibility.
        let ext =
            McpMemoryExtension::new_unbound("p".to_string(), Some("plugin:p/srv".to_string()));
        let ctx = CaptureCtx {
            agent_id: "a".into(),
            namespace: NamespaceScope::Owner,
            session_id: None,
            source_hint: "transcript".into(),
        };
        let mut r = raw();
        // Unbound: errors.
        assert!(ext.on_capture(&ctx, &mut r).await.is_err());
        // Rebind to a caller that returns empty (Allow).
        let caller = Arc::new(CannedCaller::new(vec![("memory.on_capture", json!({}))]));
        ext.rebind(caller);
        let d = ext.on_capture(&ctx, &mut r).await.unwrap();
        assert!(matches!(d, CaptureDecision::Allow));
        assert_eq!(ext.server_id(), Some("plugin:p/srv"));
    }

    #[tokio::test]
    async fn manager_backed_caller_maps_success_content() {
        // ManagerBackedMcpCaller maps McpToolResult{success,content} → content Value.
        // Uses a stub that bypasses the real handle by testing the mapping helper.
        let ok = crate::mcp::types::McpToolResult::success(json!({"text": "hi"}));
        let mapped = ManagerBackedMcpCaller::map_result(ok).unwrap();
        assert_eq!(mapped, json!({"text": "hi"}));
        let err = crate::mcp::types::McpToolResult::error("boom");
        assert!(ManagerBackedMcpCaller::map_result(err).is_err());
    }
}
