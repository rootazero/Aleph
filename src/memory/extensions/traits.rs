//! MemoryExtension trait + default no-op implementations.

use crate::error::AlephError;
use crate::memory::assembler::envelope::MemoryEnvelope;
use crate::memory::extensions::types::{CaptureCtx, CaptureDecision, ProduceCtx, RetrieveCtx};
use crate::memory::store::raw_memory::RawMemory;
use async_trait::async_trait;

/// Hook surface for first-party code and MCP plugins. Each method has a
/// default no-op body so an extension only implements what it needs.
#[async_trait]
pub trait MemoryExtension: Send + Sync {
    /// Stable identifier — shows up in logs and manifest entries.
    fn name(&self) -> &str;

    /// Modify the envelope after HybridAssembler::assemble.
    async fn on_retrieve(
        &self,
        _ctx: &RetrieveCtx,
        _envelope: &mut MemoryEnvelope,
    ) -> Result<(), AlephError> {
        Ok(())
    }

    /// Inspect / modify / veto a raw memory before persistence.
    async fn on_capture(
        &self,
        _ctx: &CaptureCtx,
        _raw: &mut RawMemory,
    ) -> Result<CaptureDecision, AlephError> {
        Ok(CaptureDecision::Allow)
    }

    /// Produce raw memories on the caller's schedule.
    async fn produce(
        &self,
        _ctx: &ProduceCtx,
    ) -> Result<Vec<RawMemory>, AlephError> {
        Ok(vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::namespace::NamespaceScope;
    use crate::memory::store::raw_memory::RawMemorySource;

    struct NoopExt;

    #[async_trait]
    impl MemoryExtension for NoopExt {
        fn name(&self) -> &str {
            "test.noop"
        }
    }

    #[tokio::test]
    async fn default_on_capture_allows() {
        let ext = NoopExt;
        let ctx = CaptureCtx {
            agent_id: "a".into(),
            namespace: NamespaceScope::Owner,
            session_id: None,
            source_hint: "transcript".into(),
        };
        let mut raw = RawMemory::new("hi".into(), RawMemorySource::Transcript);
        let decision = ext.on_capture(&ctx, &mut raw).await.unwrap();
        assert!(matches!(decision, CaptureDecision::Allow));
    }

    #[tokio::test]
    async fn default_produce_returns_empty() {
        let ext = NoopExt;
        let ctx = ProduceCtx {
            agent_id: "a".into(),
            namespace: NamespaceScope::Owner,
            tick: 0,
        };
        let out = ext.produce(&ctx).await.unwrap();
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn trait_is_object_safe() {
        // Compile-time check: dyn MemoryExtension is usable.
        let _ext: std::sync::Arc<dyn MemoryExtension> = std::sync::Arc::new(NoopExt);
    }
}
