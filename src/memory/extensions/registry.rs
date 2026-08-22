//! Registry + dispatch for `MemoryExtension` hooks.

use crate::error::AlephError;
use crate::memory::assembler::envelope::MemoryEnvelope;
use crate::memory::extensions::traits::MemoryExtension;
use crate::memory::extensions::types::{
    CaptureCtx, CaptureDecision, DelegationCtx, PreCompressCtx, ProduceCtx, RetrieveCtx,
    SessionSwitchCtx,
};
use crate::memory::store::raw_memory::RawMemory;
use crate::sync_primitives::{Arc, RwLock};
use std::time::Duration;
use tokio::time::timeout;
use tracing::{error, warn};

pub const ON_RETRIEVE_TIMEOUT: Duration = Duration::from_secs(2);
pub const ON_CAPTURE_TIMEOUT: Duration = Duration::from_secs(3);
pub const PRODUCE_TIMEOUT: Duration = Duration::from_secs(30);
/// Session switch fires on the synchronous hot path (`/resume`, `/branch`,
/// compress). Keep it short — extensions should refresh cached state, not do
/// heavy work.
pub const ON_SESSION_SWITCH_TIMEOUT: Duration = Duration::from_secs(1);
/// Pre-compress runs inside the compression pipeline; extensions may do
/// modest LLM-free extraction, but should not call external services.
pub const ON_PRE_COMPRESS_TIMEOUT: Duration = Duration::from_secs(5);
/// Delegation fires on subagent completion; the parent has already received
/// the result. Extensions may persist annotations but should not block.
pub const ON_DELEGATION_TIMEOUT: Duration = Duration::from_secs(3);

/// Registry for memory extension hooks with interior mutability.
///
/// Uses `RwLock` so that concurrent plugin loaders can call `register` safely
/// while dispatch methods hold a snapshot of the extensions list (dropping
/// the lock before any await points).
#[derive(Default)]
pub struct MemoryExtensionRegistry {
    /// Extensions in registration order (for `on_capture` this is the chain order).
    extensions: RwLock<Vec<Arc<dyn MemoryExtension>>>,
    /// Typed side-table of MCP-backed extensions, retained at their concrete
    /// type so the boot-time bind pass can call `rebind`. Each entry is the
    /// SAME `Arc` as the corresponding `dyn MemoryExtension` in `extensions`,
    /// so a rebind is immediately visible to dispatch.
    mcp_bindings: RwLock<Vec<Arc<crate::memory::extensions::mcp_adapter::McpMemoryExtension>>>,
}

impl Clone for MemoryExtensionRegistry {
    fn clone(&self) -> Self {
        // rust-doctor-disable-next-line excessive-clone
        let snapshot = self
            .extensions
            .read()
            .unwrap_or_else(|e| e.into_inner())
            // rust-doctor-disable-next-line excessive-clone
            .clone();
        // rust-doctor-disable-next-line excessive-clone
        let mcp = self
            .mcp_bindings
            .read()
            .unwrap_or_else(|e| e.into_inner())
            // rust-doctor-disable-next-line excessive-clone
            .clone();
        Self {
            extensions: RwLock::new(snapshot),
            mcp_bindings: RwLock::new(mcp),
        }
    }
}

