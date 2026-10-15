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
use tokio::sync::{broadcast, Notify};

use crate::session::events::ToolOutput;
use crate::tools::descriptor::{
    ReplayPolicyLookup, ToolCallIdentity, ToolCapabilityDescriptor, ToolDescriptorLookup,
};
use crate::tools::handlers::{McpServerFilter, ToolHandler};
use crate::tools::service::{ToolDefinition, ToolError, ToolSource};
use serde_json::Value;

/// One registry value: the callable handler plus the frozen descriptor that is
/// its capability contract.
#[derive(Clone)]
pub struct RegistryEntry {
    pub handler: Arc<dyn ToolHandler>,
    pub descriptor: Arc<ToolCapabilityDescriptor>,
}

/// Registry-owned invocation boundary. The raw handler is never returned by
/// any canonical resolver, snapshot, unregister, or visibility binding.
struct AdmissionHandler {
    inner: Arc<dyn ToolHandler>,
    descriptor: Arc<ToolCapabilityDescriptor>,
}

#[async_trait::async_trait]
impl ToolHandler for AdmissionHandler {
    async fn invoke(&self, input: Value) -> Result<ToolOutput, ToolError> {
        let identity = ToolCallIdentity::from_descriptor(&self.descriptor);
        let actor = crate::identity::current_actor();
        let admitted = crate::approval::current_tool_call_id().is_some_and(|call_id| {
            crate::tools::dispatch_verdict::invocation_admitted(
                &call_id,
                &self.descriptor.name,
                &identity,
                &input,
                actor.as_deref(),
            )
        });
        if !admitted {
            return Err(ToolError::PermissionDenied {
                name: self.descriptor.name.clone(),
                reason:
                    "canonical invocation requires exact dispatch or sealed Safe replay admission"
                        .into(),
            });
        }
        self.inner.invoke(input).await
    }

    fn definition(&self) -> ToolDefinition {
        self.inner.definition()
    }
    fn concurrency_claim(&self, input: &Value) -> crate::tools::concurrency::ConcurrencyClaim {
        self.inner.concurrency_claim(input)
    }
    fn fences_output(&self) -> bool {
        self.inner.fences_output()
    }

    fn bind_visible_servers(&self, visible: &McpServerFilter) -> Option<Arc<dyn ToolHandler>> {
        self.inner.bind_visible_servers(visible).map(|inner| {
            Arc::new(Self {
                inner,
                descriptor: Arc::clone(&self.descriptor),
            }) as Arc<dyn ToolHandler>
        })
    }
}

impl RegistryEntry {
    fn admitted(handler: Arc<dyn ToolHandler>, descriptor: ToolCapabilityDescriptor) -> Self {
        let descriptor = Arc::new(descriptor);
        Self {
            handler: Arc::new(AdmissionHandler {
                inner: handler,
                descriptor: Arc::clone(&descriptor),
            }),
            descriptor,
        }
    }
}

/// One immutable registry generation. Entries, the mutation revision and the
/// closed flag are captured from the same `ArcSwap` state, so a recovery
/// adapter cannot pair a handler from one generation with a descriptor from
/// another.
#[derive(Clone)]
pub struct RegistrySnapshot {
    entries: Arc<HashMap<String, RegistryEntry>>,
    revision: u64,
    closed: bool,
}

impl RegistrySnapshot {
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    #[must_use]
    pub fn entry(&self, name: &str) -> Option<&RegistryEntry> {
        self.entries.get(name)
    }

