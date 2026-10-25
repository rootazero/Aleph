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
use std::collections::{HashMap, HashSet};
use std::sync::Weak;

use crate::capability::versioned_registry::{VersionedRegistry, VersionedSnapshot};
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
    /// Constructed only by the crate-private atomic plugin producer.
    /// `None` preserves legacy registrations; it never certifies plugin Ready.
    pub(crate) ready: Option<Arc<ReadySnapshot>>,
}

/// Producer-owned immutable identity captured by the B2 transaction.
/// Fields are private: independent provider/owner/loader reads cannot be joined
/// by a consumer into a lease. Holding a binding is not permission to execute.
#[derive(Clone)]
pub(crate) struct ReadySnapshot {
    plugin_id: String,
    provider_state: ProviderState,
    provider: ProviderHandle,
    provider_generation: u64,
    owner_generation: u64,
    mount: crate::extension::runtime::wasm::WasmMountIdentity,
    binding: WasmBinding,
    canonical_name: String,
    entry_revision: u64,
    /// Revision of this producer publication, not the latest unrelated mutation.
    canonical_revision: u64,
    descriptor_fingerprint: [u8; 32],
    handler: Arc<dyn ToolHandler>,
    registry: Weak<RegistryShared>,
}

fn descriptor_fingerprint(descriptor: &ToolCapabilityDescriptor) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let value = serde_json::to_value(descriptor).expect("tool descriptor serializes");
    let mut bytes = Vec::new();
    crate::tools::descriptor::canonicalize_json(&value, &mut bytes);
    Sha256::digest(bytes).into()
}

impl ReadySnapshot {
    /// Read-side invalidation only, after capturing the immutable entry. Never
    /// call under the registry mutation lock or join this into producer proof.
    fn provider_is_current_ready(&self) -> bool {
        self.provider_state == ProviderState::Ready
            && self.provider.acquire_call(self.provider_generation).is_ok()
    }

    fn matches_entry(&self, entry: &RegistryEntry, revision: u64) -> bool {
        self.provider_state == ProviderState::Ready
            && self.provider_generation > 0
            && self.owner_generation > 0
            && self.entry_revision > 0
            && self.canonical_revision == self.entry_revision
            && self.canonical_revision <= revision
            && self.canonical_name == entry.descriptor.name
            && self.entry_revision == entry.descriptor.revision
            && matches!(&entry.descriptor.source, ToolSource::Extension { plugin_id } if plugin_id == &self.plugin_id)
            && self.binding.plugin_id() == self.plugin_id
            && self.binding.matches_mount(&self.mount)
            && self.descriptor_fingerprint == descriptor_fingerprint(&entry.descriptor)
            && Arc::ptr_eq(&self.handler, &entry.handler)
    }

    pub(crate) fn authorizes(&self, name: &str, plugin_id: &str, handler: &str) -> bool {
        self.canonical_name == name
            && self.plugin_id == plugin_id
            && self.binding.export() == handler
            && self.provider_is_current_ready()
    }

    pub(crate) fn binding_for(
        &self,
        provider: &ProviderHandle,
        owner_generation: u64,
    ) -> Option<WasmBinding> {
        (self.provider.same_instance(provider)
            && self.owner_generation == owner_generation
            && self.provider_is_current_ready())
        .then(|| self.binding.clone())
    }

    pub(crate) fn is_current(&self) -> bool {
        let Some(shared) = self.registry.upgrade() else {
            return false;
        };
        let current = shared.snapshot();
        !current.closed
            && current
                .value
                .entry(&self.canonical_name)
                .is_some_and(|entry| {
                    entry.ready.as_ref().is_some_and(|live| {
                        self.same_lease(live) && self.matches_entry(entry, current.revision)
                    })
                })
    }

    pub(crate) fn same_lease(&self, other: &Self) -> bool {
        self.plugin_id == other.plugin_id
            && self.provider_state == other.provider_state
            && self.provider.same_instance(&other.provider)
            && self.provider_generation == other.provider_generation
            && self.owner_generation == other.owner_generation
            && self.mount.same_instance(&other.mount)
            && self.binding.same_binding(&other.binding)
            && self.canonical_name == other.canonical_name
            && self.entry_revision == other.entry_revision
            && self.canonical_revision == other.canonical_revision
            && self.descriptor_fingerprint == other.descriptor_fingerprint
            && Arc::ptr_eq(&self.handler, &other.handler)
    }
}

/// A capture from one immutable publication, never from multiple lookups.
/// Compatibility builtin/MCP entries retain their existing cached-call rules.
/// Plugin producer captures additionally carry the full Ready identity tuple.
#[derive(Clone)]
pub struct CanonicalEntryLease {
    entry: RegistryEntry,
    revision: u64,
    provider_generation: u64,
}

impl std::fmt::Debug for CanonicalEntryLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CanonicalEntryLease")
            .field("name", &self.entry.descriptor.name)
            .field("revision", &self.revision)
            .field("provider_generation", &self.provider_generation)
            .field("producer_ready", &self.entry.ready.is_some())
            .finish()
    }
}

impl PartialEq for CanonicalEntryLease {
    fn eq(&self, other: &Self) -> bool {
        self.revision == other.revision
            && self.provider_generation == other.provider_generation
            && self.entry.descriptor == other.entry.descriptor
            && match (&self.entry.ready, &other.entry.ready) {
                (Some(left), Some(right)) => left.same_lease(right),
                (None, None) => true,
                _ => false,
            }
    }
}

impl Eq for CanonicalEntryLease {}

impl CanonicalEntryLease {
    pub fn entry(&self) -> &RegistryEntry {
        &self.entry
    }

    pub(crate) fn is_plugin_publication(&self) -> bool {
        self.entry.ready.is_some()
    }

    pub fn into_entry(self) -> RegistryEntry {
        self.entry
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn provider_generation(&self) -> u64 {
        self.provider_generation
    }

    /// Validate directly against one immutable publication snapshot. A changed
    /// producer tuple rejects even a same-name replacement.
    pub fn validate(&self, current: &CanonicalPublication) -> bool {
        if current.provider_state() != ProviderState::Ready
            || current.provider_generation() != self.provider_generation
        {
            return false;
        }
        let Some(entry) = current.entry(&self.entry.descriptor.name) else {
            return false;
        };
        if self.entry.descriptor != entry.descriptor {
            return false;
        }
        match (
            &self.entry.ready,
            current.ready_snapshot(&self.entry.descriptor.name),
        ) {
            (Some(captured), Some(live)) => {
                captured.same_lease(live) && live.provider_is_current_ready()
            }
            (None, None) => true,
            _ => false,
        }
    }

    /// Both captures must come from registry-owned immutable read boundaries.
    /// Unrelated registry mutations do not invalidate an unchanged entry; the
    /// actual plugin provider must still admit its captured generation now.
    pub(crate) fn validate_capture(&self, current: &Self) -> bool {
        if self.provider_generation != current.provider_generation
            || self.entry.descriptor != current.entry.descriptor
        {
            return false;
        }
        match (&self.entry.ready, &current.entry.ready) {
            (Some(captured), Some(live)) => {
                captured.same_lease(live) && live.provider_is_current_ready()
            }
            (None, None) => true,
            _ => false,
        }
    }
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

/// A derived callable view of one provider generation. The canonical entry
/// remains the only source of the handler, descriptor and result budget.
struct ProviderCallHandler {
    inner: Arc<dyn ToolHandler>,
    provider: ProviderHandle,
    generation: u64,
    name: String,
    ready: Option<Arc<ReadySnapshot>>,
    shared: Weak<RegistryShared>,
}

impl ProviderCallHandler {
    fn validate_ready(&self) -> Result<(), ToolError> {
        if let Some(ready) = &self.ready {
            ready
                .provider
                .acquire_call(ready.provider_generation)
                .map_err(|error| ToolError::PermissionDenied {
                    name: self.name.clone(),
                    reason: format!("canonical producer lease is stale: {error}"),
                })?;
            self.provider
                .acquire_call(self.generation)
                .map_err(|error| ToolError::PermissionDenied {
                    name: self.name.clone(),
                    reason: error.to_string(),
                })?;
            let valid = self.shared.upgrade().is_some_and(|shared| {
                let current = shared.snapshot();
                current.value.entry(&self.name).is_some_and(|entry| {
                    !current.closed
                        && entry
                            .ready
                            .as_ref()
                            .is_some_and(|live| ready.same_lease(live))
                        && ready.matches_entry(entry, current.revision)
                })
            });
            if !valid {
                return Err(ToolError::PermissionDenied {
                    name: self.name.clone(),
                    reason: "canonical producer lease is stale or not mounted".into(),
                });
            }
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl ToolHandler for ProviderCallHandler {
    async fn invoke(&self, input: Value) -> Result<ToolOutput, ToolError> {
        self.provider
            .acquire_call(self.generation)
            .map_err(|error| ToolError::PermissionDenied {
                name: self.name.clone(),
                reason: error.to_string(),
            })?;
        self.validate_ready()?;
        let result = crate::tools::dispatch_verdict::with_plugin_publication(
            self.ready.clone(),
            self.inner.invoke(input),
        )
        .await;
        // Reject a late guest result after revoke/replacement, including errors.
        self.validate_ready()?;
        result
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
                provider: self.provider.clone(),
                generation: self.generation,
                name: self.name.clone(),
                ready: self.ready.clone(),
                shared: self.shared.clone(),
            }) as Arc<dyn ToolHandler>
        })
    }
}

impl RegistryEntry {
    fn for_provider(
        &self,
        provider: &ProviderHandle,
        generation: u64,
        shared: &Arc<RegistryShared>,
    ) -> Self {
        #[cfg(test)]
        tests::PROVIDER_VIEW_CONSTRUCTIONS.with(|count| count.set(count.get() + 1));
        Self {
            handler: Arc::new(ProviderCallHandler {
                inner: Arc::clone(&self.handler),
                provider: provider.clone(),
                generation,
                name: self.descriptor.name.clone(),
                ready: self.ready.clone(),
                shared: Arc::downgrade(shared),
            }),
            descriptor: Arc::clone(&self.descriptor),
            max_result_tokens: self.max_result_tokens,
            ready: self.ready.clone(),
        }
    }

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
            ready: None,
        }
    }
}

/// One immutable registry generation. Entries, the mutation revision and the
/// closed flag are captured from the same `ArcSwap` state, so a recovery
/// adapter cannot pair a handler from one generation with a descriptor from
/// another.
#[derive(Clone)]
pub struct CanonicalPublication {
    entries: Arc<HashMap<String, RegistryEntry>>,
    revision: u64,
    closed: bool,
    provider_state: ProviderState,
    provider_generation: u64,
}

/// One raw immutable registry cut plus the provider state captured at its
/// ArcSwap load. Single-entry and full-map views share this boundary and gate.
struct CanonicalRead {
    state: Arc<VersionedSnapshot<CanonicalToolState>>,
    provider_state: ProviderState,
    provider_generation: u64,
}

impl CanonicalRead {
    fn callable_entry(
        &self,
        registry: &ToolHandlerRegistry,
        entry: &RegistryEntry,
    ) -> Option<RegistryEntry> {
        if self.provider_state != ProviderState::Ready
            || entry.ready.as_ref().is_some_and(|ready| {
                self.state.closed
                    || !ready.matches_entry(entry, self.state.revision)
                    || !ready.provider_is_current_ready()
            })
        {
            return None;
        }
        Some(entry.for_provider(
            &registry.provider,
            self.provider_generation,
            &registry.shared,
        ))
    }
}

