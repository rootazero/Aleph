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

use crate::capability::versioned_registry::VersionedRegistry;
use tokio::sync::{broadcast, Notify};

use crate::capability::provider::{
    ProviderHandle, ProviderId, ProviderLifecycle, ProviderState, ProviderTransport,
};
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
    /// The producer-owned result budget admitted with this handler/descriptor pair.
    pub max_result_tokens: Option<usize>,
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
        crate::tools::dispatch_verdict::with_admitted_handler(identity, self.inner.invoke(input))
            .await
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
    fn admitted(
        handler: Arc<dyn ToolHandler>,
        descriptor: ToolCapabilityDescriptor,
        max_result_tokens: Option<usize>,
    ) -> Self {
        let descriptor = Arc::new(descriptor);
        Self {
            handler: Arc::new(AdmissionHandler {
                inner: handler,
                descriptor: Arc::clone(&descriptor),
            }),
            descriptor,
            max_result_tokens,
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
    provider_state: ProviderState,
    provider_generation: u64,
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
    pub fn provider_state(&self) -> ProviderState {
        self.provider_state
    }

    #[must_use]
    pub fn provider_generation(&self) -> u64 {
        self.provider_generation
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

/// The primary publication emitted once per successful Tool transaction.
#[derive(Debug, Clone)]
pub struct RegistryPublication {
    pub revision: u64,
    pub tool_changes: Vec<RegistryChange>,
    pub catalog_changed: bool,
}

fn project_registry_publication(publication: &RegistryPublication) -> Vec<RegistryChange> {
    publication.tool_changes.clone()
}

impl CallableRecordKey {
    fn from_descriptor(descriptor: &ToolCapabilityDescriptor) -> Self {
        Self {
            source: descriptor.source.clone(),
            provider_name: descriptor.name.clone(),
        }
    }
}

/// Tool state is shared by registry clones and referenced weakly by disposers.
type RegistryShared = VersionedRegistry<CanonicalToolState, RegistryPublication, RegistryChange>;

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
        let outcome: Result<Option<()>, std::convert::Infallible> =
            shared.transact(|current, revision| {
                let Some(entry) = current.value.entry(&self.name) else {
                    return Ok(None);
                };
                // Generation guard stays in the Tool shell.
                if entry.descriptor.revision != self.revision {
                    return Ok(None);
                }
                let mut records = current.value.records.clone();
                records.remove(&CallableRecordKey::from_descriptor(&entry.descriptor));
                Ok(Some((
                    current.value.rebuild_records(records),
                    RegistryPublication {
                        revision,
                        tool_changes: vec![RegistryChange::Unregistered {
                            name: self.name.clone(),
                            revision,
                            source: entry.descriptor.source.clone(),
                        }],
                        catalog_changed: false,
                    },
                    (),
                )))
            });
        match outcome {
            Ok(result) => result.is_some(),
            Err(never) => match never {},
        }
    }
}

// ============================================================================
// Task59: Plugin Declaration Model
// ============================================================================

use crate::extension::registry::ToolRegistration;
use crate::extension::runtime::wasm::WasmBinding;
use crate::extension::visibility::ScopeKey;

/// A genuine Plugin tool declaration, including those not admitted as callable.
#[derive(Clone, Debug)]
#[allow(dead_code)] // Task59 infrastructure, used in Task60 reconciliation
pub struct PluginToolDeclaration {
    /// Actual ToolRegistration from manifest
    pub registration: ToolRegistration,
    /// Trusted owner from actual PluginRecord
    pub plugin_id: String,
    /// Actual ScopeKey from manager registry read
    pub scope_key: Option<ScopeKey>,
    /// For supported WASM with export: actual opaque binding
    pub(crate) binding: Option<WasmBinding>,
    /// Why this declaration cannot be called (if applicable)
    pub unavailable: Option<DeclarationUnavailable>,
    /// Registry revision when admitted
    pub revision: u64,
}

/// Why a Plugin declaration is not callable.
#[derive(Clone, Debug)]
pub enum DeclarationUnavailable {
    /// Non-WASM, unsupported runtime
    NoBackend,
    /// WASM module exists but export absent
    MissingExport,
    /// Status not active
    PluginDisabled,
    /// Another Plugin won short-name selection
    CollisionLoser,
    /// Builtin/MCP with same provider_name
    SuppressedByBuiltin,
}

/// Key for plugin_declarations map: (plugin_id, exact registration name).
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct PluginDeclarationKey {
    pub plugin_id: String,
    pub name: String,
}

/// Canonical tool state: callable records + plugin declarations, published atomically.
#[derive(Clone, Default)]
pub struct CanonicalToolState {
    /// Admitted callable tools (unique provider_name)
    pub records: HashMap<CallableRecordKey, RegistryEntry>,
    /// Plugin declarations (including losers and unavailable)
    pub plugin_declarations: HashMap<PluginDeclarationKey, PluginToolDeclaration>,
    /// Derived unique callable projection (provider_name → entry)
    pub callable_projection: Arc<HashMap<String, RegistryEntry>>,
}

/// Key for callable records: (source, provider_name).
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct CallableRecordKey {
    pub source: ToolSource,
    pub provider_name: String,
}

impl CanonicalToolState {
    /// Construct a state with unique names and descriptor-consistent keys.
    pub fn from_records(
        records: HashMap<CallableRecordKey, RegistryEntry>,
    ) -> Result<Self, String> {
        Self::from_records_with_declarations(records, HashMap::new())
    }

    /// Construct the callable projection and catalog from one validated value.
    pub fn from_records_with_declarations(
        records: HashMap<CallableRecordKey, RegistryEntry>,
        plugin_declarations: HashMap<PluginDeclarationKey, PluginToolDeclaration>,
    ) -> Result<Self, String> {
        let mut projection = HashMap::with_capacity(records.len());
        for (key, entry) in &records {
            if key != &CallableRecordKey::from_descriptor(&entry.descriptor) {
                return Err(format!(
                    "canonical callable key disagrees with descriptor: {}",
                    key.provider_name
                ));
            }
            if projection.contains_key(&key.provider_name) {
                return Err(format!(
                    "duplicate canonical provider name: {}",
                    key.provider_name
                ));
            }
            projection.insert(key.provider_name.clone(), entry.clone());
        }
        Ok(Self {
            records,
            plugin_declarations,
            callable_projection: Arc::new(projection),
        })
    }

    fn empty() -> Self {
        Self::from_records(HashMap::new()).expect("empty canonical state is valid")
    }

    fn entry(&self, name: &str) -> Option<&RegistryEntry> {
        self.callable_projection.get(name)
    }

    /// Callable-only mutations must retain the catalog in the same generation.
    fn rebuild_records(&self, records: HashMap<CallableRecordKey, RegistryEntry>) -> Self {
        Self::from_records_with_declarations(records, self.plugin_declarations.clone())
            .expect("validated callable mutation must preserve canonical key and name invariants")
    }
}

// ============================================================================
// End Task59 declarations
// ============================================================================

#[derive(Clone)]
pub struct ToolHandlerRegistry {
    shared: Arc<RegistryShared>,
    provider: ProviderHandle,
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
        let provider_id = ProviderId::new("aleph/tools").expect("built-in provider id is valid");
        Self::new_with_provider(ProviderHandle::new(ProviderLifecycle::ready(
            provider_id,
            ProviderTransport::InProcess,
        )))
    }

    /// Create a registry whose callable surface is fenced by `provider`.
    /// Registration state remains in the canonical registry; readiness only
    /// controls whether snapshots and resolution expose it.
    #[must_use]
    pub fn new_with_provider(provider: ProviderHandle) -> Self {
        Self {
            shared: Arc::new(VersionedRegistry::new_with_projection(
                CanonicalToolState::empty(),
                project_registry_publication,
            )),
            provider,
        }
    }

    /// Shared lifecycle handle for the provider that owns this registry.
    #[must_use]
    pub fn provider(&self) -> ProviderHandle {
        self.provider.clone()
    }

    /// Current generation of the provider that owns this registry.
    #[must_use]
    pub fn provider_generation(&self) -> u64 {
        self.provider.generation()
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
        self.register_with_result_budget(descriptor, handler, None)
    }

    pub(crate) fn register_with_result_budget(
        &self,
        descriptor: ToolCapabilityDescriptor,
        handler: Arc<dyn ToolHandler>,
        max_result_tokens: Option<usize>,
    ) -> Result<RegistrationHandle, ToolError> {
        let name = descriptor.name.clone();
        let revision = self.shared.transact(|current, assigned| {
            let definition = handler.definition();
            if current.closed {
                return Err(ToolError::RegistryClosed { name: name.clone() });
            }
            let normalized = ToolCapabilityDescriptor {
                revision: assigned,
                ..descriptor.clone()
            };
            normalized
                .validate()
                .map_err(|e| ToolError::InvalidDescriptor {
                    name: name.clone(),
                    reason: e.to_string(),
                })?;
            if !normalized.matches_definition(&definition) {
                return Err(ToolError::DescriptorMismatch {
                    name: name.clone(),
                    reason: mismatch_reason(&normalized, &definition),
                });
            }
            if current.value.entry(&name).is_some() {
                return Err(ToolError::Duplicate { name: name.clone() });
            }
            let mut records = current.value.records.clone();
            records.insert(
                CallableRecordKey::from_descriptor(&normalized),
                RegistryEntry::admitted(Arc::clone(&handler), normalized, max_result_tokens),
            );
            Ok(Some((
                current.value.rebuild_records(records),
                RegistryPublication {
                    revision: assigned,
                    tool_changes: vec![RegistryChange::Registered {
                        name: name.clone(),
                        revision: assigned,
                        source: descriptor.source,
                    }],
                    catalog_changed: false,
                },
                assigned,
            )))
        })?;
        // These shell operations always commit or return a ToolError.
        let revision = revision.expect("successful registration transaction commits");
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
        self.replace_with_result_budget(descriptor, handler, None)
    }

    pub(crate) fn replace_with_result_budget(
        &self,
        descriptor: ToolCapabilityDescriptor,
        handler: Arc<dyn ToolHandler>,
        max_result_tokens: Option<usize>,
    ) -> Result<RegistrationHandle, ToolError> {
        let name = descriptor.name.clone();
        let revision = self.shared.transact(|current, assigned| {
            let definition = handler.definition();
            if current.closed {
                return Err(ToolError::RegistryClosed { name: name.clone() });
            }
            let Some(incumbent) = current.value.entry(&name) else {
                return Err(ToolError::NotFound { name: name.clone() });
            };
            let normalized = ToolCapabilityDescriptor {
                revision: assigned,
                ..descriptor.clone()
            };
            normalized
                .validate()
                .map_err(|e| ToolError::InvalidDescriptor {
                    name: name.clone(),
                    reason: e.to_string(),
                })?;
            if !normalized.matches_definition(&definition) {
                return Err(ToolError::DescriptorMismatch {
                    name: name.clone(),
                    reason: mismatch_reason(&normalized, &definition),
                });
            }
            let mut records = current.value.records.clone();
            records.remove(&CallableRecordKey::from_descriptor(&incumbent.descriptor));
            records.insert(
                CallableRecordKey::from_descriptor(&normalized),
                RegistryEntry::admitted(Arc::clone(&handler), normalized, max_result_tokens),
            );
            Ok(Some((
                current.value.rebuild_records(records),
                RegistryPublication {
                    revision: assigned,
                    tool_changes: vec![RegistryChange::Replaced {
                        name: name.clone(),
                        revision: assigned,
                        source: descriptor.source,
                    }],
                    catalog_changed: false,
                },
                assigned,
            )))
        })?;
        // These shell operations always commit or return a ToolError.
        let revision = revision.expect("successful registration transaction commits");
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
        if !self.provider.is_ready() {
            return None;
        }
        let state = self.shared.snapshot();
        state.value.entry(name).cloned()
    }

    /// Resolve the live handler for `name`.
    #[must_use]
    pub fn resolve(&self, name: &str) -> Option<Arc<dyn ToolHandler>> {
        self.resolve_entry(name).map(|entry| entry.handler)
    }

    /// Resolve the live descriptor for `name`.
    #[must_use]
    pub fn descriptor(&self, name: &str) -> Option<Arc<ToolCapabilityDescriptor>> {
        self.resolve_entry(name)
            .map(|entry| Arc::clone(&entry.descriptor))
    }

    /// Remove a capability by name, returning the removed handler.
    ///
    /// Unlike [`RegistrationHandle::dispose`] this is unguarded; it is the
    /// teardown path (`unregister_mcp_tools`) that removes by an externally
    /// derived name set.
    #[must_use]
    pub fn unregister(&self, name: &str) -> Option<Arc<dyn ToolHandler>> {
        let outcome: Result<Option<Arc<dyn ToolHandler>>, std::convert::Infallible> =
            self.shared.transact(|current, revision| {
                let Some(entry) = current.value.entry(name) else {
                    return Ok(None);
                };
                let mut records = current.value.records.clone();
                records.remove(&CallableRecordKey::from_descriptor(&entry.descriptor));
                Ok(Some((
                    current.value.rebuild_records(records),
                    RegistryPublication {
                        revision,
                        tool_changes: vec![RegistryChange::Unregistered {
                            name: name.to_string(),
                            revision,
                            source: entry.descriptor.source.clone(),
                        }],
                        catalog_changed: false,
                    },
                    Arc::clone(&entry.handler),
                )))
            });
        match outcome {
            Ok(removed) => removed,
            Err(never) => match never {},
        }
    }

    /// Snapshot of plugin declarations from the same canonical generation.
    #[must_use]
    pub fn plugin_declarations_snapshot(
        &self,
    ) -> Arc<HashMap<PluginDeclarationKey, PluginToolDeclaration>> {
        Arc::new(self.shared.snapshot().value.plugin_declarations.clone())
    }

    /// An owned catalog projection; mutations of the copy do not affect authority.
    #[must_use]
    pub fn plugin_declarations(&self) -> HashMap<PluginDeclarationKey, PluginToolDeclaration> {
        self.shared.snapshot().value.plugin_declarations.clone()
    }

    /// Task59 compatibility boundary; producer reconciliation is not mounted.
    ///
    /// Raw public state must not bypass handler admission. Task60/runtime
    /// mounting requires separate approval; rejection never publishes a value.
    pub fn reconcile_plugin_tools(&self, _state: CanonicalToolState) -> Result<(), ToolError> {
        let current = self.shared.snapshot();
        if current.closed {
            return Err(ToolError::RegistryClosed {
                name: "reconcile_plugin_tools".into(),
            });
        }
        Err(ToolError::Execution {
            name: "reconcile_plugin_tools".into(),
            cause: "plugin producer reconciliation is not mounted".into(),
        })
    }

    /// Frozen handler view (compatibility projection).
    #[must_use]
    pub fn snapshot(&self) -> Arc<HashMap<String, Arc<dyn ToolHandler>>> {
        let state = self.snapshot_state();
        Arc::new(
            state
                .entries()
                .iter()
                .map(|(name, entry)| (name.clone(), Arc::clone(&entry.handler)))
                .collect(),
        )
    }

    /// Frozen handler+descriptor+registry-state view from one generation.
    /// Recovery adapters must use this instead of pairing independent reads.
    #[must_use]
    pub fn snapshot_state(&self) -> RegistrySnapshot {
        let state = self.shared.snapshot();
        let (provider_state, provider_generation) = self.provider.snapshot();
        let entries = if provider_state == ProviderState::Ready {
            Arc::clone(&state.value.callable_projection)
        } else {
            Arc::new(HashMap::new())
        };
        RegistrySnapshot {
            entries,
            revision: state.revision,
            closed: state.closed,
            provider_state,
            provider_generation,
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
        let state = self.snapshot_state();
        Arc::new(
            state
                .entries()
                .iter()
                .map(|(name, entry)| (name.clone(), Arc::clone(&entry.descriptor)))
                .collect(),
        )
    }

    /// The current monotonic revision. Advances by one on every successful
    /// register / replace / unregister.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.shared.revision()
    }

    /// Close the registry: stop accepting registrations and replacements.
    /// Existing resolution and disposal continue to work. Idempotent.
    pub fn close(&self) {
        self.shared.close();
    }

    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.shared.is_closed()
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
        self.shared.close_signal()
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
        self.shared.subscribe_projected()
    }

    /// Subscribe to primary whole-transaction publications for Core consumers.
    #[must_use]
    pub(crate) fn subscribe_publications(&self) -> broadcast::Receiver<RegistryPublication> {
        self.shared.subscribe()
    }

    /// Register two validated entries in one real shared transaction.
    ///
    /// This is test-only so production callers cannot manufacture a bulk
    /// registration API. It exercises the same descriptor validation,
    /// admission wrapping, duplicate and closed-state checks as `register`.
    #[cfg(test)]
    pub(crate) fn register_pair_for_test(
        &self,
        first: (ToolCapabilityDescriptor, Arc<dyn ToolHandler>),
        second: (ToolCapabilityDescriptor, Arc<dyn ToolHandler>),
    ) -> Result<(RegistrationHandle, RegistrationHandle), ToolError> {
        let first_name = first.0.name.clone();
        let second_name = second.0.name.clone();
        let revision = self.shared.transact(|current, assigned| {
            let first_definition = first.1.definition();
            let second_definition = second.1.definition();
            if current.closed {
                return Err(ToolError::RegistryClosed {
                    name: first_name.clone(),
                });
            }

            let first_descriptor = ToolCapabilityDescriptor {
                revision: assigned,
                ..first.0.clone()
            };
            first_descriptor
                .validate()
                .map_err(|e| ToolError::InvalidDescriptor {
                    name: first_name.clone(),
                    reason: e.to_string(),
                })?;
            if !first_descriptor.matches_definition(&first_definition) {
                return Err(ToolError::DescriptorMismatch {
                    name: first_name.clone(),
                    reason: mismatch_reason(&first_descriptor, &first_definition),
                });
            }

            let second_descriptor = ToolCapabilityDescriptor {
                revision: assigned,
                ..second.0.clone()
            };
            second_descriptor
                .validate()
                .map_err(|e| ToolError::InvalidDescriptor {
                    name: second_name.clone(),
                    reason: e.to_string(),
                })?;
            if !second_descriptor.matches_definition(&second_definition) {
                return Err(ToolError::DescriptorMismatch {
                    name: second_name.clone(),
                    reason: mismatch_reason(&second_descriptor, &second_definition),
                });
            }

            if first_name == second_name || current.value.entry(&first_name).is_some() {
                return Err(ToolError::Duplicate {
                    name: first_name.clone(),
                });
            }
            if current.value.entry(&second_name).is_some() {
                return Err(ToolError::Duplicate {
                    name: second_name.clone(),
                });
            }

            let mut records = current.value.records.clone();
            records.insert(
                CallableRecordKey::from_descriptor(&first_descriptor),
                RegistryEntry::admitted(Arc::clone(&first.1), first_descriptor, None),
            );
            records.insert(
                CallableRecordKey::from_descriptor(&second_descriptor),
                RegistryEntry::admitted(Arc::clone(&second.1), second_descriptor, None),
            );
            Ok(Some((
                current.value.rebuild_records(records),
                RegistryPublication {
                    revision: assigned,
                    tool_changes: vec![
                        RegistryChange::Registered {
                            name: first_name.clone(),
                            revision: assigned,
                            source: first.0.source,
                        },
                        RegistryChange::Registered {
                            name: second_name.clone(),
                            revision: assigned,
                            source: second.0.source,
                        },
                    ],
                    catalog_changed: false,
                },
                assigned,
            )))
        })?;
        let revision = revision.expect("successful pair registration commits");
        Ok((
            RegistrationHandle {
                shared: Arc::downgrade(&self.shared),
                name: first_name,
                revision,
            },
            RegistrationHandle {
                shared: Arc::downgrade(&self.shared),
                name: second_name,
                revision,
            },
        ))
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

    fn seed_catalog(reg: &ToolHandlerRegistry) -> PluginDeclarationKey {
        let key = PluginDeclarationKey {
            plugin_id: "catalog-owner".into(),
            name: "unavailable-tool".into(),
        };
        reg.shared
            .transact(|current, revision| {
                let declaration = PluginToolDeclaration {
                    registration: ToolRegistration {
                        name: key.name.clone(),
                        description: "catalog-only declaration".into(),
                        parameters: serde_json::json!({"type": "object"}),
                        handler: "missing_export".into(),
                        plugin_id: key.plugin_id.clone(),
                    },
                    plugin_id: key.plugin_id.clone(),
                    scope_key: None,
                    binding: None,
                    unavailable: Some(DeclarationUnavailable::NoBackend),
                    revision,
                };
                let next = CanonicalToolState::from_records_with_declarations(
                    current.value.records.clone(),
                    [(key.clone(), declaration)].into(),
                )
                .unwrap();
                Ok::<_, ToolError>(Some((
                    next,
                    RegistryPublication {
                        revision,
                        tool_changes: Vec::new(),
                        catalog_changed: true,
                    },
                    (),
                )))
            })
            .unwrap()
            .expect("test catalog seed must commit through the real transaction");
        key
    }

    #[test]
    fn callable_mutations_preserve_catalog_in_the_shared_generation() {
        let reg = ToolHandlerRegistry::new();
        let key = seed_catalog(&reg);
        let frozen = reg.plugin_declarations_snapshot();
        let expected = format!("{:?}", frozen.get(&key).unwrap());
        let mut primary = reg.subscribe_publications();
        let mut legacy = reg.subscribe();
        let mut revision = 1;
        let mut assert_catalog = |entry_count: usize, change_count: usize| {
            revision += 1;
            let state = reg.shared.snapshot();
            assert_eq!(state.revision, revision);
            assert_eq!(state.value.records.len(), entry_count);
            assert_eq!(state.value.callable_projection.len(), entry_count);
            assert_eq!(state.value.plugin_declarations.len(), 1);
            assert_eq!(
                format!("{:?}", state.value.plugin_declarations.get(&key).unwrap()),
                expected
            );
            assert_eq!(
                format!(
                    "{:?}",
                    reg.plugin_declarations_snapshot().get(&key).unwrap()
                ),
                expected
            );
            let mut owned = reg.plugin_declarations();
            assert_eq!(format!("{:?}", owned.remove(&key).unwrap()), expected);
            assert_eq!(reg.plugin_declarations().len(), 1);
            let event = primary.try_recv().expect("callable mutation publication");
            assert_eq!(event.revision, revision);
            assert!(!event.catalog_changed);
            assert_eq!(event.tool_changes.len(), change_count);
            for _ in 0..change_count {
                let change = legacy.try_recv().expect("legacy callable change");
                let actual = match change {
                    RegistryChange::Registered { revision, .. }
                    | RegistryChange::Replaced { revision, .. }
                    | RegistryChange::Unregistered { revision, .. } => revision,
                };
                assert_eq!(actual, revision);
            }
            assert!(matches!(
                primary.try_recv(),
                Err(broadcast::error::TryRecvError::Empty)
            ));
            assert!(matches!(
                legacy.try_recv(),
                Err(broadcast::error::TryRecvError::Empty)
            ));
        };

        let stale = reg.register(desc("a"), fake("a")).unwrap();
        assert_catalog(1, 1);
        let live = reg.replace(desc("a"), fake("a")).unwrap();
        assert_catalog(1, 1);
        reg.unregister("a").unwrap();
        assert_catalog(0, 1);
        assert!(!stale.dispose());
        assert!(!live.dispose());
        let disposable = reg.register(desc("b"), fake("b")).unwrap();
        assert_catalog(1, 1);
        assert!(disposable.dispose());
        assert_catalog(0, 1);
        reg.register_pair_for_test((desc("c"), fake("c")), (desc("d"), fake("d")))
            .unwrap();
        assert_catalog(2, 2);
        assert_eq!(format!("{:?}", frozen.get(&key).unwrap()), expected);
    }

    #[test]
    fn unmounted_reconcile_rejects_raw_state_without_publication_even_when_closed() {
        let reg = ToolHandlerRegistry::new();
        seed_catalog(&reg);
        reg.register(desc("stable"), fake("stable")).unwrap();
        let mut primary = reg.subscribe_publications();
        let mut legacy = reg.subscribe();
        for closed in [false, true] {
            if closed {
                reg.close();
            }
            let before = reg.shared.snapshot();
            // Public scaffold fields cannot become an admission bypass.
            let entry = RegistryEntry {
                handler: fake("injected"),
                descriptor: Arc::new(desc("injected")),
                max_result_tokens: Some(1),
            };
            let candidate = CanonicalToolState::from_records(
                [(CallableRecordKey::from_descriptor(&entry.descriptor), entry)].into(),
            )
            .unwrap();
            let error = reg.reconcile_plugin_tools(candidate).unwrap_err();
            if closed {
                assert!(matches!(error, ToolError::RegistryClosed { .. }));
            } else {
                assert!(matches!(
                    error,
                    ToolError::Execution { cause, .. } if cause.contains("not mounted")
                ));
            }
            let after = reg.shared.snapshot();
            assert!(Arc::ptr_eq(&before, &after));
            assert_eq!(before.revision, after.revision);
            assert_eq!(after.value.plugin_declarations.len(), 1);
            assert!(after.value.entry("stable").is_some());
            assert!(after.value.entry("injected").is_none());
            assert!(matches!(
                primary.try_recv(),
                Err(broadcast::error::TryRecvError::Empty)
            ));
            assert!(matches!(
                legacy.try_recv(),
                Err(broadcast::error::TryRecvError::Empty)
            ));
        }
    }

    #[test]
    fn public_state_constructors_reject_inconsistent_keys_and_duplicate_names() {
        let entry = RegistryEntry::admitted(fake("same"), desc("same"), None);
        let valid_key = CallableRecordKey::from_descriptor(&entry.descriptor);
        let mut other = entry.clone();
        Arc::make_mut(&mut other.descriptor).source = ToolSource::Extension {
            plugin_id: "other-owner".into(),
        };
        let other_key = CallableRecordKey::from_descriptor(&other.descriptor);
        let duplicate: HashMap<CallableRecordKey, RegistryEntry> =
            [(valid_key.clone(), entry.clone()), (other_key, other)].into();
        let wrong_name = [(
            CallableRecordKey {
                provider_name: "wrong".into(),
                ..valid_key.clone()
            },
            entry.clone(),
        )]
        .into();
        let wrong_source = [(
            CallableRecordKey {
                source: ToolSource::Extension {
                    plugin_id: "wrong".into(),
                },
                ..valid_key
            },
            entry,
        )]
        .into();
        for records in [duplicate, wrong_name, wrong_source] {
            assert!(CanonicalToolState::from_records(records.clone()).is_err());
            assert!(
                CanonicalToolState::from_records_with_declarations(records, HashMap::new())
                    .is_err()
            );
        }
    }

    #[test]
    fn provider_not_ready_hides_callable_snapshots_until_ready() {
        let provider = ProviderHandle::new(ProviderLifecycle::new(
            ProviderId::new("test/tools").unwrap(),
            ProviderTransport::Bridge,
        ));
        let registry = ToolHandlerRegistry::new_with_provider(provider.clone());
        registry
            .register(desc("lazy"), fake("lazy"))
            .expect("registration is retained before provider start");

        assert!(registry.snapshot_state().entries().is_empty());
        assert!(registry.resolve_entry("lazy").is_none());

        provider.transition(ProviderState::Starting).unwrap();
        provider.transition(ProviderState::Ready).unwrap();
        assert!(registry.snapshot_state().entry("lazy").is_some());
        assert!(registry.resolve_entry("lazy").is_some());

        provider.transition(ProviderState::Stopping).unwrap();
        assert!(registry.snapshot_state().entries().is_empty());
        assert!(registry.resolve_entry("lazy").is_none());
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
        let frozen = Arc::new(ToolHandlerRegistry {
            shared: Arc::new(VersionedRegistry::from_snapshot_with_projection(
                crate::capability::versioned_registry::VersionedSnapshot {
                    value: {
                        let key = CallableRecordKey::from_descriptor(&entry.descriptor);
                        CanonicalToolState::from_records([(key, entry.clone())].into())
                            .expect("captured admitted fixture has a consistent canonical key")
                    },
                    revision: entry.descriptor.revision,
                    closed: false,
                },
                project_registry_publication,
            )),
            provider: ToolHandlerRegistry::new().provider(),
        });
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

    #[tokio::test]
    async fn pair_helper_emits_one_real_two_member_publication() {
        let reg = ToolHandlerRegistry::new();
        let mut rx = reg.subscribe_publications();
        let mut legacy = reg.subscribe();
        let (first, second) = reg
            .register_pair_for_test(
                (desc("first"), fake("first")),
                (desc("second"), fake("second")),
            )
            .expect("pair transaction");
        let publication = rx.try_recv().expect("pair publication");
        assert_eq!(publication.revision, 1);
        assert!(!publication.catalog_changed);
        assert_eq!(publication.tool_changes.len(), 2);
        assert_eq!(first.revision(), 1);
        assert_eq!(second.revision(), 1);
        let frozen = reg.snapshot_state();
        assert_eq!(frozen.revision(), 1);
        assert_eq!(frozen.entries().len(), 2);
        assert_eq!(reg.shared.snapshot().value.records.len(), 2);
        for (change, name) in publication.tool_changes.iter().zip(["first", "second"]) {
            assert!(matches!(
                change,
                RegistryChange::Registered { name: actual, revision: 1, .. } if actual == name
            ));
            assert!(matches!(
                legacy.try_recv().expect("legacy pair member"),
                RegistryChange::Registered { name: actual, revision: 1, .. } if actual == name
            ));
            let entry = frozen.entry(name).expect("admitted pair member");
            assert_eq!(entry.descriptor.revision, 1);
            assert!(matches!(
                entry.handler.invoke(serde_json::json!({})).await,
                Err(ToolError::PermissionDenied { .. })
            ));
            assert_eq!(
                invoke_captured(entry).await.value,
                serde_json::json!({"tool": name})
            );
        }
        assert!(matches!(
            rx.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
        assert!(matches!(
            legacy.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
        assert!(first.dispose());
        assert!(second.dispose());
        assert_eq!(reg.revision(), 3);
        assert!(reg.shared.snapshot().value.records.is_empty());
        assert!(reg.snapshot_state().entries().is_empty());
        for (revision, name) in [(2, "first"), (3, "second")] {
            let removed = rx.try_recv().expect("cleanup publication");
            assert_eq!(removed.revision, revision);
            assert!(!removed.catalog_changed);
            assert!(matches!(
                removed.tool_changes.as_slice(),
                [RegistryChange::Unregistered { name: actual, revision: actual_revision, .. }]
                    if actual == name && *actual_revision == revision
            ));
            assert!(matches!(
                legacy.try_recv().expect("cleanup legacy member"),
                RegistryChange::Unregistered { name: actual, revision: actual_revision, .. }
                    if actual == name && actual_revision == revision
            ));
        }
        assert!(!first.dispose());
        assert!(!second.dispose());
        assert!(matches!(
            rx.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
        assert!(matches!(
            legacy.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
    }

    #[test]
    fn snapshot_projection_reuses_arc_and_rebuild_preserves_admitted_members() {
        let reg = ToolHandlerRegistry::new();
        reg.register(desc("stable"), fake("stable")).unwrap();
        let before = reg.snapshot_state();
        let repeated = reg.snapshot_state();
        assert!(Arc::ptr_eq(&before.entries, &repeated.entries));
        assert!(Arc::ptr_eq(&before.entries, &reg.entries_snapshot()));
        let admitted = reg.resolve_entry("stable").unwrap();
        let stable = before.entry("stable").unwrap();
        assert!(Arc::ptr_eq(&stable.handler, &admitted.handler));
        assert!(Arc::ptr_eq(&stable.descriptor, &admitted.descriptor));

        reg.register(desc("unrelated"), fake("unrelated")).unwrap();
        let after = reg.snapshot_state();
        assert!(!Arc::ptr_eq(&before.entries, &after.entries));
        assert_eq!(before.revision(), 1);
        assert_eq!(before.entries().len(), 1);
        assert!(before.entry("unrelated").is_none());
        assert_eq!(after.revision(), 2);
        assert_eq!(after.entries().len(), 2);
        let retained = after.entry("stable").unwrap();
        assert!(Arc::ptr_eq(&stable.handler, &retained.handler));
        assert!(Arc::ptr_eq(&stable.descriptor, &retained.descriptor));
        assert!(Arc::ptr_eq(&after.entries, &reg.snapshot_state().entries));
    }

    #[test]
    fn source_changing_replace_removes_actual_key_and_guards_disposal() {
        let reg = ToolHandlerRegistry::new();
        let stale = reg.register(desc("provider"), fake("provider")).unwrap();
        let before = reg.snapshot_state();
        let old_key =
            CallableRecordKey::from_descriptor(&before.entry("provider").unwrap().descriptor);
        let raw: Arc<dyn ToolHandler> = Arc::new(FakeHandler {
            name: "provider".into(),
            source: ToolSource::Mcp {
                server_id: "b2-server".into(),
            },
            idempotent: false,
        });
        let current = reg
            .replace(
                ToolCapabilityDescriptor::from_definition(&raw.definition(), 0),
                raw,
            )
            .unwrap();
        let after = reg.snapshot_state();
        let entry = after.entry("provider").unwrap();
        let new_key = CallableRecordKey::from_descriptor(&entry.descriptor);
        assert_ne!(old_key, new_key);
        let state = reg.shared.snapshot();
        assert_eq!(state.value.records.len(), 1);
        assert!(!state.value.records.contains_key(&old_key));
        let stored = state.value.records.get(&new_key).unwrap();
        assert!(Arc::ptr_eq(&stored.handler, &entry.handler));
        assert!(Arc::ptr_eq(&stored.descriptor, &entry.descriptor));
        assert_eq!(after.entries().len(), 1);
        assert_eq!(entry.descriptor.revision, current.revision());
        assert_eq!(
            before.entry("provider").unwrap().descriptor.source,
            ToolSource::Builtin
        );
        assert!(!stale.dispose());
        assert!(Arc::ptr_eq(&after.entries, &reg.snapshot_state().entries));
        reg.close();
        assert!(current.dispose());
        assert!(reg.shared.snapshot().value.records.is_empty());
        assert!(reg.snapshot_state().entries().is_empty());
        assert!(!current.dispose());
    }

    #[test]
    fn cross_source_duplicate_preserves_payload_projection_revision_and_silence() {
        let reg = ToolHandlerRegistry::new();
        let handle = reg.register(desc("duplicate"), fake("duplicate")).unwrap();
        let before = reg.shared.snapshot();
        let frozen = reg.snapshot_state();
        let mut primary = reg.subscribe_publications();
        let mut legacy = reg.subscribe();
        let raw: Arc<dyn ToolHandler> = Arc::new(FakeHandler {
            name: "duplicate".into(),
            source: ToolSource::Extension {
                plugin_id: "b2-plugin".into(),
            },
            idempotent: false,
        });
        let descriptor = ToolCapabilityDescriptor::from_definition(&raw.definition(), 2);
        assert!(descriptor.matches_definition(&raw.definition()));
        descriptor.validate().unwrap();
        let error = reg.register(descriptor, raw).unwrap_err();
        assert!(matches!(error, ToolError::Duplicate { name } if name == "duplicate"));
        let after = reg.shared.snapshot();
        assert!(Arc::ptr_eq(&before, &after));
        assert_eq!(after.revision, frozen.revision());
        assert_eq!(after.closed, before.closed);
        assert_eq!(after.value.records.len(), 1);
        assert!(Arc::ptr_eq(
            &frozen.entries,
            &after.value.callable_projection
        ));
        assert!(matches!(
            primary.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
        assert!(matches!(
            legacy.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
        assert!(handle.dispose());
    }

    #[tokio::test]
    async fn old_snapshot_keeps_real_admitted_generation_after_replace_and_teardown() {
        let reg = ToolHandlerRegistry::new();
        let raw: Arc<dyn ToolHandler> = Arc::new(RevisionHandler { generation: 1 });
        let first = reg
            .register(
                ToolCapabilityDescriptor::from_definition(&raw.definition(), 0),
                raw,
            )
            .unwrap();
        let frozen = reg.snapshot_state();
        let captured = frozen.entry("terminal").unwrap();
        let next: Arc<dyn ToolHandler> = Arc::new(RevisionHandler { generation: 2 });
        let second = reg
            .replace(
                ToolCapabilityDescriptor::from_definition(&next.definition(), 0),
                next,
            )
            .unwrap();
        let replaced = reg.snapshot_state();
        assert!(!Arc::ptr_eq(&frozen.entries, &replaced.entries));
        assert!(!Arc::ptr_eq(
            &captured.handler,
            &replaced.entry("terminal").unwrap().handler
        ));
        assert_eq!(frozen.revision(), first.revision());
        assert_eq!(captured.descriptor.revision, first.revision());
        assert_eq!(
            replaced.entry("terminal").unwrap().descriptor.revision,
            second.revision()
        );
        assert!(!first.dispose());
        reg.close();
        let removed = reg.unregister("terminal").unwrap();
        assert!(Arc::ptr_eq(
            &removed,
            &replaced.entry("terminal").unwrap().handler
        ));
        assert!(!second.dispose());
        assert!(reg.shared.snapshot().value.records.is_empty());
        assert!(reg.snapshot_state().entries().is_empty());
        assert!(!frozen.is_closed());
        assert!(matches!(
            captured.handler.invoke(serde_json::json!({})).await,
            Err(ToolError::PermissionDenied { .. })
        ));
        assert_eq!(
            invoke_captured(captured).await.value,
            serde_json::json!({"generation": 1})
        );
        assert_eq!(
            invoke_captured(replaced.entry("terminal").unwrap())
                .await
                .value,
            serde_json::json!({"generation": 2})
        );
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
    fn primary_publication_projects_to_legacy_and_omits_noops() {
        let reg = ToolHandlerRegistry::new();
        let mut primary = reg.subscribe_publications();
        let mut legacy = reg.subscribe();
        let handle = reg.register(desc("primary"), fake("primary")).unwrap();

        let publication = primary.try_recv().expect("primary publication");
        assert_eq!(publication.revision, 1);
        assert_eq!(publication.tool_changes.len(), 1);
        assert!(!publication.catalog_changed);
        assert!(matches!(
            legacy.try_recv().expect("legacy projection"),
            RegistryChange::Registered { revision: 1, .. }
        ));

        assert!(reg.register(desc("primary"), fake("primary")).is_err());
        assert!(reg.unregister("missing").is_none());
        assert!(primary.try_recv().is_err());
        assert!(legacy.try_recv().is_err());

        reg.close();
        assert!(reg.register(desc("closed"), fake("closed")).is_err());
        assert!(primary.try_recv().is_err());
        assert!(handle.dispose());
        let publication = primary.try_recv().expect("disposer publication");
        assert_eq!(publication.revision, 2);
        assert!(matches!(
            publication.tool_changes.as_slice(),
            [RegistryChange::Unregistered { revision: 2, .. }]
        ));
    }

    #[test]
    fn register_with_result_budget_commits_one_real_entry_snapshot() {
        let reg = ToolHandlerRegistry::new();
        let mut projected = reg.subscribe();
        let mut publications = reg.subscribe_publications();
        let handle = reg
            .register_with_result_budget(desc("budgeted"), fake("budgeted"), Some(17))
            .expect("budgeted registration");

        let state = reg.snapshot_state();
        let entry = state.entry("budgeted").expect("registered entry");
        let resolved = reg.resolve_entry("budgeted").expect("resolved entry");
        assert_eq!(handle.revision(), 1);
        assert_eq!(state.revision(), 1);
        assert_eq!(entry.max_result_tokens, Some(17));
        assert!(Arc::ptr_eq(&entry.handler, &resolved.handler));
        assert!(Arc::ptr_eq(&entry.descriptor, &resolved.descriptor));
        assert!(Arc::ptr_eq(&state.entries, &reg.entries_snapshot()));
        assert!(matches!(
            projected.try_recv(),
            Ok(RegistryChange::Registered { name, revision: 1, .. }) if name == "budgeted"
        ));
        assert!(matches!(
            publications.try_recv(),
            Ok(RegistryPublication { revision: 1, .. })
        ));
    }

    #[test]
    fn replace_with_result_budget_keeps_old_generation_and_stale_disposer_out() {
        let reg = ToolHandlerRegistry::new();
        let old_handle = reg
            .register_with_result_budget(desc("budgeted"), fake("budgeted"), Some(17))
            .expect("initial registration");
        let frozen = reg.snapshot_state();
        let old = frozen.entry("budgeted").expect("old entry").clone();

        let replacement = reg
            .replace_with_result_budget(desc("budgeted"), fake("budgeted"), Some(91))
            .expect("replacement");
        let current = reg.resolve_entry("budgeted").expect("current entry");
        let live_handler = reg.resolve("budgeted").expect("current handler");
        let live_descriptor = reg.descriptor("budgeted").expect("current descriptor");

        assert_eq!(old_handle.revision(), 1);
        assert_eq!(replacement.revision(), 2);
        assert_eq!(old.max_result_tokens, Some(17));
        let frozen_entry = frozen.entry("budgeted").expect("frozen entry retained");
        assert_eq!(frozen_entry.max_result_tokens, Some(17));
        assert!(Arc::ptr_eq(&frozen_entry.handler, &old.handler));
        assert!(Arc::ptr_eq(&frozen_entry.descriptor, &old.descriptor));
        assert_eq!(current.max_result_tokens, Some(91));
        assert!(Arc::ptr_eq(&current.handler, &live_handler));
        assert!(Arc::ptr_eq(&current.descriptor, &live_descriptor));
        assert!(!Arc::ptr_eq(&old.handler, &current.handler));
        assert!(!Arc::ptr_eq(&old.descriptor, &current.descriptor));
        assert!(
            !old_handle.dispose(),
            "stale disposer removed the replacement"
        );
        assert_eq!(
            reg.resolve_entry("budgeted").unwrap().max_result_tokens,
            Some(91)
        );

        let legacy = reg
            .replace(desc("budgeted"), fake("budgeted"))
            .expect("legacy replacement");
        assert_eq!(legacy.revision(), 3);
        assert_eq!(
            reg.resolve_entry("budgeted").unwrap().max_result_tokens,
            None
        );
    }

    #[test]
    fn rejected_register_and_replace_are_transactional_noops() {
        let reg = ToolHandlerRegistry::new();
        reg.register_with_result_budget(desc("seed"), fake("seed"), Some(7))
            .expect("seed registration");
        let before = reg.shared.snapshot();
        let mut projected = reg.subscribe();
        let mut publications = reg.subscribe_publications();

        macro_rules! assert_noop {
            ($expected:expr) => {{
                let after = reg.shared.snapshot();
                assert!(Arc::ptr_eq(&$expected, &after));
                assert_eq!(after.revision, $expected.revision);
                assert_eq!(
                    after.value.entry("seed").unwrap().max_result_tokens,
                    Some(7)
                );
            }};
        }

        let duplicate = reg.register_with_result_budget(desc("seed"), fake("seed"), Some(99));
        assert!(matches!(
            duplicate,
            Err(ToolError::Duplicate { name }) if name == "seed"
        ));
        assert_noop!(before);
        assert!(matches!(
            projected.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
        assert!(matches!(
            publications.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));

        let mut invalid = desc("invalid_schema");
        invalid.schema_version = 0;
        let invalid_result =
            reg.register_with_result_budget(invalid, fake("invalid_schema"), Some(99));
        assert!(matches!(
            invalid_result,
            Err(ToolError::InvalidDescriptor { name, .. }) if name == "invalid_schema"
        ));
        assert_noop!(before);
        assert!(matches!(
            projected.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
        assert!(matches!(
            publications.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));

        let mismatch = reg.register_with_result_budget(desc("mismatch"), fake("other"), Some(99));
        assert!(matches!(
            mismatch,
            Err(ToolError::DescriptorMismatch { name, .. }) if name == "mismatch"
        ));
        assert_noop!(before);
        assert!(matches!(
            projected.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
        assert!(matches!(
            publications.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));

        let mut invalid_replacement = desc("seed");
        invalid_replacement.schema_version = 0;
        assert!(matches!(
            reg.replace_with_result_budget(invalid_replacement, fake("seed"), Some(99)),
            Err(ToolError::InvalidDescriptor { .. })
        ));
        assert_noop!(before);
        assert!(matches!(
            reg.replace_with_result_budget(desc("seed"), fake("other"), Some(99)),
            Err(ToolError::DescriptorMismatch { .. })
        ));
        assert_noop!(before);
        assert!(matches!(
            projected.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
        assert!(matches!(
            publications.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));

        reg.close();
        let closed_before = reg.shared.snapshot();
        let closed_register =
            reg.register_with_result_budget(desc("closed"), fake("closed"), Some(99));
        assert!(matches!(
            closed_register,
            Err(ToolError::RegistryClosed { name }) if name == "closed"
        ));
        assert_noop!(closed_before);
        assert!(matches!(
            projected.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
        assert!(matches!(
            publications.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));

        let closed_replace = reg.replace_with_result_budget(desc("seed"), fake("seed"), Some(99));
        assert!(matches!(
            closed_replace,
            Err(ToolError::RegistryClosed { name }) if name == "seed"
        ));
        assert_noop!(closed_before);
        assert!(matches!(
            projected.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
        assert!(matches!(
            publications.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
    }

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