    #[must_use]
    pub fn entries(&self) -> &HashMap<String, RegistryEntry> {
        &self.entries
    }
}

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
    /// Serializes state publication with its change-event broadcast.
    ///
    /// The `ArcSwap` swap and the corresponding `change_tx.send` are two
    /// separate steps: without this lock, two concurrent mutations could
    /// interleave as `swap(rev=2); swap(rev=1); send(rev=1); send(rev=2)`,
    /// publishing change events out of revision order. Holding this lock
    /// across both steps guarantees subscribers observe events in the exact
    /// order revisions were assigned.
    ///
    /// A plain `std::sync::Mutex` is sufficient — `broadcast::Sender::send`
    /// is synchronous and never blocks, so no async mutex is needed. Poisoned
    /// locks are recovered via `PoisonError::into_inner`: the guarded data is
    /// only the unit token, and the real state lives behind `ArcSwap`, so
    /// resuming after a panic in a mutator is safe.
    mutation_lock: std::sync::Mutex<()>,
    /// Notification-only close waiter: carries no payload and no revision.
    ///
    /// `close()` publishes the `closed` bit under `mutation_lock` and THEN
    /// calls `notify_waiters()`, so a waiter that observes the wake also
    /// observes `is_closed() == true`. Observers MUST register a `notified()`
    /// future BEFORE reading the `closed` bit (register-then-check):
    ///
    /// * close-before-check — the bit is already set, read it directly;
    /// * check-then-close-before-wait — a `Notified` future captures the
    ///   notify generation AT CREATION and observes later changes when polled,
    ///   so the already-registered future resolves instead of sleeping forever;
    /// * multi-waiter — `notify_waiters` (not `notify_one`) wakes every
    ///   registered observer, none is starved.
    close_signal: Arc<Notify>,
}

impl RegistryShared {
    /// Acquire the mutation lock, recovering from a poisoned lock rather than
    /// unwrapping (production code must not panic on a downstream mutation
    /// panic).
    fn lock_mutations(&self) -> std::sync::MutexGuard<'_, ()> {
        self.mutation_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// An idempotent, generation-guarded registration disposer.
///
/// `dispose` removes the entry only if it is still the exact revision this
/// handle registered; once replaced or unregistered it reports `false` and
/// leaves the newer state untouched. Repeated calls after a successful dispose
/// return `false` and emit no further event.
// `Clone` is deliberate: a clone still refers to the same `(name, revision)`
// generation and `dispose` is idempotent, so multiple clones (e.g. one held
// by a `ToolRegistrationScope` for teardown, one held by an owner's map for
// replacement bookkeeping) can never double-dispose a newer registration.
#[derive(Clone)]
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
        // Hold the mutation lock across state swap + event send so this
        // dispose's Unregistered event cannot be published out of order
        // relative to a concurrent mutation's event.
        let _guard = shared.lock_mutations();
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

#[derive(Clone)]
pub struct ToolHandlerRegistry {
    shared: Arc<RegistryShared>,
}

impl ReplayPolicyLookup for ToolHandlerRegistry {
    fn replay_policy(&self, name: &str) -> Option<crate::tools::descriptor::ReplayPolicy> {
        self.descriptor(name)
            .map(|descriptor| descriptor.replay_policy)
    }
}

impl ToolDescriptorLookup for ToolHandlerRegistry {
    /// Copy the durable identity fields from the current descriptor for `name`.
    ///
    /// This identity is a snapshot of one descriptor, not a guarantee that a
    /// separate handler lookup observes the same generation. Callers needing
    /// both must capture [`Self::resolve_entry`] once and derive the identity
    /// from that entry's descriptor. Returns `None` for an unknown name — a
    /// missing descriptor is unknown, never allow-all. This snapshot read
    /// never authorizes replay or dispatch.
    fn tool_call_identity(&self, name: &str) -> Option<ToolCallIdentity> {
        self.descriptor(name)
            .map(|descriptor| ToolCallIdentity::from_descriptor(&descriptor))
    }
}

impl ToolHandlerRegistry {
    #[must_use]
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(256);
        Self {
            shared: Arc::new(RegistryShared {
                inner: ArcSwap::from_pointee(RegistryState::default()),
                change_tx: tx,
                mutation_lock: std::sync::Mutex::new(()),
                close_signal: Arc::new(Notify::new()),
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
        // Serialize the whole mutation so the revision assigned below and the
        // change event carrying it are published atomically in order.
        let _guard = self.shared.lock_mutations();
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
                RegistryEntry::admitted(Arc::clone(&handler), normalized),
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
        let _guard = self.shared.lock_mutations();
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
                RegistryEntry::admitted(Arc::clone(&handler), normalized),
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

    /// Capture the handler and descriptor for `name` from one atomic generation.
    ///
    /// The owned entry remains paired and usable after replacement or removal.
    /// This is an exact canonical-name lookup, not dispatch authorization.
    #[must_use]
    pub fn resolve_entry(&self, name: &str) -> Option<RegistryEntry> {
        let state = self.shared.inner.load();
        state.entries.get(name).cloned()
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
        let _guard = self.shared.lock_mutations();
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

    /// Frozen handler+descriptor+registry-state view from one generation.
    /// Recovery adapters must use this instead of pairing independent reads.
    #[must_use]
    pub fn snapshot_state(&self) -> RegistrySnapshot {
        let state = self.shared.inner.load();
        RegistrySnapshot {
            entries: Arc::new(state.entries.clone()),
            revision: state.revision,
            closed: state.closed,
        }
    }

    /// Frozen handler+descriptor view from one registry generation. Consumers
    /// that project callable tools should prefer this over pairing
    /// `snapshot()` and `descriptor_snapshot()` independently.
    #[must_use]
    pub fn entries_snapshot(&self) -> Arc<HashMap<String, RegistryEntry>> {
        Arc::clone(&self.snapshot_state().entries)
    }

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
        // Serialize with other mutations so a concurrent register/replace
        // either fully precedes or fully follows the close, rather than
        // racing its state swap against the closed flag.
        let _guard = self.shared.lock_mutations();
        self.shared.inner.rcu(|current| {
            if current.closed {
                return Arc::clone(current);
            }
            let mut next = (**current).clone();
            next.closed = true;
            Arc::new(next)
        });
        // Publish the closed bit first (the rcu swap above), then wake every
        // registered close waiter. `notify_waiters` is used so multiple
        // concurrent observers are all released; a `Notified` future captures
        // the notify generation AT CREATION and observes later changes when
        // polled, so an observer that checked `is_closed()` before the swap
        // and is about to await its already-registered future still wakes.
        self.shared.close_signal.notify_waiters();
    }

    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.shared.inner.load().closed
    }

    /// A shared, notification-only close signal for observers that must
    /// distinguish "closed" from a lagged or silently-dropped change stream.
    ///
    /// Callers register `Notify::notified()` BEFORE reading `is_closed()` (or
    /// `snapshot_state().is_closed()`) and await it only when the bit was not
    /// yet set, closing the close-before-wait race. No `RegistryChange::Closed`
    /// event and no revision/cursor change is introduced.
    #[must_use]
    pub(crate) fn close_signal(&self) -> Arc<Notify> {
        Arc::clone(&self.shared.close_signal)
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
        idempotent: bool,
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
                metadata: ToolDefinitionMetadata {
                    idempotent: self.idempotent,
                    ..Default::default()
                },
            }
        }
    }