/// Compatibility name; both APIs return the same immutable read boundary.
pub type RegistrySnapshot = CanonicalPublication;

impl CanonicalPublication {
    /// Capture one frozen entry, then reject a stopped/restarted producer using
    /// its own live provider. This read can invalidate, never create, authority.
    pub fn capture_entry(&self, name: &str) -> Option<CanonicalEntryLease> {
        if self.provider_state != ProviderState::Ready {
            return None;
        }
        let entry = self.entry(name)?.clone();
        // Legacy entries carry no producer proof; do not infer one from source.
        if entry
            .ready
            .as_ref()
            .is_some_and(|ready| !ready.provider_is_current_ready())
        {
            return None;
        }
        Some(CanonicalEntryLease {
            entry,
            revision: self.revision,
            provider_generation: self.provider_generation,
        })
    }

    /// Whether this immutable canonical cut contains a Ready producer entry for
    /// the plugin. Compatibility callers use this only to reject an ownerless
    /// path after canonical publication has taken authority for that plugin.
    #[must_use]
    pub fn has_ready_plugin(&self, plugin_id: &str) -> bool {
        self.entries.values().any(|entry| {
            matches!(&entry.descriptor.source, ToolSource::Extension { plugin_id: id } if id == plugin_id)
                && entry.ready.is_some()
        })
    }

