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
        // Atomic check-and-insert via `ArcSwap::rcu`. Returning the
        // current `Arc` from the closure is a no-op swap (compare_and_swap
        // sees pointer equality), so a concurrent register for the same name
        // either wins the CAS or rolls into the next rcu iteration with the
        // newly inserted key visible. The prior load/clone/store sequence
        // had a TOCTOU window where both callers passed `contains_key` and
        // both then overwrote each other — closing that is the point of this
        // change.
        let source = handler.definition().source.clone();
        let mut inserted = false;
        self.inner.rcu(|current| {
            // `rcu` re-runs this closure on every lost CAS, so the flag has to
            // speak for the CURRENT attempt only. Without this reset, a thread
            // that found the key absent on its first pass (setting the flag),
            // then lost the CAS and saw the key present on its second, still
            // reported success — so N concurrent registers for one name could
            // all return `Ok` and the `Duplicate` error callers rely on never
            // fired. Only the final invocation is the one whose value is
            // stored, so only its verdict may survive.
            inserted = false;
            if current.contains_key(&name) {
                return Arc::clone(current);
            }
            let mut next = (**current).clone();
            next.insert(name.clone(), handler.clone());
            inserted = true;
            Arc::new(next)
        });
        if !inserted {
            return Err(ToolError::Duplicate { name });
        }
        let _ = self
            .change_tx
            .send(RegistryChange::Registered { name, source });
        Ok(())
    }

    #[must_use]
    pub fn unregister(&self, name: &str) -> Option<Arc<dyn ToolHandler>> {
        // Atomic check-and-remove, same rcu guard as `register`. The closure
        // captures the to-be-removed handler so a successful swap hands it
        // back to the caller; a missing key returns the current `Arc` (no-op
        // swap) and leaves `removed` `None`.
        let mut removed: Option<Arc<dyn ToolHandler>> = None;
        self.inner.rcu(|current| {
            let Some(handler) = current.get(name).cloned() else {
                return Arc::clone(current);
            };
            let mut next = (**current).clone();
            next.remove(name);
            removed = Some(handler);
            Arc::new(next)
        });
        let removed = removed?;
        let source = removed.definition().source.clone();
        let _ = self.change_tx.send(RegistryChange::Unregistered {
            name: name.to_string(),
            source,
        });
        Some(removed)
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
        let _ = reg.unregister("e");
        let evt = rx.try_recv().expect("event");
        assert!(matches!(evt, RegistryChange::Unregistered { .. }));
    }

    /// Regression: concurrent `register` calls for the SAME name used to both
    /// pass the `contains_key` check (load → clone → store window), then the
    /// last `store` silently overwrote the earlier caller. After the rcu
    /// rewrite, exactly ONE caller succeeds and every other observes
    /// `ToolError::Duplicate`; the snapshot contains a single entry for
    /// the name.
    #[test]
    fn concurrent_register_for_same_name_atomic() {
        use std::sync::Arc;
        use std::thread;
        const THREADS: usize = 16;
        let reg = Arc::new(ToolHandlerRegistry::new());
        let mut handles = Vec::with_capacity(THREADS);
        for _ in 0..THREADS {
            let r = Arc::clone(&reg);
            handles.push(thread::spawn(move || r.register("race".into(), fake("race"))));
        }
        let mut wins = 0usize;
        let mut dupes = 0usize;
        for h in handles {
            match h.join().expect("join") {
                Ok(()) => wins += 1,
                Err(ToolError::Duplicate { .. }) => dupes += 1,
                Err(e) => panic!("unexpected error: {e:?}"),
            }
        }
        assert_eq!(wins, 1, "exactly one register may succeed for the same name");
        assert_eq!(dupes, THREADS - 1, "every other register must report Duplicate");
        assert_eq!(reg.snapshot().len(), 1);
    }

    /// Regression: concurrent `register` calls for DISTINCT names used to
    /// race on the load → clone → store sequence, occasionally dropping one
    /// of the entries when both cloner and inserter based on the same
    /// snapshot. The rcu loop re-loads on CAS failure, so all inserts
    /// land.
    #[test]
    fn concurrent_register_for_distinct_names_keeps_every_entry() {
        use std::sync::Arc;
        use std::thread;
        const THREADS: usize = 16;
        let reg = Arc::new(ToolHandlerRegistry::new());
        let mut handles = Vec::with_capacity(THREADS);
        for i in 0..THREADS {
            let r = Arc::clone(&reg);
            let name = format!("t{i}");
            handles.push(thread::spawn(move || r.register(name.clone(), fake(&name))));
        }
        for h in handles {
            h.join().expect("join").expect("distinct-name register must succeed");
        }
        assert_eq!(reg.snapshot().len(), THREADS);
    }
}
