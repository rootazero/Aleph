//! Dedicated tokio task that periodically invokes `MemoryExtension::produce`
//! for all registered plugins. Produced memories go through
//! `insert_with_capture_filter` so `on_capture` still applies.

use crate::memory::extensions::insert_helper::insert_with_capture_filter;
use crate::memory::extensions::registry::MemoryExtensionRegistry;
use crate::memory::extensions::types::{CaptureCtx, ProduceCtx};
use crate::memory::namespace::NamespaceScope;
use crate::memory::store::raw_memory::RawMemoryStore;
use crate::routing::DEFAULT_AGENT_ID;
use crate::sync_primitives::Arc;
use crate::sync_primitives::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::time::interval;
use tracing::{debug, warn};

pub const DEFAULT_TICK_SECONDS: u64 = 10;

pub struct MemoryProducerScheduler {
    registry: Arc<MemoryExtensionRegistry>,
    raw_store: Arc<dyn RawMemoryStore>,
    tick_duration: Duration,
    tick_counter: AtomicU64,
}

impl MemoryProducerScheduler {
    pub fn new(registry: Arc<MemoryExtensionRegistry>, raw_store: Arc<dyn RawMemoryStore>) -> Self {
        Self {
            registry,
            raw_store,
            tick_duration: Duration::from_secs(DEFAULT_TICK_SECONDS),
            tick_counter: AtomicU64::new(0),
        }
    }

    pub const fn with_tick_duration(mut self, d: Duration) -> Self {
        self.tick_duration = d;
        self
    }

    /// Spawn the tokio background task. Returns a `JoinHandle` so the caller
    /// can abort it on shutdown.
    pub fn spawn(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        // rust-doctor-disable-next-line excessive-clone
        let this = self.clone();
        tokio::spawn(async move {
            let mut tick_interval = interval(this.tick_duration);
            tick_interval.tick().await; // burn the immediate first tick
            loop {
                tick_interval.tick().await;
                if let Err(e) = this.run_once().await {
                    warn!("memory producer scheduler tick errored: {e}");
                }
            }
        })
    }

    /// Run a single tick. Exposed for integration testing.
    pub async fn run_once(&self) -> Result<(), crate::error::AlephError> {
        let tick = self.tick_counter.fetch_add(1, Ordering::Relaxed);
        let produce_ctx = ProduceCtx {
            agent_id: DEFAULT_AGENT_ID.to_string(),
            namespace: NamespaceScope::Owner,
            tick,
        };

        let results = self.registry.dispatch_produce(&produce_ctx).await;

        for (name, res) in results {
            match res {
                Ok(raws) => {
                    for raw in raws {
                        // rust-doctor-disable-next-line excessive-clone
                        let agent_id = raw.agent_id.clone();
                        // rust-doctor-disable-next-line excessive-clone
                        let session_id = raw.session_id.clone();
                        let capture_ctx = CaptureCtx {
                            agent_id,
                            namespace: NamespaceScope::Owner,
                            session_id,
                            source_hint: raw.source.as_str().to_string(),
                        };
                        if let Err(e) = insert_with_capture_filter(
                            &self.raw_store,
                            &self.registry,
                            &capture_ctx,
                            raw,
                        )
                        .await
                        {
                            warn!("producer '{name}' produced memory failed insert: {e}");
                        }
                    }
                }
                Err(e) => {
                    debug!("producer '{name}' produce tick failed: {e}");
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::AlephError;
    use crate::memory::extensions::traits::MemoryExtension;
    use crate::memory::extensions::types::CaptureDecision;
    use crate::memory::store::raw_memory::{RawMemory, RawMemorySource};
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

    struct StubProducer(usize);
    #[async_trait]
    impl MemoryExtension for StubProducer {
        fn name(&self) -> &str {
            "test.stub_producer"
        }
        async fn produce(&self, _ctx: &ProduceCtx) -> Result<Vec<RawMemory>, AlephError> {
            Ok((0..self.0)
                .map(|i| {
                    RawMemory::new(format!("m{i}"), RawMemorySource::Transcript).with_agent("a1")
                })
                .collect())
        }
    }

    struct BlockingCapture;
    #[async_trait]
    impl MemoryExtension for BlockingCapture {
        fn name(&self) -> &str {
            "test.block_capture"
        }
        async fn on_capture(
            &self,
            _ctx: &CaptureCtx,
            _raw: &mut RawMemory,
        ) -> Result<CaptureDecision, AlephError> {
            Ok(CaptureDecision::Block {
                reason: "nope".into(),
            })
        }
    }

    #[tokio::test]
    async fn run_once_persists_produced_memories() {
        let store_inner = Arc::new(FakeStore(Mutex::new(Vec::new())));
        let store: Arc<dyn RawMemoryStore> = store_inner.clone();
        let reg = MemoryExtensionRegistry::new();
        reg.register(Arc::new(StubProducer(3)));
        let scheduler = MemoryProducerScheduler::new(Arc::new(reg), store.clone());

        scheduler.run_once().await.unwrap();

        let rows = store_inner.0.lock().unwrap();
        assert_eq!(rows.len(), 3);
        assert!(rows.iter().all(|r| r.content.starts_with("m")));
    }

    #[tokio::test]
    async fn run_once_empty_registry_persists_nothing() {
        let store_inner = Arc::new(FakeStore(Mutex::new(Vec::new())));
        let store: Arc<dyn RawMemoryStore> = store_inner.clone();
        let reg = MemoryExtensionRegistry::new();
        let scheduler = MemoryProducerScheduler::new(Arc::new(reg), store.clone());
        scheduler.run_once().await.unwrap();
        assert_eq!(store_inner.0.lock().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn blocking_capture_filters_produced_memories() {
        let store_inner = Arc::new(FakeStore(Mutex::new(Vec::new())));
        let store: Arc<dyn RawMemoryStore> = store_inner.clone();
        let reg = MemoryExtensionRegistry::new();
        reg.register(Arc::new(StubProducer(2)));
        reg.register(Arc::new(BlockingCapture));
        let scheduler = MemoryProducerScheduler::new(Arc::new(reg), store.clone());
        scheduler.run_once().await.unwrap();
        // Producer emitted 2 memories; capture chain blocks them all.
        assert_eq!(store_inner.0.lock().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn tick_counter_advances_per_run() {
        let store_inner = Arc::new(FakeStore(Mutex::new(Vec::new())));
        let store: Arc<dyn RawMemoryStore> = store_inner.clone();
        let reg = MemoryExtensionRegistry::new();
        let scheduler = MemoryProducerScheduler::new(Arc::new(reg), store.clone());
        scheduler.run_once().await.unwrap();
        scheduler.run_once().await.unwrap();
        assert_eq!(scheduler.tick_counter.load(Ordering::Relaxed), 2);
    }
}

#[cfg(test)]
mod agent_id_unification_tests {
    use super::*;

    #[test]
    fn scheduler_uses_routing_default_agent_id() {
        // Compile-time witness: scheduler imports the unified constant.
        // Value witness: the unified constant resolves to "main".
        assert_eq!(DEFAULT_AGENT_ID, crate::routing::DEFAULT_AGENT_ID);
        assert_eq!(DEFAULT_AGENT_ID, "main");
    }
}