    /// A legacy registration is not evidence of a Ready plugin publication.
    /// B2 must install this proof atomically; B1 never manufactures one.
    pub(crate) fn ready_snapshot(&self, name: &str) -> Option<&ReadySnapshot> {
        self.entry(name)?.ready.as_deref()
    }

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
                if entry.descriptor.revision != self.revision || entry.ready.is_some() {
                    // Producer handles are private inside a batch receipt; even
                    // an internal misuse cannot partially withdraw that scope.
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

/// Trusted B3 owner input, not public authority. The caller must hold the real
/// owner admission fence and supply its actual provider, generation and mount.
/// Nothing here activates a plugin or discovers authority from metadata.
#[allow(dead_code)] // B3 runtime-owner wiring is deliberately not mounted.
pub(crate) struct PluginBatch {
    pub plugin_id: String,
    pub provider: ProviderHandle,
    pub provider_generation: u64,
    pub owner_generation: u64,
    pub mount: crate::extension::runtime::wasm::WasmMountIdentity,
    pub entries: Vec<PluginBatchEntry>,
}

#[allow(dead_code)] // Constructed by the future B3 owner, never by a resolver.
pub(crate) struct PluginBatchEntry {
    /// Must be unversioned (revision zero); only this registry assigns identity.
    pub descriptor: ToolCapabilityDescriptor,
    pub handler: Arc<dyn ToolHandler>,
    pub binding: WasmBinding,
    pub max_result_tokens: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[allow(dead_code)]
pub(crate) enum PluginPublicationError {
    #[error("plugin batch is empty")]
    EmptyBatch,
    #[error("invalid plugin producer identity: {0}")]
    InvalidIdentity(&'static str),
    #[error(transparent)]
    Provider(#[from] crate::capability::provider::ProviderCallError),
    #[error("invalid plugin entry {name}: {reason}")]
    InvalidEntry { name: String, reason: String },
    #[error("callable name already registered or repeated: {name}")]
    Duplicate { name: String },
    #[error("canonical registry is closed")]
    Closed,
    #[error("plugin scope already has entries; exact batch replacement required")]
    ScopeOccupied,
    #[error("plugin batch receipt is stale or incomplete: {name}")]
    Stale { name: String },
    #[error("plugin batch registry is no longer available")]
    RegistryGone,
}

/// Immutable withdrawal receipt, not another registry or disposer map. Existing
/// RegistrationHandles supply name/revision/registry identity; the accompanying
/// registry-created proofs seal the rest of the tuple. Per-entry handles stay
/// private so a plugin batch cannot be partially disposed.
#[derive(Clone)]
#[allow(dead_code)]
pub(crate) struct PluginBatchRegistration {
    entries: Vec<(RegistrationHandle, Arc<ReadySnapshot>)>,
}

impl std::fmt::Debug for PluginBatchRegistration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list()
            .entries(self.entries.iter().map(|(handle, _)| handle))
            .finish()
    }
}

#[allow(dead_code)] // B3 owns the receipt and invokes revoke during stop/replace.
impl PluginBatchRegistration {
    fn validate_current(
        &self,
        shared: &Arc<RegistryShared>,
        current: &VersionedSnapshot<CanonicalToolState>,
    ) -> Result<(), PluginPublicationError> {
        let Some((_, first)) = self.entries.first() else {
            return Err(PluginPublicationError::EmptyBatch);
        };
        let scope_count = current.value.records.values().filter(|entry| {
            matches!(&entry.descriptor.source, ToolSource::Extension { plugin_id } if plugin_id == &first.plugin_id)
        }).count();
        if scope_count != self.entries.len() {
            return Err(PluginPublicationError::Stale {
                name: first.plugin_id.clone(),
            });
        }
        for (handle, ready) in &self.entries {
            let valid = Weak::ptr_eq(&handle.shared, &Arc::downgrade(shared))
                && current.value.entry(&handle.name).is_some_and(|entry| {
                    entry.descriptor.revision == handle.revision
                        && ready.matches_entry(entry, current.revision)
                        && entry
                            .ready
                            .as_ref()
                            .is_some_and(|live| ready.same_lease(live))
                });
            if !valid {
                return Err(PluginPublicationError::Stale {
                    name: handle.name.clone(),
                });
            }
        }
        Ok(())
    }

    /// All-or-none exact withdrawal. Stale/missing/partial receipts are errors,
    /// never successful no-ops. Any failure leaves the entire old snapshot and
    /// this receipt intact for owner residual reporting/recovery. No external
    /// disposer runs under the transaction; effect-scope cleanup belongs to B3.
    pub(crate) fn revoke(&self) -> Result<u64, PluginPublicationError> {
        let (first, _) = self
            .entries
            .first()
            .ok_or(PluginPublicationError::EmptyBatch)?;
        let shared = first
            .shared
            .upgrade()
            .ok_or(PluginPublicationError::RegistryGone)?;
        let result = shared.transact(|current, revision| -> Result<_, PluginPublicationError> {
            self.validate_current(&shared, current)?;
            let mut records = current.value.records.clone();
            let mut changes = Vec::with_capacity(self.entries.len());
            for (handle, ready) in &self.entries {
                records.remove(&CallableRecordKey {
                    source: ToolSource::Extension {
                        plugin_id: ready.plugin_id.clone(),
                    },
                    provider_name: handle.name.clone(),
                });
                changes.push(RegistryChange::Unregistered {
                    name: handle.name.clone(),
                    revision,
                    source: ToolSource::Extension {
                        plugin_id: ready.plugin_id.clone(),
                    },
                });
            }
            Ok(Some((
                current.value.rebuild_records(records),
                RegistryPublication {
                    revision,
                    tool_changes: changes,
                    catalog_changed: false,
                },
                revision,
            )))
        })?;
        Ok(result.expect("exact batch withdrawal commits or errors"))
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

    fn has_ready_plugin_source(&self, source: &ToolSource) -> bool {
        matches!(source, ToolSource::Extension { .. })
            && self
                .records
                .values()
                .any(|entry| entry.ready.is_some() && &entry.descriptor.source == source)
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

    /// Compare registry identity without exposing any registry state.
    #[must_use]
    pub(crate) fn same_instance(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.shared, &other.shared)
    }

    /// Current generation of the provider that owns this registry.
    #[must_use]
    pub fn provider_generation(&self) -> u64 {
        self.provider.generation()
    }

    /// Publish for an already Ready producer; shares the guarded-commit path's
    /// preflight and single transaction. No caller can supply a ReadySnapshot.
    #[allow(dead_code)] // Also exercised by producer contract tests.
    pub(crate) fn publish_plugin_batch(
        &self,
        batch: PluginBatch,
        replacing: Option<&PluginBatchRegistration>,
    ) -> Result<PluginBatchRegistration, PluginPublicationError> {
        let names = Self::preflight_plugin_batch(&batch)?;
        batch
            .provider
            .with_snapshot(|provider| self.commit_plugin_batch(&batch, replacing, names, provider))
    }

    /// RuntimeOwner holds its control fence and has prepared all bindings and
    /// definitions before this call. Ready state and the batch are committed
    /// under the actual provider lifecycle guard, never independent snapshots.
    pub(crate) fn publish_plugin_batch_with_ready_commit(
        &self,
        batch: PluginBatch,
    ) -> Result<PluginBatchRegistration, PluginPublicationError> {
        let names = Self::preflight_plugin_batch(&batch)?;
        batch
            .provider
            .with_ready_commit(batch.provider_generation, |provider| {
                self.commit_plugin_batch(&batch, None, names, provider)
            })
    }

    fn preflight_plugin_batch(
        batch: &PluginBatch,
    ) -> Result<HashSet<String>, PluginPublicationError> {
        if batch.entries.is_empty() {
            return Err(PluginPublicationError::EmptyBatch);
        }
        if batch.plugin_id.trim().is_empty()
            || batch.owner_generation == 0
            || batch.provider_generation == 0
        {
            return Err(PluginPublicationError::InvalidIdentity(
                "empty plugin or zero generation",
            ));
        }
        let mut names = HashSet::new();
        for entry in &batch.entries {
            let descriptor = &entry.descriptor;
            let invalid = |reason: String| PluginPublicationError::InvalidEntry {
                name: descriptor.name.clone(),
                reason,
            };
            if descriptor.revision != 0 {
                return Err(invalid(
                    "producer descriptor must have revision zero".into(),
                ));
            }
            // Validate the unversioned input using a provisional nonzero value;
            // the only authoritative revision is assigned under transact below.
            ToolCapabilityDescriptor {
                revision: 1,
                ..descriptor.clone()
            }
            .validate()
            .map_err(|error| invalid(error.to_string()))?;
            let definition = entry.handler.definition();
            if !descriptor.matches_definition(&definition) {
                return Err(invalid(mismatch_reason(descriptor, &definition)));
            }
            if !matches!(&descriptor.source, ToolSource::Extension { plugin_id } if plugin_id == &batch.plugin_id)
                || entry.binding.plugin_id() != batch.plugin_id
                || !entry.binding.matches_mount(&batch.mount)
            {
                return Err(invalid("plugin source/binding/mount mismatch".into()));
            }
            if !names.insert(descriptor.name.clone()) {
                return Err(PluginPublicationError::Duplicate {
                    name: descriptor.name.clone(),
                });
            }
        }
        Ok(names)
    }

    fn commit_plugin_batch(
        &self,
        batch: &PluginBatch,
        replacing: Option<&PluginBatchRegistration>,
        names: HashSet<String>,
        provider: &crate::capability::provider::ProviderLifecycle,
    ) -> Result<PluginBatchRegistration, PluginPublicationError> {
        if provider.provider_id().as_str() != format!("plugin/{}", batch.plugin_id) {
            return Err(PluginPublicationError::InvalidIdentity(
                "provider does not belong to plugin",
            ));
        }
        provider.acquire_call(batch.provider_generation)?;
        let result = self.shared.transact(|current, revision| {
                if current.closed {
                    return Err(PluginPublicationError::Closed);
                }
                if let Some(previous) = replacing {
                    previous.validate_current(&self.shared, current)?;
                    let old = &previous.entries[0].1;
                    if old.plugin_id != batch.plugin_id
                        || batch.owner_generation <= old.owner_generation
                        || (old.provider.same_instance(&batch.provider)
                            && batch.provider_generation < old.provider_generation)
                    {
                        return Err(PluginPublicationError::InvalidIdentity("replacement must advance the same plugin owner"));
                    }
                } else if current.value.records.values().any(|entry| {
                    matches!(&entry.descriptor.source, ToolSource::Extension { plugin_id } if plugin_id == &batch.plugin_id)
                }) {
                    return Err(PluginPublicationError::ScopeOccupied);
                }
                let mut records = current.value.records.clone();
                let mut changes = Vec::new();
                if let Some(previous) = replacing {
                    for (handle, old) in &previous.entries {
                        records.remove(&CallableRecordKey {
                            source: ToolSource::Extension { plugin_id: old.plugin_id.clone() },
                            provider_name: handle.name.clone(),
                        });
                        changes.push(RegistryChange::Unregistered {
                            name: handle.name.clone(), revision,
                            source: ToolSource::Extension { plugin_id: old.plugin_id.clone() },
                        });
                    }
                }
                // Conflict checks happen before building any entry or proof.
                for name in &names {
                    if records.values().any(|entry| &entry.descriptor.name == name) {
                        return Err(PluginPublicationError::Duplicate { name: name.clone() });
                    }
                }
                let mut receipt = Vec::with_capacity(batch.entries.len());
                for input in &batch.entries {
                    let descriptor = ToolCapabilityDescriptor { revision, ..input.descriptor.clone() };
                    let mut entry = RegistryEntry::admitted(
                        Arc::clone(&input.handler), descriptor, input.max_result_tokens,
                    );
                    let ready = Arc::new(ReadySnapshot {
                        plugin_id: batch.plugin_id.clone(), provider_state: ProviderState::Ready,
                        provider: batch.provider.clone(), provider_generation: batch.provider_generation,
                        owner_generation: batch.owner_generation, mount: batch.mount.clone(),
                        binding: input.binding.clone(), canonical_name: entry.descriptor.name.clone(),
                        entry_revision: revision, canonical_revision: revision,
                        descriptor_fingerprint: descriptor_fingerprint(&entry.descriptor),
                        handler: Arc::clone(&entry.handler),
                        registry: Arc::downgrade(&self.shared),
                    });
                    entry.ready = Some(Arc::clone(&ready));
                    let name = entry.descriptor.name.clone();
                    changes.push(RegistryChange::Registered {
                        name: name.clone(), revision, source: entry.descriptor.source.clone(),
                    });
                    records.insert(CallableRecordKey::from_descriptor(&entry.descriptor), entry);
                    receipt.push((RegistrationHandle { shared: Arc::downgrade(&self.shared), name, revision }, ready));
                }
                Ok(Some((current.value.rebuild_records(records), RegistryPublication {
                    revision, tool_changes: changes, catalog_changed: false,
                }, PluginBatchRegistration { entries: receipt })))
            })?;
        Ok(result.expect("plugin publication commits or errors"))
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
        let definition = handler.definition();
        let revision = self.shared.transact(|current, assigned| {
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
            if current.value.has_ready_plugin_source(&normalized.source) {
                return Err(ToolError::Execution {
                    name: name.clone(),
                    cause: "canonical plugin scope requires batch registration".into(),
                });
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
        let definition = handler.definition();
        let revision = self.shared.transact(|current, assigned| {
            if current.closed {
                return Err(ToolError::RegistryClosed { name: name.clone() });
            }
            let Some(incumbent) = current.value.entry(&name) else {
                return Err(ToolError::NotFound { name: name.clone() });
            };
            if incumbent.ready.is_some()
                || current.value.has_ready_plugin_source(&descriptor.source)
            {
                return Err(ToolError::Execution {
                    name: name.clone(),
                    cause: "canonical plugin replacement requires its batch receipt".into(),
                });
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
    /// The owned entry remains paired after replacement or removal, but its
    /// captured provider generation must still be Ready at invocation time.
    /// This is an exact canonical-name lookup, not dispatch authorization.
    #[must_use]
    pub(crate) fn has_plugin_publication(&self, plugin_id: &str) -> bool {
        // Inspect the producer record, not the filtered callable projection:
        // a stopped provider cannot reopen an ownerless compatibility route.
        self.shared.snapshot().value.records.values().any(|entry| {
            entry
                .ready
                .as_ref()
                .is_some_and(|ready| ready.plugin_id == plugin_id)
        })
    }

    pub fn resolve_entry(&self, name: &str) -> Option<RegistryEntry> {
        self.capture_entry(name)
            .map(CanonicalEntryLease::into_entry)
    }

    /// Capture only the requested entry from the same immutable read boundary
    /// as `canonical_publication`, without wrapping or hashing unrelated entries.
    #[must_use]
    pub fn capture_entry(&self, name: &str) -> Option<CanonicalEntryLease> {
        let read = self.canonical_read();
        let entry = read.callable_entry(self, read.state.value.entry(name)?)?;
        Some(CanonicalEntryLease {
            entry,
            revision: read.state.revision,
            provider_generation: read.provider_generation,
        })
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
    /// derived name set. Canonical plugin batches reject name-only withdrawal;
    /// their owner must use the exact batch receipt's structured `revoke`.
    #[must_use]
    pub fn unregister(&self, name: &str) -> Option<Arc<dyn ToolHandler>> {
        let provider_generation = self.provider.generation();
        let outcome: Result<Option<Arc<dyn ToolHandler>>, std::convert::Infallible> =
            self.shared.transact(|current, revision| {
                let Some(entry) = current.value.entry(name) else {
                    return Ok(None);
                };
                if entry.ready.is_some() {
                    return Ok(None);
                }
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
                    entry
                        .for_provider(&self.provider, provider_generation, &self.shared)
                        .handler,
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

    /// Read-only compatibility acknowledgement, not a second publication API.
    /// Supplied public state is never imported as authority. Success only means
    /// a real producer has already committed a current Ready proof; Cold and
    /// compatibility-only registries keep the producer-not-mounted boundary.
    pub fn reconcile_plugin_tools(&self, _state: CanonicalToolState) -> Result<(), ToolError> {
        let current = self.shared.snapshot();
        if current.closed {
            return Err(ToolError::RegistryClosed {
                name: "reconcile_plugin_tools".into(),
            });
        }
        let publication = self.canonical_publication();
        // canonical_publication already validated the exact raw entry proof
        // before wrapping its handler. Do not compare that wrapper to the raw
        // identity stored by the producer.
        if publication
            .entries()
            .keys()
            .any(|name| publication.ready_snapshot(name).is_some())
        {
            return Ok(());
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
        self.canonical_publication()
    }

    /// One canonical read boundary for dispatch and projection. Compatibility
    /// provider state is captured while transitions are excluded; the registry
    /// ArcSwap load is the read linearization point. Producer identity/binding
    /// must already be frozen on each entry by B2, never joined on the read side.
    /// The plugin's live provider can only invalidate that frozen proof; its
    /// check runs after the default provider guard and outside mutation locks.
    /// This does NOT make the independent legacy writes a publish transaction.
    #[must_use]
    pub fn canonical_publication(&self) -> CanonicalPublication {
        self.canonical_publication_inner(
            #[cfg(test)]
            None,
        )
    }

    fn canonical_publication_inner(
        &self,
        #[cfg(test)] after_producer_verdict: Option<&dyn Fn()>,
    ) -> CanonicalPublication {
        let read = self.canonical_read();
        // One verdict from each actual provider instance, never one per entry.
        // Textual provider/plugin names are not instance identity. Verdicts are
        // local values for this immutable cut, not a second provider authority.
        let mut producers: Vec<(ProviderHandle, (ProviderState, u64))> = Vec::new();
        for entry in read.state.value.callable_projection.values() {
            if let Some(ready) = &entry.ready {
                if !producers
                    .iter()
                    .any(|(provider, _)| provider.same_instance(&ready.provider))
                {
                    producers.push((ready.provider.clone(), ready.provider.snapshot()));
                    #[cfg(test)]
                    if let Some(after_verdict) = after_producer_verdict {
                        after_verdict();
                    }
                }
            }
        }
        let entries = Arc::new(
            read.state
                .value
                .callable_projection
                .iter()
                .filter_map(|(name, entry)| {
                    if read.provider_state != ProviderState::Ready {
                        return None;
                    }
                    if let Some(ready) = &entry.ready {
                        let (_, verdict) = producers
                            .iter()
                            .find(|(provider, _)| provider.same_instance(&ready.provider))?;
                        if read.state.closed
                            || *verdict != (ProviderState::Ready, ready.provider_generation)
                            || !ready.matches_entry(entry, read.state.revision)
                        {
                            return None;
                        }
                    }
                    Some((
                        name.clone(),
                        entry.for_provider(&self.provider, read.provider_generation, &self.shared),
                    ))
                })
                .collect(),
        );
        CanonicalPublication {
            entries,
            revision: read.state.revision,
            closed: read.state.closed,
            provider_state: read.provider_state,
            provider_generation: read.provider_generation,
        }
    }

    fn canonical_read(&self) -> CanonicalRead {
        // Exclude provider transitions only across the ArcSwap load. Projection
        // and target-only capture derive from this same frozen cut afterward.
        self.provider.with_snapshot(|provider| CanonicalRead {
            state: self.shared.snapshot(),
            provider_state: provider.state(),
            provider_generation: provider.generation(),
        })
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
        let first_definition = first.1.definition();
        let second_definition = second.1.definition();
        let revision = self.shared.transact(|current, assigned| {
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

    // Count actual provider-wrapper constructions on this test's thread, not
    // capture API calls. Production builds contain no instrumentation.
    std::thread_local! {
        pub(super) static PROVIDER_VIEW_CONSTRUCTIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    // B1 fixtures exercise a real opaque mount/binding but do not claim guest
    // execution or a production publish transaction (those belong to B2-B6).
    mod b1 {
        use super::*;
        use crate::capability::versioned_registry::VersionedSnapshot;
        use crate::extension::runtime::wasm::tests::guest_fixture::{
            manifest_with_guest, HANDLER, PAYLOAD_A, PAYLOAD_B,
        };
        use crate::extension::runtime::wasm::WasmRuntime;
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct Probe {
            epoch: u64,
            calls: Arc<AtomicUsize>,
            pause: Option<(Arc<Notify>, Arc<Notify>)>,
        }

        #[async_trait]
        impl ToolHandler for Probe {
            async fn invoke(&self, _: Value) -> Result<ToolOutput, ToolError> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                if let Some((entered, release)) = &self.pause {
                    entered.notify_one();
                    release.notified().await;
                }
                Ok(ToolOutput {
                    value: serde_json::json!({"epoch": self.epoch}),
                    metadata: Default::default(),
                })
            }

            fn definition(&self) -> ToolDefinition {
                ToolDefinition {
                    name: "b1_probe".into(),
                    description: format!("epoch {}", self.epoch),
                    input_schema: serde_json::json!({"type": "object"}),
                    source: ToolSource::Extension {
                        plugin_id: "b1-plugin".into(),
                    },
                    metadata: Default::default(),
                }
            }
        }

        fn ready_provider(generation: u64) -> ProviderHandle {
            let mut lifecycle = ProviderLifecycle::ready(
                ProviderId::new("plugin/b1-plugin").unwrap(),
                ProviderTransport::InProcess,
            );
            for _ in 1..generation {
                lifecycle.transition(ProviderState::Stopping).unwrap();
                lifecycle.transition(ProviderState::Stopped).unwrap();
                lifecycle.transition(ProviderState::Starting).unwrap();
                lifecycle.transition(ProviderState::Ready).unwrap();
            }
            ProviderHandle::new(lifecycle)
        }

        fn mounted_entry(epoch: u64, calls: Arc<AtomicUsize>) -> RegistryEntry {
            let directory = tempfile::TempDir::new().unwrap();
            let payload = if epoch == 1 { PAYLOAD_A } else { PAYLOAD_B };
            let manifest = manifest_with_guest(directory.path(), "b1-plugin", payload);
            let mut runtime = WasmRuntime::new();
            runtime.load_plugin(&manifest).unwrap();
            let raw: Arc<dyn ToolHandler> = Arc::new(Probe {
                epoch,
                calls,
                pause: None,
            });
            let descriptor = ToolCapabilityDescriptor::from_definition(&raw.definition(), epoch);
            let mut entry = RegistryEntry::admitted(raw, descriptor, Some(123));
            entry.ready = Some(Arc::new(ReadySnapshot {
                plugin_id: "b1-plugin".into(),
                provider_state: ProviderState::Ready,
                provider: ready_provider(epoch + 6),
                provider_generation: epoch + 6,
                owner_generation: epoch + 12,
                mount: runtime.mounted_instance("b1-plugin").unwrap(),
                binding: runtime.bind("b1-plugin", HANDLER).unwrap(),
                canonical_name: entry.descriptor.name.clone(),
                entry_revision: epoch,
                canonical_revision: epoch,
                descriptor_fingerprint: descriptor_fingerprint(&entry.descriptor),
                handler: Arc::clone(&entry.handler),
                registry: Weak::new(),
            }));
            entry
        }

        fn registry_at(entry: RegistryEntry) -> ToolHandlerRegistry {
            let revision = entry.descriptor.revision;
            let records = [(CallableRecordKey::from_descriptor(&entry.descriptor), entry)].into();
            ToolHandlerRegistry {
                shared: Arc::new(VersionedRegistry::from_snapshot_with_projection(
                    VersionedSnapshot {
                        value: CanonicalToolState::from_records(records).unwrap(),
                        revision,
                        closed: false,
                    },
                    project_registry_publication,
                )),
                provider: ToolHandlerRegistry::new().provider(),
            }
        }

        fn install_for_test(registry: &ToolHandlerRegistry, entry: RegistryEntry) {
            registry
                .shared
                .transact(|current, revision| {
                    let mut records = current.value.records.clone();
                    records.insert(CallableRecordKey::from_descriptor(&entry.descriptor), entry);
                    Ok::<_, ()>(Some((
                        current.value.rebuild_records(records),
                        RegistryPublication {
                            revision,
                            tool_changes: Vec::new(),
                            catalog_changed: false,
                        },
                        (),
                    )))
                })
                .unwrap()
                .unwrap();
        }

        async fn call_through_service(
            registry: ToolHandlerRegistry,
        ) -> Result<ToolOutput, ToolError> {
            call_through_service_named(registry, "b1_probe").await
        }

        pub(super) async fn call_through_service_named(
            registry: ToolHandlerRegistry,
            name: &str,
        ) -> Result<ToolOutput, ToolError> {
            use crate::tools::service::ToolService;
            let registry = Arc::new(registry);
            let publication = registry.canonical_publication();
            let entry = publication.entry(name).unwrap();
            let mut projection = crate::tools::runtime::LoopToolRegistry::new();
            let facade = crate::capability::zahir_facade::ZahirFacade::new(
                registry.as_ref().clone(),
                Arc::new(crate::capability::ownership::OwnershipTree::new()),
            );
            projection.bind_canonical_registry(Arc::clone(&registry), Arc::new(|_| true));
            assert!(projection.bind_canonical_facade(facade));
            projection.register(Box::new(
                crate::tools::adapters::McpRegistryTool::from_registry_entry(
                    Arc::clone(&entry.handler),
                    &entry.descriptor,
                ),
            ));
            let service = crate::tools::scoped::ScopedToolService::new(
                Arc::new(projection),
                Default::default(),
            );
            let mut output = crate::approval::with_call_identity(
                Some(crate::approval::CallIdentity {
                    turn_id: crate::session::events::TurnId::nil(),
                    call_id: "b1-live-fixture".into(),
                }),
                service.execute(name, serde_json::json!({})),
            )
            .await?;
            output.value = serde_json::from_str(output.value.as_str().unwrap())
                .unwrap_or_else(|_| output.value.clone());
            Ok(output)
        }

        // Value: rejects remount/restart/same-name replacement and late effects;
        // unlike provider-only tests, every owner/binding/descriptor field matters.
        #[tokio::test]
        async fn full_ready_tuple_changes_reject_lease_and_cached_handler() {
            let calls = Arc::new(AtomicUsize::new(0));
            let base = mounted_entry(1, Arc::clone(&calls));
            let replacement = mounted_entry(2, Arc::clone(&calls));
            let mut variants = Vec::new();
            for field in [
                "plugin_id",
                "canonical_name",
                "provider_state",
                "provider",
                "provider_generation",
                "owner_generation",
                "mount",
                "binding",
                "entry_revision",
                "canonical_revision",
                "fingerprint",
                "handler",
                "descriptor",
            ] {
                let mut entry = base.clone();
                let ready = Arc::make_mut(entry.ready.as_mut().unwrap());
                match field {
                    "plugin_id" => ready.plugin_id = "replacement-plugin".into(),
                    "canonical_name" => ready.canonical_name = "other-name".into(),
                    "provider_state" => ready.provider_state = ProviderState::Starting,
                    "provider" => {
                        ready.provider = replacement.ready.as_ref().unwrap().provider.clone()
                    }
                    "provider_generation" => ready.provider_generation += 1,
                    "owner_generation" => ready.owner_generation += 1,
                    "mount" => ready.mount = replacement.ready.as_ref().unwrap().mount.clone(),
                    "binding" => {
                        ready.binding = replacement.ready.as_ref().unwrap().binding.clone()
                    }
                    "entry_revision" => ready.entry_revision += 1,
                    "canonical_revision" => ready.canonical_revision += 1,
                    "fingerprint" => ready.descriptor_fingerprint[0] ^= 1,
                    "handler" => {
                        entry.handler = Arc::clone(&replacement.handler);
                        ready.handler = Arc::clone(&entry.handler);
                    }
                    "descriptor" => {
                        Arc::make_mut(&mut entry.descriptor).requires_confirmation = true
                    }
                    _ => unreachable!(),
                }
                variants.push((field, entry));
            }
            for (field, changed) in variants {
                let registry = registry_at(base.clone());
                let publication = registry.canonical_publication();
                let lease = registry.capture_entry("b1_probe").unwrap();
                assert!(lease.validate(&publication));
                assert_eq!(
                    call_through_service(registry.clone()).await.unwrap().value,
                    serde_json::json!({"epoch": 1})
                );
                let before = calls.load(Ordering::SeqCst);
                install_for_test(&registry, changed);
                assert!(
                    !lease.validate(&registry.canonical_publication()),
                    "{field}"
                );
                assert!(
                    !registry
                        .capture_entry("b1_probe")
                        .as_ref()
                        .is_some_and(|live| lease.validate_capture(live)),
                    "single-entry capture accepted changed {field}"
                );
                assert!(
                    matches!(lease.entry().handler.invoke(serde_json::json!({})).await,
                    Err(ToolError::PermissionDenied { reason, .. }) if reason.contains("stale")),
                    "{field}"
                );
                assert_eq!(
                    calls.load(Ordering::SeqCst),
                    before,
                    "{field} executed a stale handler"
                );
                // A retained immutable version is not silently rewritten by stop/replacement.
                assert!(
                    lease.validate(&publication),
                    "{field} mutated a held publication"
                );
            }
        }

        // Value: proves the output fence, not merely that a pre-call guard ran.
        #[tokio::test]
        async fn replacement_discards_output_from_an_already_admitted_handler() {
            let calls = Arc::new(AtomicUsize::new(0));
            let mut entry = mounted_entry(1, Arc::clone(&calls));
            let entered = Arc::new(Notify::new());
            let release = Arc::new(Notify::new());
            let raw: Arc<dyn ToolHandler> = Arc::new(Probe {
                epoch: 1,
                calls: Arc::clone(&calls),
                pause: Some((Arc::clone(&entered), Arc::clone(&release))),
            });
            let descriptor = ToolCapabilityDescriptor::from_definition(&raw.definition(), 1);
            let ready = entry.ready.take().unwrap();
            entry = RegistryEntry::admitted(raw, descriptor, None);
            entry.ready = Some(ready);
            Arc::make_mut(entry.ready.as_mut().unwrap()).handler = Arc::clone(&entry.handler);
            let registry = registry_at(entry);
            let pending = tokio::spawn(call_through_service(registry.clone()));
            tokio::time::timeout(std::time::Duration::from_secs(10), entered.notified())
                .await
                .expect("real scoped dispatch reaches the original handler");
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            install_for_test(&registry, mounted_entry(2, Arc::clone(&calls)));
            release.notify_one();
            assert!(
                matches!(tokio::time::timeout(std::time::Duration::from_secs(10), pending).await.unwrap().unwrap(),
                Err(ToolError::PermissionDenied { reason, .. }) if reason.contains("stale"))
            );
            assert_eq!(
                calls.load(Ordering::SeqCst),
                1,
                "no replay or replacement invocation"
            );
        }

        // Value: readers use the actual publication boundary during concurrent
        // replacement, not a test-only reconstruction from several snapshots.
        #[test]
        fn publication_keeps_ready_binding_descriptor_and_revision_in_one_version() {
            let calls = Arc::new(AtomicUsize::new(0));
            let first = mounted_entry(1, Arc::clone(&calls));
            let second = mounted_entry(2, calls);
            let registry = registry_at(first.clone());
            let frozen = registry.canonical_publication();
            let old = frozen.capture_entry("b1_probe").unwrap();
            let barrier = Arc::new(std::sync::Barrier::new(2));
            let writer = {
                let registry = registry.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    for revision in 2..=64 {
                        let template = if revision % 2 == 0 { &second } else { &first };
                        let raw: Arc<dyn ToolHandler> = Arc::new(Probe {
                            epoch: revision,
                            calls: Arc::new(AtomicUsize::new(0)),
                            pause: None,
                        });
                        let descriptor =
                            ToolCapabilityDescriptor::from_definition(&raw.definition(), revision);
                        let mut entry = RegistryEntry::admitted(raw, descriptor, Some(123));
                        let mut ready = template.ready.as_ref().unwrap().as_ref().clone();
                        ready.provider_generation = revision + 6;
                        ready.provider = ready_provider(revision + 6);
                        ready.owner_generation = revision + 12;
                        ready.entry_revision = revision;
                        ready.canonical_revision = revision;
                        ready.descriptor_fingerprint = descriptor_fingerprint(&entry.descriptor);
                        ready.handler = Arc::clone(&entry.handler);
                        entry.ready = Some(Arc::new(ready));
                        install_for_test(&registry, entry);
                    }
                })
            };
            barrier.wait();
            for _ in 0..128 {
                let publication = registry.canonical_publication();
                let lease = publication.capture_entry("b1_probe").unwrap();
                let ready = publication.ready_snapshot("b1_probe").unwrap();
                assert!(lease.validate(&publication));
                assert_eq!(ready.entry_revision, publication.revision());
                assert_eq!(ready.provider_generation, publication.revision() + 6);
                assert_eq!(ready.owner_generation, publication.revision() + 12);
                assert!(ready.binding.matches_mount(&ready.mount));
            }
            writer.join().unwrap();
            assert_eq!(frozen.revision(), 1);
            assert!(old.validate(&frozen));
            assert!(!old.validate(&registry.canonical_publication()));
            assert_eq!(registry.revision(), 64);
        }
    }

    mod b2 {
        use super::*;
        use crate::extension::runtime::wasm::tests::guest_fixture::{
            manifest_with_guest, HANDLER, PAYLOAD_A, PAYLOAD_B,
        };
        use crate::extension::runtime::wasm::{WasmMountIdentity, WasmRuntime, WasmToolInput};

        // Test-only binding handler, not production activation authority. Calls
        // use a real mounted guest through a captured binding and normal Scoped
        // dispatch admission; B3 must supply its owner-fenced production handler.
        struct GuestHandler {
            runtime: Arc<WasmRuntime>,
            binding: WasmBinding,
            definition: ToolDefinition,
        }

        #[async_trait]
        impl ToolHandler for GuestHandler {
            async fn invoke(&self, arguments: Value) -> Result<ToolOutput, ToolError> {
                let output = self
                    .runtime
                    .call_bound(
                        &self.binding,
                        WasmToolInput {
                            name: self.definition.name.clone(),
                            arguments,
                        },
                    )
                    .map_err(|error| ToolError::Execution {
                        name: self.definition.name.clone(),
                        cause: error.to_string(),
                    })?;
                assert!(output.success);
                Ok(ToolOutput {
                    value: output.result.unwrap(),
                    metadata: Default::default(),
                })
            }

            fn definition(&self) -> ToolDefinition {
                self.definition.clone()
            }
        }

        struct Fixture {
            runtime: Arc<WasmRuntime>,
            provider: ProviderHandle,
            mount: WasmMountIdentity,
        }

        fn provider(state: ProviderState) -> ProviderHandle {
            let mut lifecycle = ProviderLifecycle::new(
                ProviderId::new("plugin/b2-plugin").unwrap(),
                ProviderTransport::InProcess,
            );
            lifecycle.transition(ProviderState::Starting).unwrap();
            if state == ProviderState::Ready {
                lifecycle.transition(state).unwrap();
            }
            ProviderHandle::new(lifecycle)
        }

        impl Fixture {
            fn new(payload: &str) -> Self {
                let directory = tempfile::TempDir::new().unwrap();
                let manifest = manifest_with_guest(directory.path(), "b2-plugin", payload);
                let mut runtime = WasmRuntime::new();
                runtime.load_plugin(&manifest).unwrap();
                let mount = runtime.mounted_instance("b2-plugin").unwrap();
                Self {
                    runtime: Arc::new(runtime),
                    provider: provider(ProviderState::Ready),
                    mount,
                }
            }

            fn batch(&self, generation: u64, names: &[&str]) -> PluginBatch {
                let entries = names
                    .iter()
                    .map(|name| {
                        let binding = self.runtime.bind("b2-plugin", HANDLER).unwrap();
                        let handler: Arc<dyn ToolHandler> = Arc::new(GuestHandler {
                            runtime: Arc::clone(&self.runtime),
                            binding: binding.clone(),
                            definition: ToolDefinition {
                                name: (*name).into(),
                                description: "real bound guest".into(),
                                input_schema: serde_json::json!({"type":"object"}),
                                source: ToolSource::Extension {
                                    plugin_id: "b2-plugin".into(),
                                },
                                metadata: Default::default(),
                            },
                        });
                        PluginBatchEntry {
                            descriptor: ToolCapabilityDescriptor::from_definition(
                                &handler.definition(),
                                0,
                            ),
                            handler,
                            binding,
                            max_result_tokens: Some(321),
                        }
                    })
                    .collect();
                PluginBatch {
                    plugin_id: "b2-plugin".into(),
                    provider: self.provider.clone(),
                    provider_generation: 1,
                    owner_generation: generation,
                    mount: self.mount.clone(),
                    entries,
                }
            }
        }

        fn no_event(events: &mut broadcast::Receiver<RegistryPublication>) {
            assert!(matches!(
                events.try_recv(),
                Err(broadcast::error::TryRecvError::Empty)
            ));
        }

        // Complete batch, frozen reader, observer revision AND actual guest
        // output reach the canonical read/Scoped dispatch boundary together.
        #[tokio::test]
        async fn full_batch_one_publication_and_exact_revoke_preserve_mixed_semantics() {
            let fixture = Fixture::new(PAYLOAD_A);
            let registry = ToolHandlerRegistry::new();
            let catalog = seed_catalog(&registry);
            let mut legacy = Vec::new();
            for (name, source) in [
                ("builtin", ToolSource::Builtin),
                (
                    "mcp",
                    ToolSource::Mcp {
                        server_id: "mixed".into(),
                    },
                ),
                (
                    "static_plugin",
                    ToolSource::Extension {
                        plugin_id: "static".into(),
                    },
                ),
            ] {
                let handler: Arc<dyn ToolHandler> = Arc::new(FakeHandler {
                    name: name.into(),
                    source,
                    idempotent: false,
                });
                legacy.push(
                    registry
                        .register(
                            ToolCapabilityDescriptor::from_definition(&handler.definition(), 1),
                            handler,
                        )
                        .unwrap(),
                );
            }
            let frozen = registry.canonical_publication();
            let before = frozen.revision();
            let mut events = registry.subscribe_publications();
            let receipt = registry
                .publish_plugin_batch(fixture.batch(1, &["first", "second"]), None)
                .unwrap();
            let published = registry.canonical_publication();
            assert_eq!(published.revision(), before + 1);
            assert_eq!(published.entries().len(), 5);
            assert_eq!(frozen.entries().len(), 3);
            let event = events.try_recv().unwrap();
            assert_eq!(event.revision, published.revision());
            assert_eq!(event.tool_changes.len(), 2);
            no_event(&mut events);
            for name in ["first", "second"] {
                let ready = published.ready_snapshot(name).unwrap();
                assert!(ready.mount.same_instance(&fixture.mount));
                assert!(ready.binding.matches_mount(&fixture.mount));
                assert!(ready.provider.same_instance(&fixture.provider));
                assert_eq!(ready.entry_revision, published.revision());
                assert_eq!(ready.canonical_revision, published.revision());
                assert_eq!(published.entry(name).unwrap().max_result_tokens, Some(321));
                assert!(registry.capture_entry(name).unwrap().validate(&published));
                assert_eq!(
                    b1::call_through_service_named(registry.clone(), name)
                        .await
                        .unwrap()
                        .value,
                    Value::String("A".into())
                );
            }
            let old = registry.capture_entry("first").unwrap();
            fixture
                .provider
                .transition(ProviderState::Stopping)
                .unwrap();
            assert_eq!(receipt.revoke().unwrap(), before + 2);
            assert_eq!(events.try_recv().unwrap().tool_changes.len(), 2);
            no_event(&mut events);
            assert!(registry.capture_entry("first").is_none());
            assert!(registry.capture_entry("second").is_none());
            assert!(!old.validate(&registry.canonical_publication()));
            assert!(old
                .entry()
                .handler
                .invoke(serde_json::json!({}))
                .await
                .is_err());
            assert_eq!(
                published.entries().len(),
                5,
                "held snapshot remains immutable"
            );
            assert!(matches!(
                receipt.revoke(),
                Err(PluginPublicationError::Stale { .. })
            ));
            assert_eq!(registry.revision(), before + 2);
            no_event(&mut events);
            assert!(registry.plugin_declarations().contains_key(&catalog));
            for name in ["builtin", "mcp", "static_plugin"] {
                assert!(registry
                    .canonical_publication()
                    .ready_snapshot(name)
                    .is_none());
                let value = b1::call_through_service_named(registry.clone(), name)
                    .await
                    .unwrap()
                    .value;
                if name == "mcp" {
                    // MCP output keeps its existing untrusted-content boundary.
                    let lines: Vec<_> = value.as_str().unwrap().lines().collect();
                    assert_eq!(lines.len(), 3);
                    let (id, source) = lines[0]
                        .strip_prefix("<<<EXTERNAL_UNTRUSTED_CONTENT id=")
                        .unwrap()
                        .split_once(" source=")
                        .unwrap();
                    assert_eq!(source, "\"mcp_tool server=\"mixed\" tool=\"mcp\"\">");
                    assert_eq!(
                        serde_json::from_str::<serde_json::Value>(lines[1]).unwrap(),
                        serde_json::json!({"tool": name})
                    );
                    assert_eq!(
                        lines[2],
                        format!("<<<END_EXTERNAL_UNTRUSTED_CONTENT id={id}>")
                    );
                } else {
                    assert_eq!(value, serde_json::json!({"tool": name}));
                }
            }
            assert!(legacy.iter().all(RegistrationHandle::dispose));
        }

        // Every invalid second entry / identity rejects the entire replacement,
        // preserving even the Arc identity of the current publication.
        #[tokio::test]
        async fn invalid_batch_preflight_never_changes_old_snapshot_revision_or_events() {
            let first = Fixture::new(PAYLOAD_A);
            let next = Fixture::new(PAYLOAD_B);
            let registry = ToolHandlerRegistry::new();
            let receipt = registry
                .publish_plugin_batch(first.batch(1, &["first", "second"]), None)
                .unwrap();
            let old = registry.capture_entry("first").unwrap();
            let frozen = registry.shared.snapshot();
            let mut events = registry.subscribe_publications();
            let mut projected = registry.subscribe();
            for case in [
                "duplicate",
                "definition",
                "revision",
                "source",
                "binding",
                "empty",
                "owner_zero",
                "owner_stale",
                "generation_zero",
                "generation_mismatch",
                "provider_id",
                "not_ready",
            ] {
                let mut batch = next.batch(2, &["first", "second"]);
                match case {
                    "duplicate" => {
                        batch.entries[1].descriptor = batch.entries[0].descriptor.clone()
                    }
                    "definition" => batch.entries[1].descriptor.description = "different".into(),
                    "revision" => batch.entries[1].descriptor.revision = 777,
                    "source" => batch.plugin_id = "other".into(),
                    "binding" => {
                        batch.entries[1].binding = first.runtime.bind("b2-plugin", HANDLER).unwrap()
                    }
                    "empty" => batch.entries.clear(),
                    "owner_zero" => batch.owner_generation = 0,
                    "owner_stale" => batch.owner_generation = 1,
                    "generation_zero" => batch.provider_generation = 0,
                    "generation_mismatch" => batch.provider_generation = 2,
                    "provider_id" => batch.provider = registry.provider(),
                    "not_ready" => batch.provider = provider(ProviderState::Starting),
                    _ => unreachable!(),
                }
                assert!(
                    registry
                        .publish_plugin_batch(batch, Some(&receipt))
                        .is_err(),
                    "{case}"
                );
                assert!(Arc::ptr_eq(&frozen, &registry.shared.snapshot()), "{case}");
                assert_eq!(registry.revision(), frozen.revision, "{case}");
                assert!(old.validate(&registry.canonical_publication()), "{case}");
                no_event(&mut events);
                assert!(matches!(
                    projected.try_recv(),
                    Err(broadcast::error::TryRecvError::Empty)
                ));
            }
            // A matching repeated name is checked independently of definitions.
            assert!(matches!(
                registry.publish_plugin_batch(next.batch(2, &["same", "same"]), Some(&receipt)),
                Err(PluginPublicationError::Duplicate { .. })
            ));
            assert!(Arc::ptr_eq(&frozen, &registry.shared.snapshot()));
            assert_eq!(
                b1::call_through_service_named(registry, "first")
                    .await
                    .unwrap()
                    .value,
                Value::String("A".into())
            );
        }

        #[tokio::test]
        async fn same_name_replacement_rejects_old_handles_receipts_and_cached_guest() {
            let first = Fixture::new(PAYLOAD_A);
            let next = Fixture::new(PAYLOAD_B);
            let registry = ToolHandlerRegistry::new();
            let receipt = registry
                .publish_plugin_batch(first.batch(1, &["same", "old_only"]), None)
                .unwrap();
            let handles = receipt
                .entries
                .iter()
                .map(|(handle, _)| handle.clone())
                .collect::<Vec<_>>();
            let lease = registry.capture_entry("same").unwrap();
            let mut events = registry.subscribe_publications();
            let new = registry
                .publish_plugin_batch(next.batch(2, &["same", "new_only"]), Some(&receipt))
                .unwrap();
            let event = events.try_recv().unwrap();
            assert_eq!(event.revision, 2);
            assert_eq!(event.tool_changes.len(), 4);
            no_event(&mut events);
            assert!(registry.capture_entry("old_only").is_none());
            assert!(registry.capture_entry("new_only").is_some());
            assert!(!lease.validate(&registry.canonical_publication()));
            assert!(
                matches!(lease.entry().handler.invoke(serde_json::json!({})).await,
                Err(ToolError::PermissionDenied { reason, .. }) if reason.contains("stale"))
            );
            let frozen = registry.shared.snapshot();
            assert!(handles.iter().all(|handle| !handle.dispose()));
            assert!(matches!(
                receipt.revoke(),
                Err(PluginPublicationError::Stale { .. })
            ));
            assert!(registry
                .publish_plugin_batch(first.batch(3, &["same"]), Some(&receipt))
                .is_err());
            assert!(Arc::ptr_eq(&frozen, &registry.shared.snapshot()));
            no_event(&mut events);
            assert_eq!(
                b1::call_through_service_named(registry.clone(), "same")
                    .await
                    .unwrap()
                    .value,
                Value::String("B".into())
            );
            // Name-only legacy APIs cannot split or replace a canonical batch.
            assert!(registry.unregister("same").is_none());
            let input = next.batch(3, &["same"]).entries.remove(0);
            assert!(registry.replace(input.descriptor, input.handler).is_err());
            let input = next.batch(3, &["extra"]).entries.remove(0);
            assert!(registry.register(input.descriptor, input.handler).is_err());
            assert!(Arc::ptr_eq(&frozen, &registry.shared.snapshot()));
            assert_eq!(new.revoke().unwrap(), 3);
            assert!(registry.canonical_publication().entries().is_empty());
        }

        #[test]
        fn canonical_plugin_lease_validation_rejects_withdrawal_from_one_publication() {
            let fixture = Fixture::new(PAYLOAD_A);
            let registry = ToolHandlerRegistry::new();
            let receipt = registry
                .publish_plugin_batch(fixture.batch(1, &["withdrawn"]), None)
                .unwrap();
            let lease = registry.capture_entry("withdrawn").unwrap();
            let published = registry.canonical_publication();
            assert!(lease.validate(&published));

            fixture
                .provider
                .transition(ProviderState::Stopping)
                .unwrap();
            receipt.revoke().unwrap();
            let withdrawn = registry.canonical_publication();
            assert!(withdrawn.entry("withdrawn").is_none());
            assert!(!lease.validate(&withdrawn));
        }

        #[test]
        fn conflict_closed_and_cross_registry_receipt_are_zero_commit() {
            let fixture = Fixture::new(PAYLOAD_A);
            let registry = ToolHandlerRegistry::new();
            let handler: Arc<dyn ToolHandler> = Arc::new(FakeHandler {
                name: "occupied".into(),
                source: ToolSource::Builtin,
                idempotent: false,
            });
            let handle = registry
                .register(
                    ToolCapabilityDescriptor::from_definition(&handler.definition(), 1),
                    handler,
                )
                .unwrap();
            let frozen = registry.shared.snapshot();
            let mut events = registry.subscribe_publications();
            assert!(matches!(
                registry.publish_plugin_batch(fixture.batch(1, &["free", "occupied"]), None),
                Err(PluginPublicationError::Duplicate { .. })
            ));
            assert!(Arc::ptr_eq(&frozen, &registry.shared.snapshot()));
            no_event(&mut events);
            assert!(handle.dispose());
            let receipt = registry
                .publish_plugin_batch(fixture.batch(1, &["first", "second"]), None)
                .unwrap();
            let other = ToolHandlerRegistry::new();
            other
                .publish_plugin_batch(fixture.batch(1, &["first", "second"]), None)
                .unwrap();
            let cut = other.shared.snapshot();
            assert!(matches!(
                other.publish_plugin_batch(fixture.batch(2, &["first", "second"]), Some(&receipt)),
                Err(PluginPublicationError::Stale { .. })
            ));
            assert!(Arc::ptr_eq(&cut, &other.shared.snapshot()));
            registry.close();
            let cut = registry.shared.snapshot();
            let mut events = registry.subscribe_publications();
            assert!(matches!(
                registry
                    .publish_plugin_batch(fixture.batch(2, &["first", "second"]), Some(&receipt)),
                Err(PluginPublicationError::Closed)
            ));
            assert!(Arc::ptr_eq(&cut, &registry.shared.snapshot()));
            no_event(&mut events);
            assert!(registry.canonical_publication().entries().is_empty());
            assert!(
                receipt.revoke().is_ok(),
                "exact cleanup is allowed after registry close"
            );
        }

        #[tokio::test]
        async fn b3_guarded_ready_commit_is_atomic_and_failure_keeps_starting() {
            let fixture = Fixture::new(PAYLOAD_A);
            for failure in [
                "duplicate",
                "provider",
                "generation",
                "closed",
                "conflict",
                "success",
            ] {
                let registry = ToolHandlerRegistry::new();
                let provider = ProviderHandle::new(ProviderLifecycle::new(
                    ProviderId::new(if failure == "provider" {
                        "plugin/other"
                    } else {
                        "plugin/b2-plugin"
                    })
                    .unwrap(),
                    ProviderTransport::InProcess,
                ));
                provider.transition(ProviderState::Starting).unwrap();
                let mut transitions = provider.subscribe_changes();
                let mut batch = fixture.batch(
                    1,
                    if failure == "duplicate" {
                        &["first", "first"]
                    } else {
                        &["first", "second"]
                    },
                );
                batch.provider = provider.clone();
                if failure == "generation" {
                    batch.provider_generation += 1;
                }
                let _conflict = if failure == "conflict" {
                    let handler: Arc<dyn ToolHandler> = Arc::new(FakeHandler {
                        name: "second".into(),
                        source: ToolSource::Builtin,
                        idempotent: false,
                    });
                    Some(
                        registry
                            .register(
                                ToolCapabilityDescriptor::from_definition(&handler.definition(), 0),
                                handler,
                            )
                            .unwrap(),
                    )
                } else {
                    None
                };
                if failure == "closed" {
                    registry.close();
                }
                let before = registry.shared.snapshot();
                let mut events = registry.subscribe_publications();
                let result = registry.publish_plugin_batch_with_ready_commit(batch);
                if failure != "success" {
                    let error = result.expect_err("must reject entire batch");
                    match failure {
                        "duplicate" | "conflict" => {
                            assert!(matches!(error, PluginPublicationError::Duplicate { .. }))
                        }
                        "provider" => {
                            assert!(matches!(error, PluginPublicationError::InvalidIdentity(_)))
                        }
                        "generation" => {
                            assert!(matches!(error, PluginPublicationError::Provider(_)))
                        }
                        "closed" => assert!(matches!(error, PluginPublicationError::Closed)),
                        _ => unreachable!(),
                    }
                    assert_eq!(provider.snapshot(), (ProviderState::Starting, 1));
                    assert!(Arc::ptr_eq(&before, &registry.shared.snapshot()));
                    no_event(&mut events);
                    assert!(matches!(
                        transitions.try_recv(),
                        Err(broadcast::error::TryRecvError::Empty)
                    ));
                } else {
                    let receipt = result.unwrap();
                    assert_eq!(provider.snapshot(), (ProviderState::Ready, 1));
                    let transition = transitions.try_recv().unwrap();
                    assert_eq!(
                        (transition.from, transition.to),
                        (ProviderState::Starting, ProviderState::Ready)
                    );
                    assert!(matches!(
                        transitions.try_recv(),
                        Err(broadcast::error::TryRecvError::Empty)
                    ));
                    let publication = registry.canonical_publication();
                    assert_eq!(publication.revision(), before.revision + 1);
                    assert_eq!(publication.entries().len(), 2);
                    assert_eq!(events.try_recv().unwrap().tool_changes.len(), 2);
                    no_event(&mut events);
                    assert!(publication
                        .ready_snapshot("first")
                        .unwrap()
                        .provider
                        .same_instance(&provider));
                    assert_eq!(
                        b1::call_through_service_named(registry.clone(), "first")
                            .await
                            .unwrap()
                            .value,
                        Value::String("A".into())
                    );
                    assert!(registry.reconcile_plugin_tools(Default::default()).is_ok());
                    receipt.revoke().unwrap();
                }
            }
        }

        #[test]
        fn b3_guarded_ready_reader_waits_for_complete_provider_registry_commit() {
            let fixture = Fixture::new(PAYLOAD_A);
            let registry = ToolHandlerRegistry::new();
            let provider = ProviderHandle::new(ProviderLifecycle::new(
                ProviderId::new("plugin/b2-plugin").unwrap(),
                ProviderTransport::InProcess,
            ));
            provider.transition(ProviderState::Starting).unwrap();
            let mut batch = fixture.batch(1, &["first", "second"]);
            batch.provider = provider.clone();
            let names = ToolHandlerRegistry::preflight_plugin_batch(&batch).unwrap();
            let mut transitions = provider.subscribe_changes();
            let (committed, at_commit) = std::sync::mpsc::channel();
            let (release, released) = std::sync::mpsc::channel();
            let (reading, at_read) = std::sync::mpsc::channel();
            let (observed, observation) = std::sync::mpsc::channel();
            std::thread::scope(|scope| {
                let registry = &registry;
                let provider = &provider;
                let batch = &batch;
                let writer = scope.spawn(move || {
                    let result: Result<_, PluginPublicationError> =
                        provider.with_ready_commit(1, |prepared| {
                            let receipt =
                                registry.commit_plugin_batch(batch, None, names, prepared)?;
                            // Deliberately park after the registry transaction but
                            // before the infallible Ready assignment, under guard.
                            committed.send(()).unwrap();
                            released.recv().unwrap();
                            Ok(receipt)
                        });
                    result.unwrap()
                });
                at_commit.recv().unwrap();
                let reader = scope.spawn(|| {
                    reading.send(()).unwrap();
                    observed.send(registry.canonical_publication()).unwrap();
                });
                at_read.recv().unwrap();
                assert!(matches!(
                    observation.try_recv(),
                    Err(std::sync::mpsc::TryRecvError::Empty)
                ));
                assert!(matches!(
                    transitions.try_recv(),
                    Err(broadcast::error::TryRecvError::Empty)
                ));
                release.send(()).unwrap();
                let receipt = writer.join().unwrap();
                reader.join().unwrap();
                let cut = observation.recv().unwrap();
                assert_eq!(cut.entries().len(), 2);
                assert_eq!(cut.revision(), 1);
                assert_eq!(provider.snapshot(), (ProviderState::Ready, 1));
                assert_eq!(transitions.try_recv().unwrap().to, ProviderState::Ready);
                assert!(matches!(
                    transitions.try_recv(),
                    Err(broadcast::error::TryRecvError::Empty)
                ));
                receipt.revoke().unwrap();
            });
        }

        #[test]
        fn b3_full_publication_uses_one_verdict_for_the_complete_producer_batch() {
            let fixture = Fixture::new(PAYLOAD_A);
            let registry = ToolHandlerRegistry::new();
            let receipt = registry
                .publish_plugin_batch(fixture.batch(1, &["first", "second"]), None)
                .unwrap();
            let verdicts = std::cell::Cell::new(0);
            let cut = registry.canonical_publication_inner(Some(&|| {
                verdicts.set(verdicts.get() + 1);
                // Stop after the captured verdict, before any entry is projected.
                fixture
                    .provider
                    .transition(ProviderState::Stopping)
                    .unwrap();
            }));
            assert_eq!(verdicts.get(), 1);
            assert_eq!(cut.revision(), 1);
            assert_eq!(
                cut.entries().len(),
                2,
                "one historical producer verdict covers both entries"
            );
            assert!(
                cut.capture_entry("first").is_none(),
                "target capture still has a live fence"
            );
            assert!(cut.capture_entry("second").is_none());
            assert!(registry.canonical_publication().entries().is_empty());
            receipt.revoke().unwrap();
        }

        #[test]
        fn concurrent_publications_never_expose_partial_or_mixed_generation_batches() {
            let first = Fixture::new(PAYLOAD_A);
            let next = Fixture::new(PAYLOAD_B);
            let registry = ToolHandlerRegistry::new();
            let receipt = registry
                .publish_plugin_batch(first.batch(1, &["same", "a_only"]), None)
                .unwrap();
            let frozen = registry.canonical_publication();
            let barrier = Arc::new(std::sync::Barrier::new(2));
            let writer = {
                let registry = registry.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let mut receipt = receipt;
                    barrier.wait();
                    for generation in 2..=32 {
                        let batch = if generation % 2 == 0 {
                            next.batch(generation, &["same", "b_only"])
                        } else {
                            first.batch(generation, &["same", "a_only"])
                        };
                        receipt = registry
                            .publish_plugin_batch(batch, Some(&receipt))
                            .unwrap();
                    }
                })
            };
            barrier.wait();
            for _ in 0..128 {
                let publication = registry.canonical_publication();
                assert_eq!(publication.entries().len(), 2);
                let common = publication.ready_snapshot("same").unwrap();
                let companion = if common.owner_generation.is_multiple_of(2) {
                    "b_only"
                } else {
                    "a_only"
                };
                let paired = publication.ready_snapshot(companion).unwrap();
                assert_eq!(common.owner_generation, publication.revision());
                assert_eq!(common.canonical_revision, paired.canonical_revision);
                assert_eq!(common.entry_revision, publication.revision());
                assert!(common.mount.same_instance(&paired.mount));
                assert!(publication
                    .capture_entry(companion)
                    .unwrap()
                    .validate(&publication));
            }
            writer.join().unwrap();
            assert_eq!(registry.revision(), 32);
            assert_eq!(frozen.revision(), 1);
            assert!(frozen.entry("a_only").is_some());
            assert!(registry.capture_entry("a_only").is_none());
            assert!(registry.capture_entry("b_only").is_some());
        }

        #[tokio::test]
        async fn plugin_provider_stop_and_restart_invalidate_captures_and_zahir_leases() {
            use crate::capability::backend::TOOL_NAMESPACE;
            use crate::capability::descriptor::CapabilityId;
            use crate::capability::facade::{Reference, ResolveError, Scope, Zahir};
            use crate::capability::ownership::OwnershipTree;
            use crate::capability::zahir_facade::ZahirFacade;

            let fixture = Fixture::new(PAYLOAD_A);
            let registry = ToolHandlerRegistry::new();
            for (name, source) in [
                ("builtin", ToolSource::Builtin),
                (
                    "mcp",
                    ToolSource::Mcp {
                        server_id: "mixed".into(),
                    },
                ),
                (
                    "static_plugin",
                    ToolSource::Extension {
                        plugin_id: "static".into(),
                    },
                ),
            ] {
                let handler: Arc<dyn ToolHandler> = Arc::new(FakeHandler {
                    name: name.into(),
                    source,
                    idempotent: false,
                });
                registry
                    .register(
                        ToolCapabilityDescriptor::from_definition(&handler.definition(), 0),
                        handler,
                    )
                    .unwrap();
            }
            let receipt = registry
                .publish_plugin_batch(fixture.batch(1, &["first", "second"]), None)
                .unwrap();
            let frozen = registry.canonical_publication();
            let old = frozen.capture_entry("first").unwrap();
            let cached = registry.capture_entry("first").unwrap();
            let legacy: Vec<_> = ["builtin", "mcp", "static_plugin"]
                .into_iter()
                .map(|name| (name, frozen.capture_entry(name).unwrap()))
                .collect();
            let facade = ZahirFacade::new(registry.clone(), Arc::new(OwnershipTree::new()));
            let reference = Reference {
                id: CapabilityId {
                    namespace: TOOL_NAMESPACE.into(),
                    name: "first".into(),
                },
                visibility: Default::default(),
            };
            let zahir_lease = facade.resolve(reference.clone()).unwrap();
            assert_eq!(facade.validate_lease(&zahir_lease), Ok(()));
            assert!(old.validate(&frozen));
            assert!(old.validate_capture(&cached));
            assert_eq!(fixture.provider.snapshot(), (ProviderState::Ready, 1));
            assert_eq!(
                b1::call_through_service_named(registry.clone(), "first")
                    .await
                    .unwrap()
                    .value,
                Value::String("A".into())
            );
            let cut = registry.shared.snapshot();
            let mut events = registry.subscribe_publications();
            let mut stale_receipt = receipt.clone();
            Arc::make_mut(&mut stale_receipt.entries[1].1).owner_generation += 1;

            for state in [
                ProviderState::Stopping,
                ProviderState::Stopped,
                ProviderState::Starting,
                ProviderState::Ready,
            ] {
                fixture.provider.transition(state).unwrap();
                if state == ProviderState::Stopping {
                    // Withdrawal failure retains records, not callable authority.
                    assert!(matches!(stale_receipt.revoke(),
                        Err(PluginPublicationError::Stale { name }) if name == "second"));
                }
                assert_eq!(registry.provider().snapshot(), (ProviderState::Ready, 1));
                assert!(Arc::ptr_eq(&cut, &registry.shared.snapshot()));
                assert_eq!(registry.revision(), cut.revision);
                no_event(&mut events);
                assert!(registry.capture_entry("first").is_none(), "{state:?}");
                assert!(frozen.capture_entry("first").is_none(), "{state:?}");
                assert!(!old.validate(&frozen), "{state:?}");
                assert!(!old.validate_capture(&cached), "{state:?}");
                let current = registry.canonical_publication();
                assert!(!old.validate(&current), "{state:?}");
                assert!(current.entry("first").is_none());
                assert!(current.entry("second").is_none());
                assert_eq!(current.entries().len(), legacy.len());
                for (name, lease) in &legacy {
                    assert!(lease.validate(&frozen));
                    assert!(lease.validate(&current));
                    assert!(registry.capture_entry(name).is_some());
                }
                assert_eq!(
                    facade.resolve(reference.clone()),
                    Err(ResolveError::Unknown)
                );
                assert_eq!(
                    facade.validate_lease(&zahir_lease),
                    Err(ResolveError::Unknown)
                );
                let projected = facade.describe(Scope::default());
                assert_eq!(projected.capabilities.len(), legacy.len());
                assert!(projected
                    .capabilities
                    .iter()
                    .all(|descriptor| legacy.iter().any(|(name, _)| *name == descriptor.id.name)));
                assert!(old
                    .entry()
                    .handler
                    .invoke(serde_json::json!({}))
                    .await
                    .is_err());
            }
            // Ready at generation 2 cannot resurrect generation 1. Only a new
            // exact batch publication can expose that provider generation.
            assert_eq!(fixture.provider.snapshot(), (ProviderState::Ready, 2));
            let mut replacement = fixture.batch(2, &["first", "second"]);
            replacement.provider_generation = 2;
            registry
                .publish_plugin_batch(replacement, Some(&receipt))
                .unwrap();
            assert!(registry.capture_entry("first").is_some());
            assert!(!old.validate(&registry.canonical_publication()));
            assert_eq!(
                facade.validate_lease(&zahir_lease),
                Err(ResolveError::Unknown)
            );
            let fresh = facade.resolve(reference).unwrap();
            assert_eq!(facade.validate_lease(&fresh), Ok(()));
            assert_eq!(
                b1::call_through_service_named(registry, "first")
                    .await
                    .unwrap()
                    .value,
                Value::String("A".into())
            );
        }

        #[tokio::test]
        async fn partial_stale_disposer_preflight_retains_fail_closed_residual() {
            let fixture = Fixture::new(PAYLOAD_A);
            let registry = ToolHandlerRegistry::new();
            let receipt = registry
                .publish_plugin_batch(fixture.batch(1, &["first", "second"]), None)
                .unwrap();
            let lease = registry.capture_entry("first").unwrap();
            let mut stale = receipt.clone();
            Arc::make_mut(&mut stale.entries[1].1).owner_generation += 1;
            // B3 closes provider admission before withdrawal; a stale second
            // identity must not delete the first entry or reopen this residual.
            fixture
                .provider
                .transition(ProviderState::Stopping)
                .unwrap();
            let frozen = registry.shared.snapshot();
            let mut events = registry.subscribe_publications();
            assert!(
                matches!(stale.revoke(), Err(PluginPublicationError::Stale { name }) if name == "second")
            );
            assert!(Arc::ptr_eq(&frozen, &registry.shared.snapshot()));
            assert_eq!(frozen.value.records.len(), 2);
            no_event(&mut events);
            assert!(lease
                .entry()
                .handler
                .invoke(serde_json::json!({}))
                .await
                .is_err());
            assert_eq!(
                receipt.revoke().unwrap(),
                2,
                "the exact retained receipt can recover atomically"
            );
            assert!(registry.canonical_publication().entries().is_empty());
        }
    }

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
                ready: None,
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

    #[tokio::test]
    async fn provider_fencing_rejects_cached_views_and_preserves_other_providers() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let provider = ProviderHandle::new(ProviderLifecycle::ready(
            ProviderId::new("test/fenced").unwrap(),
            ProviderTransport::InProcess,
        ));
        let other = ProviderHandle::new(ProviderLifecycle::ready(
            ProviderId::new("test/unaffected").unwrap(),
            ProviderTransport::InProcess,
        ));
        let registry = ToolHandlerRegistry::new_with_provider(provider.clone());
        let other_registry = ToolHandlerRegistry::new_with_provider(other);
        let calls = Arc::new(AtomicUsize::new(0));
        let other_calls = Arc::new(AtomicUsize::new(0));
        for (registry, counter, name) in [
            (&registry, &calls, "counted"),
            (&registry, &calls, "removed"),
            (&other_registry, &other_calls, "counted"),
        ] {
            let raw: Arc<dyn ToolHandler> = Arc::new(AdmissionCountingHandler {
                name: name.into(),
                calls: counter.clone(),
            });
            registry
                .register(
                    ToolCapabilityDescriptor::from_definition(&raw.definition(), 0),
                    raw,
                )
                .unwrap();
        }
        let entry = registry.resolve_entry("counted").unwrap();
        let visible: McpServerFilter = Arc::new(|_| true);
        let removed = registry.resolve_entry("removed").unwrap();
        let cached = vec![
            entry.clone(),
            registry.snapshot_state().entry("counted").unwrap().clone(),
            registry.entries_snapshot().get("counted").unwrap().clone(),
            RegistryEntry {
                handler: registry.resolve("counted").unwrap(),
                ..entry.clone()
            },
            RegistryEntry {
                handler: registry.snapshot().get("counted").unwrap().clone(),
                ..entry.clone()
            },
            RegistryEntry {
                handler: entry.handler.bind_visible_servers(&visible).unwrap(),
                ..entry.clone()
            },
            RegistryEntry {
                handler: registry.unregister("removed").unwrap(),
                ..removed
            },
        ];
        for entry in &cached {
            assert_eq!(
                invoke_captured(entry).await.value,
                serde_json::json!({"executed": true})
            );
        }
        let admitted = calls.load(Ordering::SeqCst);
        assert_eq!(admitted, cached.len());
        let unaffected = other_registry.resolve_entry("counted").unwrap();

        // Even a future constructed before stop must check admission when polled.
        let pending = try_invoke_captured(&cached[0]);
        provider.transition(ProviderState::Stopping).unwrap();
        assert_provider_denied(pending.await);
        for state in [
            ProviderState::Stopping,
            ProviderState::Stopped,
            ProviderState::Disabled,
        ] {
            if state != ProviderState::Stopping {
                provider.transition(state).unwrap();
            }
            assert!(registry.resolve_entry("counted").is_none());
            assert!(registry.snapshot_state().entries().is_empty());
            for entry in &cached {
                assert_provider_denied(try_invoke_captured(entry).await);
            }
            assert_eq!(calls.load(Ordering::SeqCst), admitted);
            invoke_captured(&unaffected).await;
        }

        provider.transition(ProviderState::Starting).unwrap();
        assert_provider_denied(try_invoke_captured(&cached[0]).await);
        provider.transition(ProviderState::Ready).unwrap();
        for entry in &cached {
            assert_provider_denied(try_invoke_captured(entry).await);
        }
        assert_eq!(calls.load(Ordering::SeqCst), admitted);
        let fresh = registry.resolve_entry("counted").unwrap();
        invoke_captured(&fresh).await;
        assert_eq!(calls.load(Ordering::SeqCst), admitted + 1);
        provider.transition(ProviderState::Failed).unwrap();
        assert!(registry.resolve_entry("counted").is_none());
        assert!(registry.snapshot_state().entries().is_empty());
        assert_provider_denied(try_invoke_captured(&fresh).await);
        for entry in &cached {
            assert_provider_denied(try_invoke_captured(entry).await);
        }
        assert_eq!(calls.load(Ordering::SeqCst), admitted + 1);
        invoke_captured(&unaffected).await;
        assert_eq!(other_calls.load(Ordering::SeqCst), 4);

        provider.transition(ProviderState::Starting).unwrap();
        provider.transition(ProviderState::Ready).unwrap();
        assert_provider_denied(try_invoke_captured(&fresh).await);
        invoke_captured(&registry.resolve_entry("counted").unwrap()).await;
        assert_eq!(calls.load(Ordering::SeqCst), admitted + 2);
    }

    fn assert_provider_denied(result: Result<ToolOutput, ToolError>) {
        assert!(
            matches!(result, Err(ToolError::PermissionDenied { ref reason, .. })
            if reason.contains("rejects call")),
            "unexpected admission result: {result:?}"
        );
    }

    // This raw fixture deliberately knows nothing about admission proof.
    // Only the canonical registry entry may stop its observable effect.
    struct AdmissionCountingHandler {
        name: String,
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait]
    impl ToolHandler for AdmissionCountingHandler {
        fn bind_visible_servers(&self, _visible: &McpServerFilter) -> Option<Arc<dyn ToolHandler>> {
            Some(Arc::new(Self {
                name: self.name.clone(),
                calls: self.calls.clone(),
            }))
        }

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
        try_invoke_captured(entry)
            .await
            .expect("frozen fixture entry passes real gates")
    }

    async fn try_invoke_captured(entry: &RegistryEntry) -> Result<ToolOutput, ToolError> {
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
        projection.bind_canonical_registry(Arc::clone(&frozen), Arc::new(|_| true));
        let facade = crate::capability::zahir_facade::ZahirFacade::new(
            frozen.as_ref().clone(),
            Arc::new(crate::capability::ownership::OwnershipTree::new()),
        );
        assert!(projection.bind_canonical_facade(facade));
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
        .await?;
        output.value =
            serde_json::from_str(output.value.as_str().expect("Layer 2 rendered JSON")).unwrap();
        Ok(output)
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
            assert_eq!(captured.handler.definition(), first_published.definition());
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
        assert_eq!(
            current.handler.definition(),
            reg.resolve("terminal").unwrap().definition()
        );
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
    fn single_entry_capture_does_not_construct_unrelated_callable_views() {
        use crate::capability::descriptor::CapabilityId;
        use crate::capability::facade::{Reference, Zahir};
        use crate::capability::ownership::{OwnershipTree, VisibilityScope};
        use crate::capability::zahir_facade::ZahirFacade;

        let registry = ToolHandlerRegistry::new();
        for index in 0..32 {
            let name = format!("tool_{index}");
            registry.register(desc(&name), fake(&name)).unwrap();
        }
        PROVIDER_VIEW_CONSTRUCTIONS.with(|count| count.set(0));
        let target = registry.capture_entry("tool_7").unwrap();
        assert_eq!(target.revision(), 32);
        assert_eq!(target.entry().descriptor.revision, 8);
        assert_eq!(target.entry().descriptor.name, "tool_7");
        assert_eq!(
            PROVIDER_VIEW_CONSTRUCTIONS.with(|count| count.replace(0)),
            1,
            "target capture constructs only the requested callable view"
        );
        assert!(registry.capture_entry("missing").is_none());
        assert_eq!(
            PROVIDER_VIEW_CONSTRUCTIONS.with(|count| count.replace(0)),
            0
        );
        assert!(registry.resolve("tool_7").is_some());
        assert!(registry.descriptor("tool_7").is_some());
        assert_eq!(
            PROVIDER_VIEW_CONSTRUCTIONS.with(|count| count.replace(0)),
            2
        );

        let facade = ZahirFacade::new(registry.clone(), Arc::new(OwnershipTree::new()));
        let lease = facade
            .resolve(Reference {
                id: CapabilityId {
                    namespace: crate::capability::backend::TOOL_NAMESPACE.into(),
                    name: "tool_7".into(),
                },
                visibility: VisibilityScope::default(),
            })
            .unwrap();
        assert_eq!(
            PROVIDER_VIEW_CONSTRUCTIONS.with(|count| count.replace(0)),
            1,
            "single resolve must not reconcile a full callable projection"
        );
        facade.validate_lease(&lease).unwrap();
        assert_eq!(
            PROVIDER_VIEW_CONSTRUCTIONS.with(|count| count.replace(0)),
            1
        );

        let publication = registry.canonical_publication();
        assert_eq!(publication.entries().len(), 32);
        assert!(target.validate(&publication));
        assert_eq!(
            PROVIDER_VIEW_CONSTRUCTIONS.with(|count| count.replace(0)),
            32,
            "full projection intentionally constructs all callable views"
        );
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

    #[tokio::test]
    async fn snapshot_projection_preserves_admitted_members_and_frozen_identity() {
        let reg = ToolHandlerRegistry::new();
        reg.register(desc("stable"), fake("stable")).unwrap();
        let before = reg.snapshot_state();
        let repeated = reg.snapshot_state();
        assert_eq!(before.revision(), repeated.revision());
        assert_eq!(before.entries().len(), reg.entries_snapshot().len());
        let admitted = reg.resolve_entry("stable").unwrap();
        let stable = before.entry("stable").unwrap();
        assert_eq!(
            invoke_captured(stable).await.value,
            invoke_captured(&admitted).await.value
        );
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
        assert_eq!(
            invoke_captured(stable).await.value,
            invoke_captured(retained).await.value
        );
        assert!(Arc::ptr_eq(&stable.descriptor, &retained.descriptor));
        assert_eq!(after.revision(), reg.snapshot_state().revision());
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
        assert_eq!(stored.handler.definition(), entry.handler.definition());
        assert!(Arc::ptr_eq(&stored.descriptor, &entry.descriptor));
        assert_eq!(after.entries().len(), 1);
        assert_eq!(entry.descriptor.revision, current.revision());
        assert_eq!(
            before.entry("provider").unwrap().descriptor.source,
            ToolSource::Builtin
        );
        assert!(!stale.dispose());
        assert_eq!(after.revision(), reg.snapshot_state().revision());
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
            &frozen.entry("duplicate").unwrap().descriptor,
            &after.value.entry("duplicate").unwrap().descriptor
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
        let removed_entry = RegistryEntry {
            handler: removed,
            ..replaced.entry("terminal").unwrap().clone()
        };
        assert_eq!(
            invoke_captured(&removed_entry).await.value,
            serde_json::json!({"generation": 2})
        );
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
        assert_eq!(entry.handler.definition(), resolved.handler.definition());
        assert!(Arc::ptr_eq(&entry.descriptor, &resolved.descriptor));
        assert_eq!(state.entries().len(), reg.entries_snapshot().len());
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
        assert_eq!(frozen_entry.handler.definition(), old.handler.definition());
        assert!(Arc::ptr_eq(&frozen_entry.descriptor, &old.descriptor));
        assert_eq!(current.max_result_tokens, Some(91));
        assert_eq!(current.handler.definition(), live_handler.definition());
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
