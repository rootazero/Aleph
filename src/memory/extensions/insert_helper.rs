//! Bridge helper: runs `on_capture` dispatch then persists the raw memory
//! only if the chain returns Allow. Callers should prefer this helper over
//! calling `RawMemoryStore::insert_raw_memory` directly.

use crate::error::AlephError;
use crate::memory::extensions::registry::MemoryExtensionRegistry;
use crate::memory::extensions::types::{CaptureCtx, CaptureDecision};
use crate::memory::store::raw_memory::{RawMemory, RawMemoryStore};
use crate::sync_primitives::Arc;

pub async fn insert_with_capture_filter(
    store: &Arc<dyn RawMemoryStore>,
    registry: &Arc<MemoryExtensionRegistry>,
    ctx: &CaptureCtx,
    mut raw: RawMemory,
) -> Result<CaptureDecision, AlephError> {
    let decision = registry.dispatch_on_capture(ctx, &mut raw).await?;
    match &decision {
        CaptureDecision::Allow => {
            store.insert_raw_memory(&raw).await?;
        }
        CaptureDecision::Block { reason } => {
            tracing::info!(
                "raw_memory blocked by extension pipeline (reason={reason}, \
                 source_hint={source}, agent={agent})",
                source = ctx.source_hint,
                agent = ctx.agent_id,
            );
        }
    }
    Ok(decision)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::extensions::traits::MemoryExtension;
    use crate::memory::namespace::NamespaceScope;
    use crate::memory::store::raw_memory::RawMemorySource;
    use crate::sync_primitives::Mutex;
    use async_trait::async_trait;

    struct FakeStore(Mutex<Vec<RawMemory>>);
    #[async_trait]
    impl RawMemoryStore for FakeStore {
        async fn insert_raw_memory(&self, raw: &RawMemory) -> Result<(), AlephError> {
            self.0.lock().unwrap().push(raw.clone());
            Ok(())
        }
        async fn get_unprocessed_raw_memories(
            &self,
            _: &str,
            _: usize,
        ) -> Result<Vec<RawMemory>, AlephError> {
            Ok(vec![])
        }
        async fn mark_raw_as_processed(&self, _: &[String]) -> Result<usize, AlephError> {
            Ok(0)
        }
        async fn count_unprocessed(&self, _: &str) -> Result<usize, AlephError> {
            Ok(0)
        }
        async fn get_raw_by_path_prefix(
            &self,
            _: &str,
            _: &str,
            _: usize,
        ) -> Result<Vec<RawMemory>, AlephError> {
            Ok(vec![])
        }
    }

    struct BlockExt;
    #[async_trait]
    impl MemoryExtension for BlockExt {
        fn name(&self) -> &str {
            "test.block"
        }
        async fn on_capture(
            &self,
            _ctx: &CaptureCtx,
            _raw: &mut RawMemory,
        ) -> Result<CaptureDecision, AlephError> {
            Ok(CaptureDecision::Block {
                reason: "test-block".into(),
            })
        }
    }

    struct PrefixExt;
    #[async_trait]
    impl MemoryExtension for PrefixExt {
        fn name(&self) -> &str {
            "test.prefix"
        }
        async fn on_capture(
            &self,
            _ctx: &CaptureCtx,
            raw: &mut RawMemory,
        ) -> Result<CaptureDecision, AlephError> {
            raw.content = format!("[X] {}", raw.content);
            Ok(CaptureDecision::Allow)
        }
    }

    fn ctx() -> CaptureCtx {
        CaptureCtx {
            agent_id: "a".into(),
            namespace: NamespaceScope::Owner,
            session_id: None,
            source_hint: "transcript".into(),
        }
    }

    fn raw() -> RawMemory {
        RawMemory::new("hi".into(), RawMemorySource::Transcript)
    }

    #[tokio::test]
    async fn no_extensions_allows_and_persists() {
        let store_inner = Arc::new(FakeStore(Mutex::new(Vec::new())));
        let store: Arc<dyn RawMemoryStore> = store_inner.clone();
        let reg = Arc::new(MemoryExtensionRegistry::new());
        let d = insert_with_capture_filter(&store, &reg, &ctx(), raw())
            .await
            .unwrap();
        assert!(matches!(d, CaptureDecision::Allow));
        assert_eq!(store_inner.0.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn block_extension_prevents_persistence() {
        let store_inner = Arc::new(FakeStore(Mutex::new(Vec::new())));
        let store: Arc<dyn RawMemoryStore> = store_inner.clone();
        let reg = MemoryExtensionRegistry::new();
        reg.register(Arc::new(BlockExt)).unwrap();
        let reg = Arc::new(reg);
        let d = insert_with_capture_filter(&store, &reg, &ctx(), raw())
            .await
            .unwrap();
        assert!(matches!(d, CaptureDecision::Block { .. }));
        assert_eq!(store_inner.0.lock().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn prefix_extension_mutates_persisted_row() {
        let store_inner = Arc::new(FakeStore(Mutex::new(Vec::new())));
        let store: Arc<dyn RawMemoryStore> = store_inner.clone();
        let reg = MemoryExtensionRegistry::new();
        reg.register(Arc::new(PrefixExt)).unwrap();
        let reg = Arc::new(reg);
        let d = insert_with_capture_filter(&store, &reg, &ctx(), raw())
            .await
            .unwrap();
        assert!(matches!(d, CaptureDecision::Allow));
        let stored = store_inner.0.lock().unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].content, "[X] hi");
    }
}
