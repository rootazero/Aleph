//! `ToolHandlerRegistry` — descriptor-backed, ArcSwap-snapshotted capability
//! registry.
//!
//! The registry is the single runtime source of truth for a Tool capability:
//! every entry pairs a `ToolHandler` with the [`ToolCapabilityDescriptor`] that
//! describes it, and both are written in one atomic `ArcSwap` swap so a
//! subscriber can never observe a half-registered tool. The model-visible
//! `ToolDefinition` is a *projection* of the descriptor, not a second source.
//!
//! Every successful mutation (register / replace / unregister) advances a
//! registry-wide monotonic `revision`; the assigned revision is stored on the
//! entry's descriptor and handed back in change events and [`RegistrationHandle`]s
//! so a stale handle can never remove a newer entry.

use crate::sync_primitives::Arc;
use std::collections::HashMap;
use std::sync::Weak;

use arc_swap::ArcSwap;
use tokio::sync::broadcast;

use crate::tools::descriptor::ToolCapabilityDescriptor;
use crate::tools::handlers::ToolHandler;
use crate::tools::service::{ToolDefinition, ToolError, ToolSource};

/// One registry value: the callable handler plus the frozen descriptor that is
/// its capability contract.
#[derive(Clone)]
pub struct RegistryEntry {
    pub handler: Arc<dyn ToolHandler>,
    pub descriptor: Arc<ToolCapabilityDescriptor>,
}

/// A registry mutation event.
///
/// Carries the `revision` assigned to the mutation, the affected `name` and the
/// tool `source`, so a subscriber can rebuild its view from the
/// [`ToolHandlerRegistry::snapshot`] / [`descriptor_snapshot`] rather than
/// trusting an incremental delta.
///
/// [`descriptor_snapshot`]: ToolHandlerRegistry::descriptor_snapshot
#[derive(Debug, Clone)]
pub enum RegistryChange {
    Registered {
        name: String,
        revision: u64,
        source: ToolSource,
    },
    Replaced {
        name: String,
        revision: u64,
        source: ToolSource,
    },
    Unregistered {
        name: String,
        revision: u64,
        source: ToolSource,
    },
}

/// Immutable registry state; swapped wholesale under `ArcSwap` so entries,
/// revision and the closed flag move together.
#[derive(Clone, Default)]
struct RegistryState {
    entries: HashMap<String, RegistryEntry>,
    revision: u64,
    closed: bool,
}

/// Shared inner owned by the registry and referenced weakly by handles, so a
/// handle never keeps the registry alive.
struct RegistryShared {
    inner: ArcSwap<RegistryState>,
    change_tx: broadcast::Sender<RegistryChange>,
}

/// An idempotent, generation-guarded registration disposer.
///
/// `dispose` removes the entry only if it is still the exact revision this
/// handle registered; once replaced or unregistered it reports `false` and
/// leaves the newer state untouched. Repeated calls after a successful dispose
/// return `false` and emit no further event.
pub struct RegistrationHandle {
    shared: Weak<RegistryShared>,
    name: String,
    revision: u64,
}

impl std::fmt::Debug for RegistrationHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegistrationHandle")
            .field("name", &self.name)
            .field("revision", &self.revision)
            .finish()
    }
}

impl RegistrationHandle {
    /// The tool name this handle registered.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The revision assigned when this handle registered.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Remove the entry iff it still matches this handle's revision.
    ///
    /// Idempotent: the first successful dispose removes the entry and advances
    /// the registry revision; later calls (or calls after a replacement) return
    /// `false` without mutating state or emitting an event.
    #[must_use]
    pub fn dispose(&self) -> bool {
        let Some(shared) = self.shared.upgrade() else {
            return false;
        };
        let mut disposed = false;
        let mut new_revision = 0u64;
        let mut source: Option<ToolSource> = None;
        shared.inner.rcu(|current| {
            let Some(entry) = current.entries.get(&self.name) else {
                return Arc::clone(current);
            };
            // Generation guard: a stale handle must not remove a replacement.
            if entry.descriptor.revision != self.revision {
                return Arc::clone(current);
            }
            let mut next = (**current).clone();
            next.entries.remove(&self.name);
            next.revision = current.revision + 1;
            new_revision = next.revision;
            source = Some(entry.descriptor.source.clone());
            disposed = true;
            Arc::new(next)
        });
        if disposed {
            if let Some(source) = source {
                let _ = shared.change_tx.send(RegistryChange::Unregistered {
                    name: self.name.clone(),
                    revision: new_revision,
                    source,
                });
            }
        }
        disposed
    }
}