impl MemoryExtensionRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self {
            extensions: RwLock::new(Vec::new()),
            mcp_bindings: RwLock::new(Vec::new()),
        }
    }

    /// Register an extension. Safe to call concurrently from multiple loaders.
    pub fn register(&self, ext: Arc<dyn MemoryExtension>) {
        self.extensions
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .push(ext);
    }

    /// Register an MCP-backed extension. It lands in BOTH the dispatch list
    /// (as `dyn MemoryExtension`) and the typed side-table (as the concrete
    /// `McpMemoryExtension`), sharing one `Arc` so a later `rebind` on the
    /// side-table entry is visible to dispatch.
    pub fn register_mcp(
        &self,
        ext: Arc<crate::memory::extensions::mcp_adapter::McpMemoryExtension>,
    ) {
        self.extensions
            .write()
            .unwrap_or_else(|e| e.into_inner())
            // rust-doctor-disable-next-line excessive-clone
            .push(ext.clone() as Arc<dyn MemoryExtension>);
        self.mcp_bindings
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .push(ext);
    }

    /// Snapshot the MCP-backed extensions for the boot-time bind pass. The
    /// lock is released before the caller does any async work.
    pub fn mcp_bindings_snapshot(
        &self,
    ) -> Vec<Arc<crate::memory::extensions::mcp_adapter::McpMemoryExtension>> {
        self.mcp_bindings
            .read()
            .unwrap_or_else(|e| e.into_inner())
            // rust-doctor-disable-next-line excessive-clone
            .clone()
    }

    pub fn len(&self) -> usize {
        self.extensions
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.extensions
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty()
    }

    /// Snapshot the current extension list. The lock is released before
    /// any async dispatch to avoid holding it across await points.
    fn snapshot(&self) -> Vec<Arc<dyn MemoryExtension>> {
        self.extensions
            .read()
            .unwrap_or_else(|e| e.into_inner())
            // rust-doctor-disable-next-line excessive-clone
            .clone()
    }

    /// `on_retrieve`: sequential broadcast. Each extension sees the current
    /// (possibly-mutated) envelope. Timeouts drop that plugin's work without
    /// failing the call.
    pub async fn dispatch_on_retrieve(
        &self,
        ctx: &RetrieveCtx,
        envelope: &mut MemoryEnvelope,
    ) -> Result<(), AlephError> {
        for ext in self.snapshot() {
            let name = ext.name().to_string();
            match timeout(ON_RETRIEVE_TIMEOUT, ext.on_retrieve(ctx, envelope)).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => warn!("memory extension '{name}' on_retrieve failed: {e}"),
                Err(_) => warn!("memory extension '{name}' on_retrieve timed out"),
            }
        }
        Ok(())
    }

    /// `on_capture`: chained pipeline. Each extension's modification of `raw`
    /// is visible to the next.
    ///
    /// Decision policy (fail-safe without being a single point of failure):
    /// - A explicit `Block` from any extension short-circuits — the most
    ///   restrictive verdict wins.
    /// - A single extension erroring or timing out does NOT block the whole
    ///   chain. Previously this returned Block immediately, so one buggy or
    ///   misbehaving extension silently broke memory writes for every
    ///   `insert_with_capture_filter` call site — with only a `tracing::warn!`
    ///   to indicate the culprit.
    /// - Errors/timeouts are recorded; if the chain finishes without any
    ///   explicit Allow or Block, and at least one extension errored, the
    ///   combined failure becomes a single Block so we still fail closed.
    pub async fn dispatch_on_capture(
        &self,
        ctx: &CaptureCtx,
        raw: &mut RawMemory,
    ) -> Result<CaptureDecision, AlephError> {
        let mut failures: Vec<String> = Vec::new();
        for ext in self.snapshot() {
            let name = ext.name().to_string();
            match timeout(ON_CAPTURE_TIMEOUT, ext.on_capture(ctx, raw)).await {
                Ok(Ok(CaptureDecision::Allow)) => continue,
                Ok(Ok(blk @ CaptureDecision::Block { .. })) => {
                    warn!("memory extension '{name}' blocked raw memory");
                    return Ok(blk);
                }
                Ok(Err(e)) => {
                    error!(
                        "memory extension '{name}' on_capture errored: {e} \
                         — continuing with other extensions"
                    );
                    failures.push(format!("'{name}' errored: {e}"));
                }
                Err(_) => {
                    error!(
                        "memory extension '{name}' on_capture timed out — \
                         continuing with other extensions"
                    );
                    failures.push(format!("'{name}' timed out"));
                }
            }
        }
        if !failures.is_empty() {
            return Ok(CaptureDecision::Block {
                reason: format!(
                    "{} extension(s) failed: {}",
                    failures.len(),
                    failures.join("; ")
                ),
            });
        }
        Ok(CaptureDecision::Allow)
    }

    /// produce: independent per-plugin calls. Returns per-plugin results so
    /// the scheduler can count consecutive failures per plugin.
    pub async fn dispatch_produce(
        &self,
        ctx: &ProduceCtx,
    ) -> Vec<(String, Result<Vec<RawMemory>, AlephError>)> {
        let mut out = Vec::with_capacity(self.len());
        for ext in self.snapshot() {
            let name = ext.name().to_string();
            let result = match timeout(PRODUCE_TIMEOUT, ext.produce(ctx)).await {
                Ok(r) => r,
                Err(_) => Err(AlephError::other(format!(
                    "extension '{name}' produce timeout"
                ))),
            };
            out.push((name, result));
        }
        out
    }

    /// `on_session_switch`: sequential broadcast. Failures and timeouts are
    /// logged and skipped — they never block a session rotation.
    ///
    /// **No Aleph-side producer (by design, X1).** Aleph sessions are created
    /// fresh, compacted in place, or deleted — none rotate a session id
    /// mid-process, so there is no event matching this hook's contract. The
    /// hook stays part of the extension API surface (third-party MCP `[memory]`
    /// plugins may implement `memory.on_session_switch`); wire an Aleph
    /// producer here only if a real session-rotation event is introduced.
    pub async fn dispatch_on_session_switch(&self, ctx: &SessionSwitchCtx) {
        for ext in self.snapshot() {
            let name = ext.name().to_string();
            match timeout(ON_SESSION_SWITCH_TIMEOUT, ext.on_session_switch(ctx)).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => warn!("memory extension '{name}' on_session_switch failed: {e}"),
                Err(_) => warn!("memory extension '{name}' on_session_switch timed out"),
            }
        }
    }

    /// `on_pre_compress`: sequential broadcast. Returns each extension's
    /// contribution joined with a blank line. An empty result means no
    /// extension contributed — callers should treat that as "no extra
    /// summary context", not a failure.
    ///
    /// Wired at `CompressionService::compress_to_notes`
    /// (`src/memory/compression/service.rs`): the returned text is folded into
    /// the ingest prompt as extra context before the LLM extract step.
    pub async fn dispatch_on_pre_compress(&self, ctx: &PreCompressCtx) -> String {
        let mut parts: Vec<String> = Vec::new();
        for ext in self.snapshot() {
            let name = ext.name().to_string();
            match timeout(ON_PRE_COMPRESS_TIMEOUT, ext.on_pre_compress(ctx)).await {
                Ok(Ok(s)) => {
                    let trimmed = s.trim();
                    if !trimmed.is_empty() {
                        parts.push(trimmed.to_string());
                    }
                }
                Ok(Err(e)) => warn!("memory extension '{name}' on_pre_compress failed: {e}"),
                Err(_) => warn!("memory extension '{name}' on_pre_compress timed out"),
            }
        }
        parts.join("\n\n")
    }

    /// `on_delegation`: sequential broadcast. Fires on parent-side completion
    /// of a subagent run. Failures/timeouts are logged and skipped.
    ///
    /// Wired at the subagent spawn site (`src/agents/subagent_tool/spawn.rs`):
    /// fired fire-and-forget with a trimmed `result_summary` after the child
    /// run completes (or panics/errors, surfaced as `(error) …`).
    pub async fn dispatch_on_delegation(&self, ctx: &DelegationCtx) {
        for ext in self.snapshot() {
            let name = ext.name().to_string();
            match timeout(ON_DELEGATION_TIMEOUT, ext.on_delegation(ctx)).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => warn!("memory extension '{name}' on_delegation failed: {e}"),
                Err(_) => warn!("memory extension '{name}' on_delegation timed out"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::assembler::envelope::{EnvelopeMeta, MemoryEnvelope};
    use crate::memory::namespace::NamespaceScope;
    use crate::memory::store::raw_memory::RawMemorySource;
    use async_trait::async_trait;

    // --- Stub extensions ---

    struct NoopExt;
    #[async_trait]
    impl MemoryExtension for NoopExt {
        fn name(&self) -> &str {
            "test.noop"
        }
    }

    struct AppendQueryExt;
    #[async_trait]
    impl MemoryExtension for AppendQueryExt {
        fn name(&self) -> &str {
            "test.append_query"
        }
        async fn on_retrieve(
            &self,
            _ctx: &RetrieveCtx,
            envelope: &mut MemoryEnvelope,
        ) -> Result<(), AlephError> {
            envelope.query.push_str(" +ext");
            Ok(())
        }
    }

    struct BlockingExt;
    #[async_trait]
    impl MemoryExtension for BlockingExt {
        fn name(&self) -> &str {
            "test.blocker"
        }
        async fn on_capture(
            &self,
            _ctx: &CaptureCtx,
            _raw: &mut RawMemory,
        ) -> Result<CaptureDecision, AlephError> {
            Ok(CaptureDecision::Block {
                reason: "test".into(),
            })
        }
    }

    struct PrefixContentExt;
    #[async_trait]
    impl MemoryExtension for PrefixContentExt {
        fn name(&self) -> &str {
            "test.prefix"
        }
        async fn on_capture(
            &self,
            _ctx: &CaptureCtx,
            raw: &mut RawMemory,
        ) -> Result<CaptureDecision, AlephError> {
            raw.content = format!("[P] {}", raw.content);
            Ok(CaptureDecision::Allow)
        }
    }

    struct StubProducerExt;
    #[async_trait]
    impl MemoryExtension for StubProducerExt {
        fn name(&self) -> &str {
            "test.producer"
        }
        async fn produce(&self, _ctx: &ProduceCtx) -> Result<Vec<RawMemory>, AlephError> {
            Ok(vec![RawMemory::new(
                "produced".into(),
                RawMemorySource::Transcript,
            )])
        }
    }

    fn retrieve_ctx() -> RetrieveCtx {
        RetrieveCtx {
            agent_id: "a".into(),
            namespace: NamespaceScope::Owner,
            query: "original".into(),
            session_id: None,
        }
    }

    fn capture_ctx() -> CaptureCtx {
        CaptureCtx {
            agent_id: "a".into(),
            namespace: NamespaceScope::Owner,
            session_id: None,
            source_hint: "transcript".into(),
        }
    }

    fn produce_ctx() -> ProduceCtx {
        ProduceCtx {
            agent_id: "a".into(),
            namespace: NamespaceScope::Owner,
            tick: 0,
        }
    }

    fn make_envelope() -> MemoryEnvelope {
        MemoryEnvelope {
            schema_version: "1.0".into(),
            generated_at: 0,
            query: "original".into(),
            agent_id: "a".into(),
            session_id: None,
            slots: vec![],
            meta: EnvelopeMeta {
                strategy: "hybrid_v1".into(),
                candidates_considered: 0,
                used_fallback: false,
                fallback_reason: None,
                llm_rerank_latency_ms: None,
                total_latency_ms: 0,
            },
        }
    }

    fn make_raw() -> RawMemory {
        RawMemory::new("hi".into(), RawMemorySource::Transcript)
    }

    #[tokio::test]
    async fn empty_registry_on_retrieve_is_noop() {
        let reg = MemoryExtensionRegistry::new();
        let mut env = make_envelope();
        let before = env.query.clone();
        reg.dispatch_on_retrieve(&retrieve_ctx(), &mut env)
            .await
            .unwrap();
        assert_eq!(env.query, before);
    }

    #[tokio::test]
    async fn on_retrieve_broadcast_applies_each_extension() {
        let reg = MemoryExtensionRegistry::new();
        reg.register(Arc::new(AppendQueryExt));
        reg.register(Arc::new(AppendQueryExt));
        let mut env = make_envelope();
        env.query = "q".into();
        reg.dispatch_on_retrieve(&retrieve_ctx(), &mut env)
            .await
            .unwrap();
        assert_eq!(env.query, "q +ext +ext");
    }

    #[tokio::test]
    async fn empty_registry_on_capture_allows() {
        let reg = MemoryExtensionRegistry::new();
        let mut raw = make_raw();
        let decision = reg
            .dispatch_on_capture(&capture_ctx(), &mut raw)
            .await
            .unwrap();
        assert!(matches!(decision, CaptureDecision::Allow));
    }

    #[tokio::test]
    async fn on_capture_chain_short_circuits_on_block() {
        let reg = MemoryExtensionRegistry::new();
        reg.register(Arc::new(BlockingExt));
        reg.register(Arc::new(PrefixContentExt));
        let mut raw = make_raw();
        let decision = reg
            .dispatch_on_capture(&capture_ctx(), &mut raw)
            .await
            .unwrap();
        assert!(matches!(decision, CaptureDecision::Block { .. }));
        assert_eq!(
            raw.content, "hi",
            "content must not be modified after Block"
        );
    }

    #[tokio::test]
    async fn on_capture_chain_mutates_raw_in_order() {
        let reg = MemoryExtensionRegistry::new();
        reg.register(Arc::new(PrefixContentExt));
        reg.register(Arc::new(PrefixContentExt));
        let mut raw = make_raw();
        let decision = reg
            .dispatch_on_capture(&capture_ctx(), &mut raw)
            .await
            .unwrap();
        assert!(matches!(decision, CaptureDecision::Allow));
        assert_eq!(raw.content, "[P] [P] hi");
    }

    #[tokio::test]
    async fn empty_registry_produce_returns_empty() {
        let reg = MemoryExtensionRegistry::new();
        let out = reg.dispatch_produce(&produce_ctx()).await;
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn produce_returns_per_plugin_results() {
        let reg = MemoryExtensionRegistry::new();
        reg.register(Arc::new(StubProducerExt));
        reg.register(Arc::new(NoopExt));
        let out = reg.dispatch_produce(&produce_ctx()).await;
        assert_eq!(out.len(), 2);
        let first = out.iter().find(|(n, _)| n == "test.producer").unwrap();
        assert_eq!(first.1.as_ref().unwrap().len(), 1);
        let second = out.iter().find(|(n, _)| n == "test.noop").unwrap();
        assert_eq!(second.1.as_ref().unwrap().len(), 0);
    }

    // ------------------------------------------------------------------
    // hermes-parity hook tests: on_session_switch / on_pre_compress /
    // on_delegation. Each verifies (a) the default no-op extension
    // behaviour, (b) custom override invocation, and (c) failure/timeout
    // isolation (one extension never blocks the next).
    // ------------------------------------------------------------------

    use super::super::types::{
        DelegationCtx, PreCompressCtx, SessionSwitchCtx, SessionSwitchReason,
    };
    use crate::sync_primitives::Mutex;

    struct RecordSwitchExt {
        seen: Arc<Mutex<Vec<String>>>,
    }
    #[async_trait]
    impl MemoryExtension for RecordSwitchExt {
        fn name(&self) -> &str {
            "test.record_switch"
        }
        async fn on_session_switch(&self, ctx: &SessionSwitchCtx) -> Result<(), AlephError> {
            self.seen
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(ctx.new_session_id.clone());
            Ok(())
        }
    }

    struct FailingSwitchExt;
    #[async_trait]
    impl MemoryExtension for FailingSwitchExt {
        fn name(&self) -> &str {
            "test.fail_switch"
        }
        async fn on_session_switch(&self, _ctx: &SessionSwitchCtx) -> Result<(), AlephError> {
            Err(AlephError::other("boom"))
        }
    }

    struct SlowSwitchExt;
    #[async_trait]
    impl MemoryExtension for SlowSwitchExt {
        fn name(&self) -> &str {
            "test.slow_switch"
        }
        async fn on_session_switch(&self, _ctx: &SessionSwitchCtx) -> Result<(), AlephError> {
            // Sleep for longer than ON_SESSION_SWITCH_TIMEOUT.
            tokio::time::sleep(Duration::from_secs(3)).await;
            Ok(())
        }
    }

    struct PreCompressContribExt {
        text: &'static str,
    }
    #[async_trait]
    impl MemoryExtension for PreCompressContribExt {
        fn name(&self) -> &str {
            "test.pre_compress_contrib"
        }
        async fn on_pre_compress(&self, _ctx: &PreCompressCtx) -> Result<String, AlephError> {
            Ok(self.text.to_string())
        }
    }

    struct FailingPreCompressExt;
    #[async_trait]
    impl MemoryExtension for FailingPreCompressExt {
        fn name(&self) -> &str {
            "test.fail_pre_compress"
        }
        async fn on_pre_compress(&self, _ctx: &PreCompressCtx) -> Result<String, AlephError> {
            Err(AlephError::other("nope"))
        }
    }

    struct RecordDelegationExt {
        seen: Arc<Mutex<Vec<(String, String)>>>,
    }
    #[async_trait]
    impl MemoryExtension for RecordDelegationExt {
        fn name(&self) -> &str {
            "test.record_delegation"
        }
        async fn on_delegation(&self, ctx: &DelegationCtx) -> Result<(), AlephError> {
            self.seen
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((ctx.task.clone(), ctx.result_summary.clone()));
            Ok(())
        }
    }

    fn switch_ctx(new_sid: &str) -> SessionSwitchCtx {
        SessionSwitchCtx {
            agent_id: "a".into(),
            namespace: NamespaceScope::Owner,
            new_session_id: new_sid.into(),
            parent_session_id: None,
            reason: SessionSwitchReason::Resume,
        }
    }

    fn pre_compress_ctx() -> PreCompressCtx {
        PreCompressCtx {
            agent_id: "a".into(),
            namespace: NamespaceScope::Owner,
            session_id: Some("s".into()),
            messages_count: 12,
            oldest_at: None,
            newest_at: None,
        }
    }

    fn delegation_ctx(task: &str, result: &str) -> DelegationCtx {
        DelegationCtx {
            agent_id: "a".into(),
            namespace: NamespaceScope::Owner,
            parent_session_id: "p".into(),
            child_session_id: "c".into(),
            task: task.into(),
            result_summary: result.into(),
        }
    }

    #[tokio::test]
    async fn empty_registry_on_session_switch_is_noop() {
        let reg = MemoryExtensionRegistry::new();
        // Must complete without panicking; no return value to inspect.
        reg.dispatch_on_session_switch(&switch_ctx("s1")).await;
    }

    #[tokio::test]
    async fn on_session_switch_broadcast_records_new_id() {
        let reg = MemoryExtensionRegistry::new();
        let seen = Arc::new(Mutex::new(Vec::new()));
        reg.register(Arc::new(RecordSwitchExt { seen: seen.clone() }));
        reg.dispatch_on_session_switch(&switch_ctx("s42")).await;
        let observed = seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert_eq!(observed, vec!["s42".to_string()]);
    }

    #[tokio::test]
    async fn on_session_switch_failure_does_not_block_other_extensions() {
        let reg = MemoryExtensionRegistry::new();
        let seen = Arc::new(Mutex::new(Vec::new()));
        reg.register(Arc::new(FailingSwitchExt));
        reg.register(Arc::new(RecordSwitchExt { seen: seen.clone() }));
        reg.dispatch_on_session_switch(&switch_ctx("s99")).await;
        let observed = seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert_eq!(
            observed,
            vec!["s99".to_string()],
            "failing extension must not prevent later extensions from running"
        );
    }

    #[tokio::test]
    async fn on_session_switch_slow_extension_times_out_without_blocking_others() {
        let reg = MemoryExtensionRegistry::new();
        let seen = Arc::new(Mutex::new(Vec::new()));
        reg.register(Arc::new(SlowSwitchExt));
        reg.register(Arc::new(RecordSwitchExt { seen: seen.clone() }));
        let start = std::time::Instant::now();
        reg.dispatch_on_session_switch(&switch_ctx("s_timeout"))
            .await;
        let elapsed = start.elapsed();
        // Should finish in roughly ON_SESSION_SWITCH_TIMEOUT (1s), not the
        // 3s the slow extension wanted. Allow generous slack for CI.
        assert!(
            elapsed < Duration::from_millis(2500),
            "session_switch must respect timeout; elapsed={:?}",
            elapsed
        );
        let observed = seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert_eq!(observed, vec!["s_timeout".to_string()]);
    }

    #[tokio::test]
    async fn empty_registry_on_pre_compress_returns_empty_string() {
        let reg = MemoryExtensionRegistry::new();
        let out = reg.dispatch_on_pre_compress(&pre_compress_ctx()).await;
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn on_pre_compress_joins_non_empty_contributions() {
        let reg = MemoryExtensionRegistry::new();
        reg.register(Arc::new(PreCompressContribExt { text: "  one  " }));
        reg.register(Arc::new(PreCompressContribExt { text: "" }));
        reg.register(Arc::new(PreCompressContribExt { text: "two" }));
        let out = reg.dispatch_on_pre_compress(&pre_compress_ctx()).await;
        assert_eq!(
            out, "one\n\ntwo",
            "empty contributions must be dropped; non-empty joined by blank line"
        );
    }

    #[tokio::test]
    async fn on_pre_compress_failure_does_not_drop_other_contribs() {
        let reg = MemoryExtensionRegistry::new();
        reg.register(Arc::new(FailingPreCompressExt));
        reg.register(Arc::new(PreCompressContribExt { text: "kept" }));
        let out = reg.dispatch_on_pre_compress(&pre_compress_ctx()).await;
        assert_eq!(out, "kept");
    }

    #[tokio::test]
    async fn empty_registry_on_delegation_is_noop() {
        let reg = MemoryExtensionRegistry::new();
        reg.dispatch_on_delegation(&delegation_ctx("t", "r")).await;
    }

    #[tokio::test]
    async fn on_delegation_broadcast_records_task_result() {
        let reg = MemoryExtensionRegistry::new();
        let seen = Arc::new(Mutex::new(Vec::new()));
        reg.register(Arc::new(RecordDelegationExt { seen: seen.clone() }));
        reg.dispatch_on_delegation(&delegation_ctx("ship it", "shipped"))
            .await;
        let observed = seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert_eq!(
            observed,
            vec![("ship it".to_string(), "shipped".to_string())]
        );
    }

    #[tokio::test]
    async fn register_mcp_appears_in_both_dispatch_and_snapshot() {
        use crate::memory::extensions::mcp_adapter::McpMemoryExtension;
        let reg = MemoryExtensionRegistry::new();
        let ext = Arc::new(McpMemoryExtension::new_unbound(
            "p".to_string(),
            Some("plugin:p/srv".to_string()),
        ));
        reg.register_mcp(ext);
        // Visible to dispatch (main list).
        assert_eq!(reg.len(), 1);
        // Visible to the typed side-table for binding.
        let snap = reg.mcp_bindings_snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].server_id(), Some("plugin:p/srv"));
    }
}