    fn fake(name: &str) -> Arc<dyn ToolHandler> {
        Arc::new(FakeHandler {
            name: name.into(),
            source: ToolSource::Builtin,
            idempotent: false,
        })
    }

    fn fake_idempotent(name: &str) -> Arc<dyn ToolHandler> {
        Arc::new(FakeHandler {
            name: name.into(),
            source: ToolSource::Builtin,
            idempotent: true,
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
            implementation_contract: None,
        }
    }

    // This raw fixture deliberately knows nothing about admission proof.
    // Only the canonical registry entry may stop its observable effect.
    struct AdmissionCountingHandler {
        name: String,
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait]
    impl ToolHandler for AdmissionCountingHandler {
        async fn invoke(&self, _input: Value) -> Result<ToolOutput, ToolError> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(ToolOutput {
                value: serde_json::json!({"executed": true}),
                metadata: Default::default(),
            })
        }

        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                name: self.name.clone(),
                description: "Observable raw admission fixture".into(),
                input_schema: serde_json::json!({"type": "object"}),
                source: ToolSource::Builtin,
                metadata: ToolDefinitionMetadata::default(),
            }
        }
    }

    #[tokio::test]
    async fn canonical_unsafe_entry_invoke_without_proof_denies_before_inner_effect() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let reg = ToolHandlerRegistry::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let raw: Arc<dyn ToolHandler> = Arc::new(AdmissionCountingHandler {
            name: "a3_raw_unsafe".into(),
            calls: Arc::clone(&calls),
        });
        let descriptor = ToolCapabilityDescriptor::from_definition(&raw.definition(), 0);
        assert!(descriptor.matches_definition(&raw.definition()));
        assert_eq!(descriptor.replay_policy, ReplayPolicy::Unsafe);
        reg.register(descriptor, raw).expect("valid Unsafe fixture");
        let entry = reg.resolve_entry("a3_raw_unsafe").expect("canonical entry");
        entry
            .descriptor
            .validate()
            .expect("valid registered descriptor");

        // No dispatcher, Scoped wrapper, admission scope, or replay token.
        let result = entry.handler.invoke(serde_json::json!({})).await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "unproved raw call reached the inner handler"
        );
        assert!(
            matches!(result, Err(ToolError::PermissionDenied { ref name, .. }) if name == "a3_raw_unsafe"),
            "canonical Unsafe raw invocation must return PermissionDenied without proof"
        );
    }

    #[tokio::test]
    async fn canonical_safe_entry_invoke_without_proof_denies_before_inner_effect() {
        use crate::tools::descriptor::ImplementationContract;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let reg = ToolHandlerRegistry::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let raw: Arc<dyn ToolHandler> = Arc::new(AdmissionCountingHandler {
            name: "a3_raw_safe".into(),
            calls: Arc::clone(&calls),
        });
        let mut descriptor = ToolCapabilityDescriptor::from_definition(&raw.definition(), 0);
        descriptor.replay_policy = ReplayPolicy::Safe;
        descriptor.implementation_contract = Some(ImplementationContract {
            id: "test:a3-raw-safe".into(),
            version: "1".into(),
        });
        assert!(descriptor.matches_definition(&raw.definition()));
        reg.register(descriptor, raw)
            .expect("valid Safe fixture with audited contract");
        let entry = reg.resolve_entry("a3_raw_safe").expect("canonical entry");
        entry
            .descriptor
            .validate()
            .expect("valid registered descriptor");
        assert_eq!(entry.descriptor.replay_policy, ReplayPolicy::Safe);
        assert!(entry.descriptor.replay_contract_fingerprint().is_some());

        // Safe replay metadata is not dispatch or replay admission proof.
        let result = entry.handler.invoke(serde_json::json!({})).await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "unproved Safe raw call reached the inner handler"
        );
        assert!(
            matches!(result, Err(ToolError::PermissionDenied { ref name, .. }) if name == "a3_raw_safe"),
            "canonical Safe raw invocation must return PermissionDenied without proof"
        );
    }

    // A3 tests-first: the fixed API is
    // resolve_entry(&self, name: &str) -> Option<RegistryEntry>.
    // No fixture emulates it with separate handler/descriptor lookups.
    struct RevisionHandler {
        generation: u64,
    }

    #[async_trait]
    impl ToolHandler for RevisionHandler {
        fn fences_output(&self) -> bool {
            false
        }

        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                name: "terminal".into(),
                description: format!("terminal implementation {}", self.generation),
                input_schema: serde_json::json!({"type": "object"}),
                source: ToolSource::Builtin,
                metadata: ToolDefinitionMetadata::default(),
            }
        }

        async fn invoke(&self, _input: Value) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput {
                value: serde_json::json!({"generation": self.generation}),
                metadata: Default::default(),
            })
        }
    }

    // Invoke a frozen fixture entry through the real gates, never by minting
    // proof in a test or exposing the registry-owned wrapper's raw handler.
    async fn invoke_captured(entry: &RegistryEntry) -> ToolOutput {
        use crate::tools::service::ToolService;
        let mut projection = crate::tools::runtime::LoopToolRegistry::new();
        // Frozen fixture store containing the actual captured, already wrapped
        // pair. Re-registering would assign a new identity and double-wrap it.
        let frozen = Arc::new(ToolHandlerRegistry::new());
        frozen.shared.inner.store(Arc::new(RegistryState {
            entries: [(entry.descriptor.name.clone(), entry.clone())].into(),
            revision: entry.descriptor.revision,
            closed: false,
        }));
        projection.bind_canonical_registry(frozen, Arc::new(|_| true));
        projection.register(Box::new(
            crate::tools::adapters::McpRegistryTool::from_registry_entry(
                entry.handler.clone(),
                &entry.descriptor,
            ),
        ));
        let service =
            crate::tools::scoped::ScopedToolService::new(Arc::new(projection), Default::default());
        let mut output = crate::approval::with_call_identity(
            Some(crate::approval::CallIdentity {
                turn_id: crate::session::events::TurnId::nil(),
                call_id: "captured-fixture".into(),
            }),
            service.execute(&entry.descriptor.name, serde_json::json!({})),
        )
        .await
        .expect("frozen fixture entry passes real gates");
        output.value =
            serde_json::from_str(output.value.as_str().expect("Layer 2 rendered JSON")).unwrap();
        output
    }

    // Break caught: a captured entry pairs rev1 authority with rev2 code, or
    // becomes a live name lookup instead of retaining its owned generation.
    #[tokio::test]
    async fn resolve_entry_keeps_handler_and_identity_from_one_generation() {
        use crate::tools::descriptor::{ImplementationContract, ToolCallIdentity};
        use tokio::sync::Barrier;

        let reg = ToolHandlerRegistry::new();
        let first_handler: Arc<dyn ToolHandler> = Arc::new(RevisionHandler { generation: 1 });
        reg.register(
            ToolCapabilityDescriptor::from_definition(&first_handler.definition(), 0),
            Arc::clone(&first_handler),
        )
        .unwrap();
        let first_published = reg.resolve("terminal").unwrap();
        assert!(
            !Arc::ptr_eq(&first_published, &first_handler),
            "canonical publication owns the admission wrapper"
        );
        let next_handler: Arc<dyn ToolHandler> = Arc::new(RevisionHandler { generation: 2 });
        let mut next_descriptor =
            ToolCapabilityDescriptor::from_definition(&next_handler.definition(), 0);
        next_descriptor.replay_policy = ReplayPolicy::Safe;
        next_descriptor.implementation_contract = Some(ImplementationContract {
            id: "test:terminal-generation".into(),
            version: "2".into(),
        });
        let arrived = Barrier::new(2);
        let release = Barrier::new(2);
        let holder = async {
            let captured: RegistryEntry = reg.resolve_entry("terminal").expect("rev1 entry");
            arrived.wait().await;
            release.wait().await;
            let identity = ToolCallIdentity::from_descriptor(&captured.descriptor);
            assert_eq!(identity.revision, 1);
            assert_eq!(identity.replay_policy, ReplayPolicy::Unsafe);
            assert_eq!(identity.replay_contract_fingerprint, None);
            assert_eq!(captured.descriptor.description, "terminal implementation 1");
            assert!(Arc::ptr_eq(&captured.handler, &first_published));
            let output = invoke_captured(&captured).await;
            assert_eq!(output.value, serde_json::json!({"generation": 1}));
            captured
        };
        let replacer = async {
            arrived.wait().await;
            reg.replace(next_descriptor, Arc::clone(&next_handler))
                .unwrap();
            release.wait().await;
        };
        let (captured, ()) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            tokio::join!(holder, replacer)
        })
        .await
        .expect("capture must precede replacement and retained invocation must finish");
        let current = reg.resolve_entry("terminal").expect("rev2 entry");
        let identity = ToolCallIdentity::from_descriptor(&current.descriptor);
        assert_eq!(identity.revision, 2);
        assert_eq!(identity.replay_policy, ReplayPolicy::Safe);
        assert!(identity.replay_contract_fingerprint.is_some());
        assert_eq!(current.descriptor.description, "terminal implementation 2");
        assert!(Arc::ptr_eq(
            &current.handler,
            &reg.resolve("terminal").unwrap()
        ));
        assert!(
            !Arc::ptr_eq(&current.handler, &next_handler),
            "replacement also owns an admission wrapper"
        );
        assert!(!Arc::ptr_eq(&current.handler, &captured.handler));
        assert_eq!(
            invoke_captured(&current).await.value,
            serde_json::json!({"generation": 2})
        );

        reg.unregister("terminal").unwrap();
        assert!(reg.resolve_entry("terminal").is_none());
        assert_eq!(captured.descriptor.revision, 1);
        assert_eq!(
            invoke_captured(&captured).await.value,
            serde_json::json!({"generation": 1}),
            "unregister must not revoke an already captured owned entry"
        );
    }

    // Break caught: resolve_entry fabricates an entry on miss or falls back
    // to a stale compatibility projection after canonical removal.
    #[test]
    fn resolve_entry_returns_none_for_missing_and_unregistered_name() {
        let resolve: fn(&ToolHandlerRegistry, &str) -> Option<RegistryEntry> =
            ToolHandlerRegistry::resolve_entry;
        let reg = ToolHandlerRegistry::new();
        assert!(resolve(&reg, "terminal").is_none());
        reg.register(desc("terminal"), fake("terminal")).unwrap();
        assert!(resolve(&reg, "terminal").is_some());
        assert!(resolve(&reg, "missing").is_none());
        reg.unregister("terminal").unwrap();
        assert!(resolve(&reg, "terminal").is_none());
    }

    #[test]
    fn replay_lookup_is_descriptor_based_not_tool_name_special_case() {
        let reg = ToolHandlerRegistry::new();
        reg.register(desc("arbitrary_name"), fake("arbitrary_name"))
            .expect("register");
        assert_eq!(
            ReplayPolicyLookup::replay_policy(&reg, "arbitrary_name"),
            Some(ReplayPolicy::Unsafe)
        );
        assert_eq!(ReplayPolicyLookup::replay_policy(&reg, "missing"), None);
    }

    #[test]
    fn tool_call_identity_reflects_current_descriptor() {
        let reg = ToolHandlerRegistry::new();
        let mut d = desc("arbitrary_name");
        d.replay_policy = ReplayPolicy::Safe;
        d.implementation_contract = Some(crate::tools::descriptor::ImplementationContract {
            id: "test:arbitrary".into(),
            version: "1".into(),
        });
        reg.register(d, fake("arbitrary_name")).expect("register");

        let identity = ToolDescriptorLookup::tool_call_identity(&reg, "arbitrary_name")
            .expect("identity for registered tool");
        assert_eq!(identity.schema_version, SCHEMA_VERSION);
        assert_eq!(identity.revision, 1);
        assert_eq!(identity.replay_policy, ReplayPolicy::Safe);
        assert_eq!(
            ToolDescriptorLookup::tool_call_identity(&reg, "missing"),
            None
        );
    }

    #[test]
    fn tool_call_identity_defaults_to_unsafe_policy() {
        let reg = ToolHandlerRegistry::new();
        reg.register(desc("plain"), fake("plain"))
            .expect("register");
        assert_eq!(
            ToolDescriptorLookup::tool_call_identity(&reg, "plain")
                .unwrap()
                .replay_policy,
            ReplayPolicy::Unsafe
        );
    }

    #[test]
    fn tool_call_identity_never_infers_safe_from_idempotent() {
        let reg = ToolHandlerRegistry::new();
        let mut d = desc("idem");
        d.idempotent = true;
        reg.register(d, fake_idempotent("idem")).expect("register");
        assert_eq!(
            ToolDescriptorLookup::tool_call_identity(&reg, "idem")
                .unwrap()
                .replay_policy,
            ReplayPolicy::Unsafe
        );
    }

    #[test]
    fn tool_call_identity_tracks_replacement_generation() {
        let reg = ToolHandlerRegistry::new();
        reg.register(desc("t"), fake("t")).expect("register");
        assert_eq!(
            ToolDescriptorLookup::tool_call_identity(&reg, "t")
                .unwrap()
                .revision,
            1
        );

        let mut next = desc("t");
        next.replay_policy = ReplayPolicy::Safe;
        next.implementation_contract = Some(crate::tools::descriptor::ImplementationContract {
            id: "test:t".into(),
            version: "1".into(),
        });
        reg.replace(next, fake("t")).expect("replace");

        let identity = ToolDescriptorLookup::tool_call_identity(&reg, "t").unwrap();
        assert_eq!(
            identity.revision, 2,
            "identity must track the new generation"
        );
        assert_eq!(identity.replay_policy, ReplayPolicy::Safe);
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
        let snapshot = reg.snapshot_state();
        assert_eq!(snapshot.revision(), 2);
        assert!(!snapshot.is_closed());
        assert!(snapshot.entry("a").is_some());
        assert!(snapshot.entry("b").is_some());
        reg.close();
        let closed = reg.snapshot_state();
        assert_eq!(closed.revision(), 2);
        assert!(closed.is_closed());
        assert!(closed.entry("a").is_some());

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
            handles.push(thread::spawn(move || {
                r.register(desc("race"), fake("race"))
            }));
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
        assert!(matches!(
            evt,
            RegistryChange::Registered { revision: 1, .. }
        ));
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

    /// Regression: the `ArcSwap` state swap and the `change_tx.send` were two
    /// separate steps, so concurrent mutations could publish their change
    /// events out of revision order (thread A assigns revision 2 and sends it,
    /// then thread B — which assigned revision 1 — sends). The mutation lock
    /// covers both steps, so a subscriber must observe events in strictly
    /// increasing revision order.
    #[test]
    fn concurrent_mutations_publish_events_in_revision_order() {
        use std::sync::Arc;
        use std::thread;
        const THREADS: usize = 16;
        let reg = Arc::new(ToolHandlerRegistry::new());
        let mut rx = reg.subscribe();

        let mut handles = Vec::with_capacity(THREADS);
        for i in 0..THREADS {
            let r = Arc::clone(&reg);
            let name = format!("ord{i}");
            handles.push(thread::spawn(move || r.register(desc(&name), fake(&name))));
        }
        for h in handles {
            h.join()
                .expect("join")
                .expect("distinct-name register must succeed");
        }

        let mut seen_revisions = Vec::with_capacity(THREADS);
        while let Ok(evt) = rx.try_recv() {
            match evt {
                RegistryChange::Registered { revision, .. } => seen_revisions.push(revision),
                other => panic!("unexpected event kind: {other:?}"),
            }
        }

        assert_eq!(
            seen_revisions.len(),
            THREADS,
            "every successful mutation must emit exactly one change event"
        );
        let mut sorted = seen_revisions.clone();
        sorted.sort_unstable();
        assert_eq!(
            sorted,
            (1..=THREADS as u64).collect::<Vec<_>>(),
            "every revision 1..=N must be published exactly once"
        );
        for pair in seen_revisions.windows(2) {
            assert!(
                pair[0] < pair[1],
                "published change events must be in revision order, got {seen_revisions:?}"
            );
        }
    }

    /// The close signal wakes a registered waiter exactly once after
    /// `close()`, letting an observer distinguish "closed" from a
    /// silently-lagged change stream without polling.
    #[tokio::test]
    async fn close_signal_wakes_registered_waiter() {
        let reg = ToolHandlerRegistry::new();
        let signal = reg.close_signal();
        assert!(!reg.is_closed());

        // Explicit readiness handshake (not a yield-based guess): the waiter
        // creates its `notified()` future BEFORE signalling readiness, so the
        // test's close() is guaranteed to happen after registration.
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let reg2 = reg.clone();
        let wake = tokio::spawn(async move {
            let n = signal.notified();
            let _ = ready_tx.send(());
            n.await;
        });
        ready_rx.await.expect("waiter must signal registration");
        reg2.close();

        tokio::time::timeout(std::time::Duration::from_secs(5), wake)
            .await
            .expect("close must wake the registered waiter")
            .expect("join");
        assert!(reg2.is_closed());
    }

    /// Register-then-check ordering: a waiter that registers its `notified()`
    /// future BEFORE reading `is_closed()` never loses the wake, even when
    /// `close()` lands between the check and the await — the `Notified` future
    /// captured the generation at creation and observes the later change.
    #[tokio::test]
    async fn close_signal_register_before_check_loses_no_wake() {
        let reg = ToolHandlerRegistry::new();
        let signal = reg.close_signal();

        let notified = signal.notified();
        tokio::pin!(notified);
        if reg.is_closed() {
            // Already closed before the check: no await needed.
            return;
        }
        // Simulate close() landing right after the check: close first, then
        // await the already-registered future.
        reg.close();
        tokio::time::timeout(std::time::Duration::from_secs(5), &mut notified)
            .await
            .expect("registered-before-check waiter must observe the generation change");
        assert!(reg.is_closed());
    }

    /// `notify_waiters` releases every registered waiter, not just one.
    #[tokio::test]
    async fn close_signal_wakes_all_waiters() {
        let reg = ToolHandlerRegistry::new();
        let signal = reg.close_signal();

        // Explicit readiness handshake: each waiter creates its `notified()`
        // future BEFORE signalling readiness, so close() happens only after
        // every waiter has registered (no yield-based guess).
        let (ready_tx, mut ready_rx) = tokio::sync::mpsc::channel(8);
        let waiters: Vec<_> = (0..8)
            .map(|_| {
                let s = Arc::clone(&signal);
                let ready_tx = ready_tx.clone();
                tokio::spawn(async move {
                    let n = s.notified();
                    let _ = ready_tx.send(()).await;
                    n.await;
                })
            })
            .collect();
        for _ in 0..8 {
            ready_rx.recv().await.expect("every waiter must register");
        }
        reg.close();

        for w in waiters {
            tokio::time::timeout(std::time::Duration::from_secs(5), w)
                .await
                .expect("every waiter must be released")
                .expect("join");
        }
    }
}