pub struct ToolHandlerRegistry {
    shared: Arc<RegistryShared>,
}

impl ToolHandlerRegistry {
    #[must_use]
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(256);
        Self {
            shared: Arc::new(RegistryShared {
                inner: ArcSwap::from_pointee(RegistryState::default()),
                change_tx: tx,
            }),
        }
    }

    /// Register a new capability. Rejects a descriptor that fails validation, a
    /// descriptor that does not match its handler's own definition (ignoring
    /// the registry-assigned revision), a duplicate name, and a closed
    /// registry. On success the entry is stored under `descriptor.name` with a
    /// freshly assigned monotonic revision.
    pub fn register(
        &self,
        descriptor: ToolCapabilityDescriptor,
        handler: Arc<dyn ToolHandler>,
    ) -> Result<RegistrationHandle, ToolError> {
        let definition = handler.definition();
        let name = descriptor.name.clone();
        let mut outcome: Result<u64, ToolError> = Ok(0);
        self.shared.inner.rcu(|current| {
            if current.closed {
                outcome = Err(ToolError::RegistryClosed { name: name.clone() });
                return Arc::clone(current);
            }
            let assigned = current.revision + 1;
            let normalized = ToolCapabilityDescriptor {
                revision: assigned,
                ..descriptor.clone()
            };
            if let Err(e) = normalized.validate() {
                outcome = Err(ToolError::InvalidDescriptor {
                    name: name.clone(),
                    reason: e.to_string(),
                });
                return Arc::clone(current);
            }
            if !normalized.matches_definition(&definition) {
                outcome = Err(ToolError::DescriptorMismatch {
                    name: name.clone(),
                    reason: mismatch_reason(&normalized, &definition),
                });
                return Arc::clone(current);
            }
            if current.entries.contains_key(&name) {
                outcome = Err(ToolError::Duplicate { name: name.clone() });
                return Arc::clone(current);
            }
            let mut next = (**current).clone();
            next.entries.insert(
                name.clone(),
                RegistryEntry {
                    handler: Arc::clone(&handler),
                    descriptor: Arc::new(normalized),
                },
            );
            next.revision = assigned;
            outcome = Ok(assigned);
            Arc::new(next)
        });
        let revision = outcome?;
        let _ = self.shared.change_tx.send(RegistryChange::Registered {
            name: name.clone(),
            revision,
            source: descriptor.source,
        });
        Ok(RegistrationHandle {
            shared: Arc::downgrade(&self.shared),
            name,
            revision,
        })
    }

    /// Replace an existing capability's handler and descriptor in one atomic
    /// swap. Callers already holding the previous handler keep their stable
    /// `Arc`; subsequent [`resolve`](Self::resolve) sees the new one. Returns
    /// `ToolError::NotFound` when the name was never registered.
    pub fn replace(
        &self,
        descriptor: ToolCapabilityDescriptor,
        handler: Arc<dyn ToolHandler>,
    ) -> Result<RegistrationHandle, ToolError> {
        let definition = handler.definition();
        let name = descriptor.name.clone();
        let mut outcome: Result<u64, ToolError> = Ok(0);
        self.shared.inner.rcu(|current| {
            if current.closed {
                outcome = Err(ToolError::RegistryClosed { name: name.clone() });
                return Arc::clone(current);
            }
            if !current.entries.contains_key(&name) {
                outcome = Err(ToolError::NotFound { name: name.clone() });
                return Arc::clone(current);
            }
            let assigned = current.revision + 1;
            let normalized = ToolCapabilityDescriptor {
                revision: assigned,
                ..descriptor.clone()
            };
            if let Err(e) = normalized.validate() {
                outcome = Err(ToolError::InvalidDescriptor {
                    name: name.clone(),
                    reason: e.to_string(),
                });
                return Arc::clone(current);
            }
            if !normalized.matches_definition(&definition) {
                outcome = Err(ToolError::DescriptorMismatch {
                    name: name.clone(),
                    reason: mismatch_reason(&normalized, &definition),
                });
                return Arc::clone(current);
            }
            let mut next = (**current).clone();
            next.entries.insert(
                name.clone(),
                RegistryEntry {
                    handler: Arc::clone(&handler),
                    descriptor: Arc::new(normalized),
                },
            );
            next.revision = assigned;
            outcome = Ok(assigned);
            Arc::new(next)
        });
        let revision = outcome?;
        let _ = self.shared.change_tx.send(RegistryChange::Replaced {
            name: name.clone(),
            revision,
            source: descriptor.source,
        });
        Ok(RegistrationHandle {
            shared: Arc::downgrade(&self.shared),
            name,
            revision,
        })
    }

    /// Resolve the live handler for `name`.
    #[must_use]
    pub fn resolve(&self, name: &str) -> Option<Arc<dyn ToolHandler>> {
        self.shared
            .inner
            .load()
            .entries
            .get(name)
            .map(|entry| Arc::clone(&entry.handler))
    }

    /// Resolve the live descriptor for `name`.
    #[must_use]
    pub fn descriptor(&self, name: &str) -> Option<Arc<ToolCapabilityDescriptor>> {
        self.shared
            .inner
            .load()
            .entries
            .get(name)
            .map(|entry| Arc::clone(&entry.descriptor))
    }

    /// Remove a capability by name, returning the removed handler.
    ///
    /// Unlike [`RegistrationHandle::dispose`] this is unguarded; it is the
    /// teardown path (`unregister_mcp_tools`) that removes by an externally
    /// derived name set.
    #[must_use]
    pub fn unregister(&self, name: &str) -> Option<Arc<dyn ToolHandler>> {
        let mut removed: Option<Arc<dyn ToolHandler>> = None;
        let mut new_revision = 0u64;
        let mut source: Option<ToolSource> = None;
        self.shared.inner.rcu(|current| {
            let Some(entry) = current.entries.get(name) else {
                return Arc::clone(current);
            };
            let mut next = (**current).clone();
            next.entries.remove(name);
            next.revision = current.revision + 1;
            removed = Some(Arc::clone(&entry.handler));
            new_revision = next.revision;
            source = Some(entry.descriptor.source.clone());
            Arc::new(next)
        });
        let removed = removed?;
        if let Some(source) = source {
            let _ = self.shared.change_tx.send(RegistryChange::Unregistered {
                name: name.to_string(),
                revision: new_revision,
                source,
            });
        }
        Some(removed)
    }

    /// Frozen handler view (compatibility projection).
    #[must_use]
    pub fn snapshot(&self) -> Arc<HashMap<String, Arc<dyn ToolHandler>>> {
        let state = self.shared.inner.load();
        Arc::new(
            state
                .entries
                .iter()
                .map(|(name, entry)| (name.clone(), Arc::clone(&entry.handler)))
                .collect(),
        )
    }

    /// Frozen descriptor view — the canonical capability snapshot.
    #[must_use]
    pub fn descriptor_snapshot(&self) -> Arc<HashMap<String, Arc<ToolCapabilityDescriptor>>> {
        let state = self.shared.inner.load();
        Arc::new(
            state
                .entries
                .iter()
                .map(|(name, entry)| (name.clone(), Arc::clone(&entry.descriptor)))
                .collect(),
        )
    }

    /// The current monotonic revision. Advances by one on every successful
    /// register / replace / unregister.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.shared.inner.load().revision
    }

    /// Close the registry: stop accepting registrations and replacements.
    /// Existing resolution and disposal continue to work. Idempotent.
    pub fn close(&self) {
        self.shared.inner.rcu(|current| {
            if current.closed {
                return Arc::clone(current);
            }
            let mut next = (**current).clone();
            next.closed = true;
            Arc::new(next)
        });
    }

    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.shared.inner.load().closed
    }

    /// Subscribe to registry mutation events.
    ///
    /// Each receiver gets a 256-slot circular buffer (see channel allocation
    /// in `new()`). Slow consumers lose the oldest events rather than
    /// blocking publishers — this is the intended behavior for diagnostic
    /// taps and tool-catalog refresh hooks. A lagged subscriber must rebuild
    /// from [`snapshot`](Self::snapshot) rather than treat the gap as "no
    /// change".
    ///
    /// First production consumer: the boot-time `RegistryChange` logger in
    /// `aleph-server commands::start` records every MCP server connect /
    /// disconnect for ops visibility. Treat additional consumers as additive
    /// — never block on this channel.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<RegistryChange> {
        self.shared.change_tx.subscribe()
    }
}

