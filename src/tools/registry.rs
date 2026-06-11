//! `ToolHandlerRegistry` — ArcSwap-backed name → handler map.

use crate::sync_primitives::Arc;
use std::collections::HashMap;

use arc_swap::ArcSwap;
use tokio::sync::broadcast;

use crate::tools::handlers::ToolHandler;
use crate::tools::service::{ToolError, ToolSource};

#[derive(Debug, Clone)]
pub enum RegistryChange {
    Registered { name: String, source: ToolSource },
    Unregistered { name: String, source: ToolSource },
}

pub struct ToolHandlerRegistry {
    inner: Arc<ArcSwap<HashMap<String, Arc<dyn ToolHandler>>>>,
    change_tx: broadcast::Sender<RegistryChange>,
}

impl ToolHandlerRegistry {
    #[must_use]
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(256);
        Self {
            inner: Arc::new(ArcSwap::from_pointee(HashMap::new())),
            change_tx: tx,
        }
    }

    pub fn register(&self, name: String, handler: Arc<dyn ToolHandler>) -> Result<(), ToolError> {
        let current = self.inner.load();
        if current.contains_key(&name) {
            return Err(ToolError::Duplicate { name: name.clone() });
        }
        let mut next = (**current).clone();
        let source = handler.definition().source.clone();
        next.insert(name.clone(), handler);
        self.inner.store(Arc::new(next));
        let _ = self
            .change_tx
            .send(RegistryChange::Registered { name, source });
        Ok(())
    }

    #[must_use]
    pub fn unregister(&self, name: &str) -> Option<Arc<dyn ToolHandler>> {
        let current = self.inner.load();
        let handler = current.get(name).cloned()?;
        let mut next = (**current).clone();
        let removed = next.remove(name)?;
        let source = removed.definition().source.clone();
        self.inner.store(Arc::new(next));
        let _ = self.change_tx.send(RegistryChange::Unregistered {
            name: name.to_string(),
            source,
        });
        Some(handler)
    }

    #[must_use]
    pub fn snapshot(&self) -> Arc<HashMap<String, Arc<dyn ToolHandler>>> {
        self.inner.load_full()
    }

    /// Subscribe to registry mutation events.
    ///
    /// Each receiver gets a 256-slot circular buffer (see channel allocation
    /// in `new()`). Slow consumers lose the oldest events rather than
    /// blocking publishers — this is the intended behavior for diagnostic
    /// taps and tool-catalog refresh hooks.
    ///
    /// First production consumer: the boot-time `RegistryChange` logger in
    /// `aleph-server commands::start` records every MCP server connect /
    /// disconnect for ops visibility. Treat additional consumers as additive
    /// — never block on this channel.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<RegistryChange> {
        self.change_tx.subscribe()
    }
}

impl Default for ToolHandlerRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::events::ToolOutput;
    use crate::tools::service::{ToolDefinition, ToolDefinitionMetadata, ToolSource};
    use async_trait::async_trait;
    use serde_json::Value;

    struct FakeHandler {
        name: String,
        source: ToolSource,
    }

    #[async_trait]
    impl ToolHandler for FakeHandler {
        async fn invoke(&self, _input: Value) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput {
                value: serde_json::json!({"tool": self.name}),
                metadata: Default::default(),
            })
        }
        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                name: self.name.clone(),
                description: String::new(),
                input_schema: serde_json::json!({}),
                source: self.source.clone(),
                metadata: ToolDefinitionMetadata::default(),
            }
        }
    }

    fn fake(name: &str) -> Arc<dyn ToolHandler> {
        Arc::new(FakeHandler {
            name: name.into(),
            source: ToolSource::Builtin,
        })
    }

    #[test]
    fn register_and_snapshot() {
        let reg = ToolHandlerRegistry::new();
        reg.register("a".into(), fake("a")).unwrap();
        reg.register("b".into(), fake("b")).unwrap();
        let snap = reg.snapshot();
        assert_eq!(snap.len(), 2);
        assert!(snap.contains_key("a"));
        assert!(snap.contains_key("b"));
    }

    #[test]
    fn duplicate_register_returns_other() {
        let reg = ToolHandlerRegistry::new();
        reg.register("dup".into(), fake("dup")).unwrap();
        let err = reg.register("dup".into(), fake("dup")).unwrap_err();
        assert!(matches!(err, ToolError::Duplicate { name } if name == "dup"));
    }

    #[test]
    fn unregister_removes() {
        let reg = ToolHandlerRegistry::new();
        reg.register("z".into(), fake("z")).unwrap();
        let removed = reg.unregister("z").unwrap();
        assert_eq!(removed.definition().name, "z");
        assert_eq!(reg.snapshot().len(), 0);
    }

    #[test]
    fn unregister_missing_returns_none() {
        let reg = ToolHandlerRegistry::new();
        assert!(reg.unregister("nope").is_none());
    }

    #[test]
    fn snapshot_stable_against_concurrent_register() {
        // Emit a snapshot, then register while holding the snapshot — snapshot's
        // contents must be unchanged (that's the ArcSwap guarantee).
        let reg = ToolHandlerRegistry::new();
        reg.register("x".into(), fake("x")).unwrap();
        let snap1 = reg.snapshot();
        reg.register("y".into(), fake("y")).unwrap();
        assert_eq!(snap1.len(), 1); // snap1 frozen
        assert_eq!(reg.snapshot().len(), 2); // new snapshot sees both
    }

    #[test]
    fn change_events_are_sent() {
        let reg = ToolHandlerRegistry::new();
        let mut rx = reg.subscribe();
        reg.register("e".into(), fake("e")).unwrap();
        let evt = rx.try_recv().expect("event");
        assert!(matches!(evt, RegistryChange::Registered { .. }));
        reg.unregister("e");
        let evt = rx.try_recv().expect("event");
        assert!(matches!(evt, RegistryChange::Unregistered { .. }));
    }
}
