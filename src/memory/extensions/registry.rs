//! Registry + dispatch for MemoryExtension hooks.

use crate::error::AlephError;
use crate::memory::assembler::envelope::MemoryEnvelope;
use crate::memory::extensions::traits::MemoryExtension;
use crate::memory::extensions::types::{CaptureCtx, CaptureDecision, ProduceCtx, RetrieveCtx};
use crate::memory::store::raw_memory::RawMemory;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::time::timeout;
use tracing::warn;

pub const ON_RETRIEVE_TIMEOUT: Duration = Duration::from_secs(2);
pub const ON_CAPTURE_TIMEOUT: Duration = Duration::from_secs(3);
pub const PRODUCE_TIMEOUT: Duration = Duration::from_secs(30);

/// Registry for memory extension hooks with interior mutability.
///
/// Uses `RwLock` so that concurrent plugin loaders can call `register` safely
/// while dispatch methods hold a snapshot of the extensions list (dropping
/// the lock before any await points).
#[derive(Default)]
pub struct MemoryExtensionRegistry {
    /// Extensions in registration order (for on_capture this is the chain order).
    extensions: RwLock<Vec<Arc<dyn MemoryExtension>>>,
}

impl Clone for MemoryExtensionRegistry {
    fn clone(&self) -> Self {
        let snapshot = self
            .extensions
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        Self {
            extensions: RwLock::new(snapshot),
        }
    }
}

impl MemoryExtensionRegistry {
    pub fn new() -> Self {
        Self {
            extensions: RwLock::new(Vec::new()),
        }
    }

    /// Register an extension. Safe to call concurrently from multiple loaders.
    pub fn register(&self, ext: Arc<dyn MemoryExtension>) {
        self.extensions
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .push(ext);
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
            .clone()
    }

    /// on_retrieve: sequential broadcast. Each extension sees the current
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

    /// on_capture: chained pipeline. Each extension's modification of `raw`
    /// is visible to the next. A Block short-circuits. Error or timeout =>
    /// Block (fail-safe).
    pub async fn dispatch_on_capture(
        &self,
        ctx: &CaptureCtx,
        raw: &mut RawMemory,
    ) -> Result<CaptureDecision, AlephError> {
        for ext in self.snapshot() {
            let name = ext.name().to_string();
            match timeout(ON_CAPTURE_TIMEOUT, ext.on_capture(ctx, raw)).await {
                Ok(Ok(CaptureDecision::Allow)) => continue,
                Ok(Ok(blk @ CaptureDecision::Block { .. })) => {
                    warn!("memory extension '{name}' blocked raw memory");
                    return Ok(blk);
                }
                Ok(Err(e)) => {
                    warn!(
                        "memory extension '{name}' on_capture errored: {e} — blocking for safety"
                    );
                    return Ok(CaptureDecision::Block {
                        reason: format!("extension '{name}' errored: {e}"),
                    });
                }
                Err(_) => {
                    warn!("memory extension '{name}' on_capture timed out — blocking");
                    return Ok(CaptureDecision::Block {
                        reason: format!("extension '{name}' timeout"),
                    });
                }
            }
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
        assert_eq!(raw.content, "hi", "content must not be modified after Block");
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
}