impl Default for ToolHandlerRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Which descriptor fields disagree with the handler's own definition, for the
/// `DescriptorMismatch` diagnostic. The revision is deliberately excluded —
/// the registry owns it.
fn mismatch_reason(descriptor: &ToolCapabilityDescriptor, definition: &ToolDefinition) -> String {
    let mut fields: Vec<&str> = Vec::new();
    if descriptor.name != definition.name {
        fields.push("name");
    }
    if descriptor.source != definition.source {
        fields.push("source");
    }
    if descriptor.description != definition.description {
        fields.push("description");
    }
    if descriptor.input_schema != definition.input_schema {
        fields.push("input_schema");
    }
    if descriptor.requires_confirmation != definition.metadata.requires_approval {
        fields.push("requires_confirmation");
    }
    if descriptor.idempotent != definition.metadata.idempotent {
        fields.push("idempotent");
    }
    if descriptor.concurrent_safe != definition.metadata.concurrent_safe {
        fields.push("concurrent_safe");
    }
    if descriptor.max_duration_ms != definition.metadata.max_duration_ms {
        fields.push("max_duration_ms");
    }
    if fields.is_empty() {
        "descriptor and handler definition disagree".to_string()
    } else {
        format!("fields differ: {}", fields.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::events::ToolOutput;
    use crate::tools::descriptor::{ReplayPolicy, ToolKind, SCHEMA_VERSION};
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

    /// A descriptor whose every projected field matches [`fake`]. `revision` is
    /// left 0 on purpose: the registry assigns it.
    fn desc(name: &str) -> ToolCapabilityDescriptor {
        ToolCapabilityDescriptor {
            name: name.into(),
            kind: ToolKind::Tool,
            schema_version: SCHEMA_VERSION,
            description: String::new(),
            input_schema: serde_json::json!({}),
            source: ToolSource::Builtin,
            replay_policy: ReplayPolicy::Unsafe,
            requires_confirmation: false,
            idempotent: false,
            concurrent_safe: false,
            max_duration_ms: None,
            revision: 0,
        }
    }

    #[test]
    fn register_and_snapshot() {
        let reg = ToolHandlerRegistry::new();
        reg.register(desc("a"), fake("a")).unwrap();
        reg.register(desc("b"), fake("b")).unwrap();
        let snap = reg.snapshot();
        assert_eq!(snap.len(), 2);
        assert!(snap.contains_key("a"));
        assert!(snap.contains_key("b"));
        let descriptors = reg.descriptor_snapshot();
        assert_eq!(descriptors.len(), 2);
        assert!(descriptors.contains_key("a"));
        assert!(descriptors.contains_key("b"));
    }

    #[test]
    fn duplicate_register_returns_other() {
        let reg = ToolHandlerRegistry::new();
        reg.register(desc("dup"), fake("dup")).unwrap();
        let err = reg.register(desc("dup"), fake("dup")).unwrap_err();
        assert!(matches!(err, ToolError::Duplicate { name } if name == "dup"));
    }

    #[test]
    fn unregister_removes() {
        let reg = ToolHandlerRegistry::new();
        reg.register(desc("z"), fake("z")).unwrap();
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
        // contents must be unchanged (frozen view).
        let reg = ToolHandlerRegistry::new();
        reg.register(desc("x"), fake("x")).unwrap();
        let snap1 = reg.snapshot();
        reg.register(desc("y"), fake("y")).unwrap();
        assert_eq!(snap1.len(), 1); // snap1 frozen
        assert_eq!(reg.snapshot().len(), 2); // new snapshot sees both
    }

    #[test]
    fn change_events_are_sent() {
        let reg = ToolHandlerRegistry::new();
        let mut rx = reg.subscribe();
        reg.register(desc("e"), fake("e")).unwrap();
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
            handles.push(thread::spawn(move || r.register(desc("race"), fake("race"))));
        }
        let mut wins = 0usize;
        let mut dupes = 0usize;
        for h in handles {
            match h.join().expect("join") {
                Ok(_handle) => wins += 1,
                Err(ToolError::Duplicate { .. }) => dupes += 1,
                Err(e) => panic!("unexpected error: {e:?}"),
            }
        }
        assert_eq!(
            wins, 1,
            "exactly one register may succeed for the same name"
        );
        assert_eq!(
            dupes,
            THREADS - 1,
            "every other register must report Duplicate"
        );
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
            handles.push(thread::spawn(move || r.register(desc(&name), fake(&name))));
        }
        for h in handles {
            h.join()
                .expect("join")
                .expect("distinct-name register must succeed");
        }
        assert_eq!(reg.snapshot().len(), THREADS);
    }

    // ------------------------------------------------------------------
    // Task 2 brief tests
    // ------------------------------------------------------------------

    #[test]
    fn registration_assigns_monotonic_revision_and_emits_it() {
        let reg = ToolHandlerRegistry::new();
        let mut rx = reg.subscribe();

        let h1 = reg.register(desc("a"), fake("a")).unwrap();
        let h2 = reg.register(desc("b"), fake("b")).unwrap();

        assert_eq!(h1.revision(), 1, "first registration gets revision 1");
        assert_eq!(h2.revision(), 2, "second registration gets revision 2");
        assert_eq!(reg.revision(), 2);
        assert_eq!(reg.descriptor("a").unwrap().revision, 1);
        assert_eq!(reg.descriptor("b").unwrap().revision, 2);

        let e1 = rx.try_recv().expect("first event");
        assert!(matches!(
            e1,
            RegistryChange::Registered { revision: 1, ref name, .. } if name == "a"
        ));
        let e2 = rx.try_recv().expect("second event");
        assert!(matches!(
            e2,
            RegistryChange::Registered { revision: 2, ref name, .. } if name == "b"
        ));
    }

    #[test]
    fn replacement_keeps_old_handler_alive_and_new_resolve_uses_new_handler() {
        let reg = ToolHandlerRegistry::new();
        let mut rx = reg.subscribe();

        let first = reg.register(desc("t"), fake("t")).unwrap();
        let old = reg.resolve("t").expect("old handler");
        assert_eq!(first.revision(), 1);

        let second = reg.replace(desc("t"), fake("t")).unwrap();
        assert_eq!(second.revision(), 2);

        let new = reg.resolve("t").expect("new handler");
        assert!(
            !Arc::ptr_eq(&old, &new),
            "resolve must return the replacement handler"
        );
        // The old handler is still alive and fully usable for in-flight calls.
        assert_eq!(old.definition().name, "t");
        assert_eq!(reg.descriptor("t").unwrap().revision, 2);
        assert_eq!(reg.snapshot().len(), 1);

        let evt = rx.try_recv().expect("register event");
        assert!(matches!(evt, RegistryChange::Registered { revision: 1, .. }));
        let evt = rx.try_recv().expect("replace event");
        assert!(matches!(
            evt,
            RegistryChange::Replaced { revision: 2, ref name, .. } if name == "t"
        ));
    }

    #[test]
    fn stale_registration_handle_cannot_remove_replacement() {
        let reg = ToolHandlerRegistry::new();

        let stale = reg.register(desc("t"), fake("t")).unwrap(); // revision 1
        let _current = reg.replace(desc("t"), fake("t")).unwrap(); // revision 2

        assert!(
            !stale.dispose(),
            "a handle for a superseded revision must not dispose the replacement"
        );
        assert!(reg.resolve("t").is_some(), "replacement must survive");
        assert_eq!(reg.snapshot().len(), 1);
    }

    #[test]
    fn close_rejects_register_and_dispose_is_idempotent() {
        let reg = ToolHandlerRegistry::new();
        let handle = reg.register(desc("t"), fake("t")).unwrap();
        assert!(!reg.is_closed());

        reg.close();
        reg.close(); // idempotent
        assert!(reg.is_closed());

        let err = reg.register(desc("u"), fake("u")).unwrap_err();
        assert!(matches!(err, ToolError::RegistryClosed { ref name } if name == "u"));

        // Existing handles stay disposable after close.
        assert!(handle.dispose(), "first dispose removes");
        assert_eq!(reg.snapshot().len(), 0);
        assert!(!handle.dispose(), "second dispose is a no-op");
        assert!(!handle.dispose(), "third dispose is a no-op");
    }

    #[test]
    fn descriptor_handler_mismatch_is_rejected() {
        let reg = ToolHandlerRegistry::new();
        let mut mismatched = desc("t");
        // Handler advertises an empty schema; descriptor claims properties.
        mismatched.input_schema = serde_json::json!({
            "type": "object",
            "properties": { "x": { "type": "string" } }
        });

        let before = reg.revision();
        let err = reg.register(mismatched, fake("t")).unwrap_err();
        assert!(matches!(err, ToolError::DescriptorMismatch { ref name, .. } if name == "t"));

        // Fail-closed: no state mutation and no revision advance.
        assert_eq!(reg.snapshot().len(), 0);
        assert_eq!(reg.revision(), before);
    }
}
