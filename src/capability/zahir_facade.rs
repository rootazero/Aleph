//! The one live [`Zahir`] host.
//!
//! [`ZahirFacade`] composes the existing [`ToolBackendAdapter`] (whose inner
//! [`crate::tools::registry::ToolHandlerRegistry`] is the single callable-Tool
//! source of truth) with an [`Arc`]`<`[`OwnershipTree`]`>` (the single owner /
//! revoke / dispose source). It holds NO second descriptor map, handler map, or
//! owner counter: every read goes through one of those two sources.
//!
//! The registry broadcast is a live *notification* tap (256 slots, lag can
//! drop), never a durable recovery source. `subscribe` drains it synchronously
//! into a pure [`CapabilityChangeStream`]; a lagged receiver resyncs from a
//! fresh snapshot and emits [`CapabilityChange::Invalidated`] rather than
//! pretending it delivered every event.

use crate::capability::backend::{CapabilityBackend, ToolBackendAdapter, TOOL_NAMESPACE};
use crate::capability::descriptor::{CapabilityId, CapabilityRevision, ProjectionMetadata};
use crate::capability::facade::{
    BackendLease, CapabilityChange, CapabilityChangeStream, CapabilitySnapshot, Cursor, Projection,
    Reference, ResolveError, Scope, TransportTarget, Zahir,
};
use crate::capability::ownership::{
    LifetimeScope, OwnerGeneration, OwnerRef, OwnershipTree, VisibilityScope,
};
use crate::sync_primitives::Arc;
use crate::tools::registry::{RegistryChange, RegistryEntry, RegistrySnapshot, ToolHandlerRegistry};
use std::collections::HashMap;
use tokio::sync::broadcast::{self, error::TryRecvError};

/// Fold a bare tool name into the tool-namespaced [`CapabilityId`].
fn tool_id(name: &str) -> CapabilityId {
    CapabilityId {
        namespace: TOOL_NAMESPACE.to_string(),
        name: name.to_string(),
    }
}

/// One projection of the live registry entries, owner generations, and
/// registry state. It is TWO internally coherent cuts, NOT one atomic
/// generation:
///
/// * the **registry cut** (`entries` + `registry_cursor` + `registry_closed`)
///   comes from a SINGLE [`RegistrySnapshot`] load, so those three can never
///   disagree about the registry generation;
/// * the **owner cut** (`owner_generations`) comes from a SINGLE
///   [`OwnershipTree`] mutex acquisition, so it can never mix owner
///   generations across ids.
///
/// The two cuts are independent authority domains: a registry mutation landing
/// between the registry load and the owner read is NOT excluded. Callers that
/// need cross-authority ordering must use the subscribe-before-snapshot path,
/// not this type.
///
/// `entries` and `owner_generations` share identical id membership: an entry is
/// exposed iff its id still has a live ownership binding under `scope`'s
/// visibility (a revoked/disposed id has no binding and is therefore absent from
/// both).
#[derive(Clone)]
pub(crate) struct FacadeEntriesSnapshot {
    /// Registry entries in scope, keyed by tool name, SHARED behind an [`Arc`]
    /// so cloning a snapshot is cheap. Each entry's `handler` and `descriptor`
    /// `Arc`s are frozen at the registry snapshot generation.
    pub(crate) entries: Arc<HashMap<String, RegistryEntry>>,
    /// The registry revision the `entries` were frozen from (registry cut).
    pub(crate) registry_cursor: Cursor,
    /// The registry close flag captured from the same frozen state (registry cut).
    pub(crate) registry_closed: bool,
    /// Owner generation for every id in `entries` (identical membership), read
    /// under ONE mutex so a concurrent `bump` cannot mix generations (owner
    /// cut). Per-binding generations, NOT a registry revision.
    pub(crate) owner_generations: HashMap<CapabilityId, OwnerGeneration>,
}

/// The one live [`Zahir`] implementation.
pub struct ZahirFacade {
    adapter: ToolBackendAdapter,
    tree: Arc<OwnershipTree>,
}

impl ZahirFacade {
    /// No startup scan: bindings are registered lazily by
    /// `reconcile_registry_bindings()` on the first facade call, so a tool
    /// registered after construction is visible on the next call.
    #[must_use]
    pub fn new(registry: ToolHandlerRegistry, tree: Arc<OwnershipTree>) -> Arc<Self> {
        Arc::new(Self {
            adapter: ToolBackendAdapter { registry },
            tree,
        })
    }

    /// Idempotently register an ownership binding for every registry tool that
    /// has none (and is not revoked/disposed). Called at the top of
    /// `describe`/`resolve` (`subscribe`/`project` reach it via `describe`).
    ///
    /// Registration is atomic insert-if-absent: the observe and the insert
    /// happen under one lock, so a binding or claim minted by another caller
    /// between observe and insert is preserved (its owner, generation, and
    /// active claims survive). `unregister` does NOT revoke a binding, so a
    /// later re-register of the same name resolves again. A revoked/disposed
    /// id makes `tree.register_if_absent` return `Err` — treated as a no-op.
    pub fn reconcile_registry_bindings(&self) {
        let snap = self.adapter.registry.snapshot_state();
        self.reconcile_snapshot(&snap);
    }

    /// Reconcile ownership bindings from ONE frozen registry snapshot's
    /// entries. Shared by `reconcile_registry_bindings` and
    /// `snapshot_entries_in_scope` so both reconcile from the SAME frozen
    /// entries rather than two independent loads.
    fn reconcile_snapshot(&self, snap: &RegistrySnapshot) {
        for name in snap.entries().keys() {
            let id = tool_id(name);
            let visibility = VisibilityScope::default();
            let _ = self
                .tree
                .register_if_absent(id, OwnerRef::Runtime, LifetimeScope::Runtime, visibility);
        }
    }

    /// Project the live registry entries, owner generations, and registry state
    /// into one [`FacadeEntriesSnapshot`], reconciling ownership bindings from
    /// the SAME frozen entries.
    ///
    /// Exactly ONE [`RegistrySnapshot`] is loaded (yielding the internally
    /// coherent registry cut: `entries` + `registry_cursor` + `registry_closed`)
    /// and exactly ONE bulk owner observation is taken under ONE mutex (yielding
    /// the internally coherent owner cut: `owner_generations`). The two cuts are
    /// separate authority domains and are NOT jointly atomic; see
    /// [`FacadeEntriesSnapshot`].
    ///
    /// `entries` membership equals `owner_generations` membership: an entry is
    /// exposed iff its id has a live ownership binding under `scope.visibility`
    /// (revoked/disposed ids are therefore absent). The kind/namespace filter is
    /// the SAME [`ToolBackendAdapter::in_scope`] predicate the backend uses — no
    /// second predicate, no authority counter.
    #[must_use]
    pub(crate) fn snapshot_entries_in_scope(&self, scope: &Scope) -> FacadeEntriesSnapshot {
        let snap = self.adapter.registry.snapshot_state();
        self.reconcile_snapshot(&snap);

        let registry_cursor = Cursor(snap.revision());
        let registry_closed = snap.is_closed();

        // Candidate ids: registry entries passing the shared kind/namespace
        // filter (`in_scope` does NOT check visibility — the owner map below
        // supplies that half).
        let in_scope_ids: Vec<CapabilityId> = snap
            .entries()
            .keys()
            .map(|name| tool_id(name))
            .filter(|id| ToolBackendAdapter::in_scope(id, scope))
            .collect();

        // ONE bulk owner observation under ONE mutex. Membership is the live
        // binding set, so revoked/disposed ids are omitted.
        let owner_generations = self.tree.generations_for(in_scope_ids, &scope.visibility);

        // Build the filtered registry cut ONCE and SHARE it behind an `Arc`:
        // entries share identical id membership with the owner map, and
        // cloning a snapshot is cheap.
        let entries: Arc<HashMap<String, RegistryEntry>> = Arc::new(
            snap.entries()
                .iter()
                .filter(|(name, _)| owner_generations.contains_key(&tool_id(name)))
                .map(|(name, entry)| (name.clone(), entry.clone()))
                .collect(),
        );

        FacadeEntriesSnapshot {
            entries,
            registry_cursor,
            registry_closed,
            owner_generations,
        }
    }

    /// Map a registry mutation event onto the facade's capability change
    /// vocabulary. The `source` field is deliberately dropped — the capability
    /// change carries only `(id, revision)`.
    pub(super) fn map_registry_change(&self, change: RegistryChange) -> (Cursor, CapabilityChange) {
        match change {
            RegistryChange::Registered { name, revision, .. } => (
                Cursor(revision),
                CapabilityChange::Registered {
                    id: tool_id(&name),
                    revision: CapabilityRevision(revision),
                },
            ),
            RegistryChange::Replaced { name, revision, .. } => (
                Cursor(revision),
                CapabilityChange::Replaced {
                    id: tool_id(&name),
                    revision: CapabilityRevision(revision),
                },
            ),
            RegistryChange::Unregistered { name, revision, .. } => (
                Cursor(revision),
                CapabilityChange::Unregistered {
                    id: tool_id(&name),
                    revision: CapabilityRevision(revision),
                },
            ),
        }
    }

    /// Whether a registry-sourced capability change is visible under `scope`.
    ///
    /// Applies the SAME kind / namespace predicate the backend uses for
    /// `enumerate` / `snapshot_capabilities` (`ToolBackendAdapter::in_scope`)
    /// BEFORE the `OwnershipTree` visibility binding, so a change for a
    /// non-Tool kind or a foreign namespace is invisible everywhere.
    /// `Invalidated` is scope-wide (carries no id) and always passes, though
    /// `map_registry_change` never produces it — the drain only ever asks
    /// about per-tool changes.
    pub(super) fn change_in_scope(&self, change: &CapabilityChange, scope: &Scope) -> bool {
        let id = match change {
            CapabilityChange::Registered { id, .. }
            | CapabilityChange::Replaced { id, .. }
            | CapabilityChange::Unregistered { id, .. } => id,
            CapabilityChange::Invalidated => return true,
        };
        if !ToolBackendAdapter::in_scope(id, scope) {
            return false;
        }
        self.tree.generation(id, &scope.visibility).is_some()
    }

    /// Drain a live registry broadcast into a pure [`CapabilityChangeStream`],
    /// applying the same scope predicate as `describe`.
    ///
    /// Shared by `subscribe` (fresh receiver) and the lag test (a receiver
    /// taken BEFORE the snapshot so its 256-slot ring overflows). The receiver
    /// is the actual [`tokio::sync::broadcast::Receiver`]`<`[`RegistryChange`]`>`
    /// — never a test double — so the `Lagged` branch is exercised against the
    /// real registry broadcast.
    fn drain_registry(
        &self,
        scope: &Scope,
        snapshot: CapabilitySnapshot,
        mut receiver: broadcast::Receiver<RegistryChange>,
    ) -> CapabilityChangeStream {
        let mut stream = CapabilityChangeStream::from_snapshot(snapshot);
        loop {
            match receiver.try_recv() {
                Ok(change) => {
                    let (cursor, change) = self.map_registry_change(change);
                    // The registry broadcasts changes for ANY tool; honor the
                    // shared kind / namespace + visibility predicate so an
                    // out-of-scope tool is never leaked to the subscriber.
                    if self.change_in_scope(&change, scope) {
                        let _ = stream.append(cursor, change);
                    }
                }
                Err(TryRecvError::Lagged(_)) => {
                    // Lag: rebuild from a fresh snapshot and mark the stream
                    // invalidated — never fake per-event delivery.
                    let fresh = self.describe(scope.clone());
                    let _ = stream.invalidate_and_resync(fresh, Vec::new());
                    break;
                }
                Err(TryRecvError::Empty) | Err(TryRecvError::Closed) => break,
            }
        }
        stream
    }
}

impl Zahir for ZahirFacade {
    fn describe(&self, scope: Scope) -> CapabilitySnapshot {
        self.reconcile_registry_bindings();
        let (capabilities, revision) = self.adapter.snapshot_capabilities(&scope);
        let capabilities = capabilities
            .into_iter()
            .filter(|d| self.tree.generation(&d.id, &scope.visibility).is_some())
            .collect();
        CapabilitySnapshot {
            capabilities,
            committed_cursor: Cursor(revision),
        }
    }

    fn resolve(&self, reference: Reference) -> Result<BackendLease, ResolveError> {
        self.reconcile_registry_bindings();
        let mut descriptor = self
            .adapter
            .lookup(&reference.id)
            .ok_or(ResolveError::Unknown)?;
        // The registry descriptor is identity-level; the facade binds the
        // resolved descriptor to the reference's visibility so `validate_lease`
        // checks against the same binding key (custom visibility never falls
        // back to the default binding).
        descriptor.visibility = reference.visibility.clone();
        match self.tree.generation(&reference.id, &reference.visibility) {
            Some(owner_generation) => Ok(BackendLease {
                descriptor,
                owner_generation,
            }),
            None if self.tree.is_revoked(&reference.id) => Err(ResolveError::Revoked),
            None => Err(ResolveError::NotVisible),
        }
    }

    fn validate_lease(&self, lease: &BackendLease) -> Result<(), ResolveError> {
        // ① binding location — the lease's own visibility selects the binding.
        match self
            .tree
            .generation(&lease.descriptor.id, &lease.descriptor.visibility)
        {
            Some(current) => {
                // ② owner generation — a stale lease must fail closed.
                if current != lease.owner_generation {
                    return Err(ResolveError::StaleOwner);
                }
            }
            None if self.tree.is_revoked(&lease.descriptor.id) => {
                return Err(ResolveError::Revoked)
            }
            None => return Err(ResolveError::Unknown),
        }
        // ③ descriptor freshness — the current descriptor must still match the
        // lease's (revision + schema fingerprint). A replacement or unregister
        // closes to `Unknown`.
        match self.adapter.lookup(&lease.descriptor.id) {
            Some(current)
                if current.revision == lease.descriptor.revision
                    && current.schema.fingerprint == lease.descriptor.schema.fingerprint =>
            {
                Ok(())
            }
            _ => Err(ResolveError::Unknown),
        }
    }

    fn subscribe(&self, scope: Scope, _cursor: Cursor) -> CapabilityChangeStream {
        // ① subscribe BEFORE snapshot: the receiver exists before the snapshot
        // is read, so the synchronous drain never misses an event that lands in
        // the window between snapshot load and drain.
        let receiver = self.adapter.registry.subscribe();
        let snapshot = self.describe(scope.clone());
        // The caller `cursor` is IGNORED: this synchronous snapshot-anchored
        // API has no replay, so there is nothing to resume from. `from_snapshot`
        // anchors `committed_cursor` at the snapshot revision and `append`
        // rejects cursors <= committed_cursor, so events already reflected in
        // the snapshot are dropped, and a caller cursor ahead of the snapshot
        // fails closed to the snapshot cursor — events after the snapshot are
        // never silently dropped.
        self.drain_registry(&scope, snapshot, receiver)
    }

    fn project(&self, target: TransportTarget, scope: Scope) -> Projection {
        // Projection shares the dispatch generation: one `describe` read.
        let snapshot = self.describe(scope);
        Projection {
            target,
            metadata: ProjectionMetadata::default(),
            capabilities: snapshot.capabilities.iter().map(|d| d.id.clone()).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::ownership::{
        ClaimState, LifetimeScope, OwnerGeneration, OwnerRef, TaskId, VisibilityScope,
    };
    use crate::session::events::ToolOutput;
    use crate::tools::descriptor::ToolCapabilityDescriptor;
    use crate::tools::handlers::ToolHandler;
    use crate::tools::service::{ToolDefinition, ToolDefinitionMetadata, ToolError, ToolSource};
    use async_trait::async_trait;
    use serde_json::{json, Value};

    fn fake_def(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: format!("desc {name}"),
            input_schema: json!({"type": "object", "properties": {}}),
            source: ToolSource::Builtin,
            metadata: ToolDefinitionMetadata::default(),
        }
    }

    struct FakeHandler {
        definition: ToolDefinition,
    }

    #[async_trait]
    impl ToolHandler for FakeHandler {
        async fn invoke(&self, _input: Value) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput {
                value: json!({"tool": self.definition.name}),
                metadata: Default::default(),
            })
        }
        fn definition(&self) -> ToolDefinition {
            self.definition.clone()
        }
    }

    fn fake_handler(name: &str) -> FakeHandler {
        FakeHandler {
            definition: fake_def(name),
        }
    }

    /// Build a facade plus its registry and tree, so tests can mutate the
    /// registry (`register` / `unregister` / `replace`) and the tree
    /// (`register` / `bump` / `revoke`) after construction.
    fn host_parts(tools: &[&str]) -> (Arc<ZahirFacade>, ToolHandlerRegistry, Arc<OwnershipTree>) {
        let registry = ToolHandlerRegistry::new();
        for name in tools {
            registry
                .register(
                    ToolCapabilityDescriptor::from_definition(&fake_def(name), 0),
                    Arc::new(fake_handler(name)),
                )
                .expect("register test tool");
        }
        let tree = Arc::new(OwnershipTree::new());
        let facade = ZahirFacade::new(registry.clone(), tree.clone());
        (facade, registry, tree)
    }

    fn ref_for(id: &CapabilityId) -> Reference {
        Reference {
            id: id.clone(),
            visibility: VisibilityScope::default(),
        }
    }

    fn scope_all() -> Scope {
        Scope {
            kind: None,
            namespace: None,
            visibility: VisibilityScope::default(),
        }
    }

    #[test]
    fn resolve_mints_a_fresh_lease_with_current_generation() {
        let (host, _reg, _tree) = host_parts(&["a"]);
        let id = tool_id("a");
        let lease = host.resolve(ref_for(&id)).expect("resolve succeeds");
        // A freshly minted lease carries the generation read at resolve time;
        // it can never be `StaleOwner` at construction.
        assert_eq!(lease.owner_generation, OwnerGeneration(0));
        assert_eq!(host.validate_lease(&lease), Ok(()));
    }

    #[test]
    fn resolve_fails_closed_for_unknown_not_visible_revoked() {
        // Lookup miss → Unknown.
        let (host, _reg, _tree) = host_parts(&["a"]);
        let missing = tool_id("missing");
        assert_eq!(host.resolve(ref_for(&missing)), Err(ResolveError::Unknown));

        // No binding under a custom visibility → NotVisible.
        let ws = VisibilityScope {
            workspace: Some("w".to_string()),
            ..VisibilityScope::default()
        };
        let id = tool_id("a");
        assert_eq!(
            host.resolve(Reference {
                id: id.clone(),
                visibility: ws,
            }),
            Err(ResolveError::NotVisible)
        );

        // Revoked binding → Revoked.
        let (host, _reg, tree) = host_parts(&["a"]);
        assert!(host.resolve(ref_for(&id)).is_ok());
        tree.revoke(&id);
        assert_eq!(host.resolve(ref_for(&id)), Err(ResolveError::Revoked));
    }

    #[test]
    fn validate_lease_detects_stale_owner_after_bump() {
        let (host, _reg, tree) = host_parts(&["a"]);
        let id = tool_id("a");
        let lease = host.resolve(ref_for(&id)).expect("resolve succeeds");
        tree.bump(LifetimeScope::Runtime);
        assert_eq!(host.validate_lease(&lease), Err(ResolveError::StaleOwner));
    }

    #[test]
    fn validate_lease_detects_descriptor_freshness_after_replace() {
        let (host, reg, _tree) = host_parts(&["a"]);
        let id = tool_id("a");
        let lease = host.resolve(ref_for(&id)).expect("resolve succeeds");

        // Replace with a changed schema (different fingerprint) and matching
        // handler definition.
        let mut def = fake_def("a");
        def.input_schema = json!({"type": "object", "properties": {"x": {"type": "string"}}});
        let handler = FakeHandler {
            definition: def.clone(),
        };
        reg.replace(
            ToolCapabilityDescriptor::from_definition(&def, 0),
            Arc::new(handler),
        )
        .expect("replace succeeds");

        // Same owner generation, but the descriptor moved on → Unknown (not
        // StaleOwner).
        assert_eq!(host.validate_lease(&lease), Err(ResolveError::Unknown));
    }

    #[test]
    fn describe_and_resolve_share_one_registry_source() {
        let (host, _reg, _tree) = host_parts(&["a", "b"]);
        let snapshot = host.describe(scope_all());
        assert_eq!(snapshot.committed_cursor, Cursor(2));
        assert_eq!(snapshot.capabilities.len(), 2);
        assert!(snapshot
            .capabilities
            .iter()
            .all(|d| d.kind == crate::capability::descriptor::CapabilityKind::Tool));
    }

    #[test]
    fn subscribe_returns_pure_stream_starting_at_snapshot() {
        let (host, _reg, _tree) = host_parts(&["a"]);
        let stream = host.subscribe(scope_all(), Cursor(0));
        assert_eq!(stream.committed_cursor, Cursor(1));
        assert!(stream.changes.is_empty());
    }

    #[test]
    fn register_after_construction_is_visible_on_next_describe() {
        let (host, reg, _tree) = host_parts(&["a"]);
        let id_b = tool_id("b");
        assert_eq!(host.resolve(ref_for(&id_b)), Err(ResolveError::Unknown));
        reg.register(
            ToolCapabilityDescriptor::from_definition(&fake_def("b"), 0),
            Arc::new(fake_handler("b")),
        )
        .expect("register b");
        assert_eq!(host.describe(scope_all()).capabilities.len(), 2);
        assert!(host.resolve(ref_for(&id_b)).is_ok());
    }

    #[test]
    fn unregister_then_reregister_resolves() {
        let (host, reg, _tree) = host_parts(&["a"]);
        let id_a = tool_id("a");
        assert!(host.resolve(ref_for(&id_a)).is_ok());

        // Unregister does NOT revoke the ownership binding.
        assert!(reg.unregister("a").is_some());
        assert_eq!(host.resolve(ref_for(&id_a)), Err(ResolveError::Unknown));

        // Re-register succeeds: the preserved binding still resolves.
        reg.register(
            ToolCapabilityDescriptor::from_definition(&fake_def("a"), 0),
            Arc::new(fake_handler("a")),
        )
        .expect("reregister a");
        assert!(host.resolve(ref_for(&id_a)).is_ok());
    }

    #[test]
    fn describe_filters_by_visibility_binding() {
        let (host, _reg, tree) = host_parts(&["a", "b"]);
        let ws = VisibilityScope {
            workspace: Some("w".to_string()),
            ..VisibilityScope::default()
        };
        let ws_scope = Scope {
            kind: None,
            namespace: None,
            visibility: ws.clone(),
        };
        let id_b = tool_id("b");

        // Default visibility sees both.
        assert_eq!(host.describe(scope_all()).capabilities.len(), 2);
        // Workspace visibility has no binding yet → empty.
        assert_eq!(host.describe(ws_scope.clone()).capabilities.len(), 0);

        // Bind `b` under the workspace visibility.
        tree.register(
            id_b.clone(),
            OwnerRef::Runtime,
            LifetimeScope::Runtime,
            ws.clone(),
        )
        .expect("register b under ws");

        let snapshot = host.describe(ws_scope.clone());
        assert_eq!(snapshot.capabilities.len(), 1);
        assert_eq!(snapshot.capabilities[0].id, id_b);
        assert_eq!(snapshot.capabilities[0].visibility, ws);

        let lease = host
            .resolve(Reference {
                id: id_b.clone(),
                visibility: ws.clone(),
            })
            .expect("resolve b under ws");
        assert_eq!(lease.descriptor.visibility, ws);
        assert_eq!(host.validate_lease(&lease), Ok(()));

        let projection = host.project(TransportTarget::AcpJson, ws_scope);
        assert_eq!(projection.capabilities, vec![id_b]);
    }

    #[test]
    fn subscribe_cursor_ahead_of_snapshot_fails_closed_to_snapshot() {
        let (host, _reg, _tree) = host_parts(&["a"]);
        let stream = host.subscribe(scope_all(), Cursor(99));
        // Fail-closed: clamped to the snapshot cursor (1), never fakes 99.
        assert_eq!(stream.committed_cursor, Cursor(1));
        assert!(stream.changes.is_empty());
    }

    #[test]
    fn subscribe_change_filter_honors_visibility_binding() {
        let (host, _reg, tree) = host_parts(&["a"]);
        let id_a = tool_id("a");
        let id_b = tool_id("b");
        let ws = VisibilityScope {
            workspace: Some("w".to_string()),
            ..VisibilityScope::default()
        };
        let ws_scope = Scope {
            kind: None,
            namespace: None,
            visibility: ws.clone(),
        };

        // Reconcile creates the DEFAULT binding for "a" only ("b" is not in
        // the registry), so under default visibility "a" is visible and "b" is
        // not.
        host.reconcile_registry_bindings();
        let reg_a = CapabilityChange::Registered {
            id: id_a.clone(),
            revision: CapabilityRevision(1),
        };
        let reg_b = CapabilityChange::Registered {
            id: id_b.clone(),
            revision: CapabilityRevision(1),
        };
        assert!(host.change_in_scope(&reg_a, &scope_all()));
        assert!(!host.change_in_scope(&reg_b, &scope_all()));

        // A custom visibility with no binding sees neither tool.
        assert!(!host.change_in_scope(&reg_a, &ws_scope));

        // Bind "a" under the workspace visibility → visible; "b" stays hidden.
        tree.register(
            id_a.clone(),
            OwnerRef::Runtime,
            LifetimeScope::Runtime,
            ws.clone(),
        )
        .expect("register a under ws");
        assert!(host.change_in_scope(&reg_a, &ws_scope));
        assert!(!host.change_in_scope(&reg_b, &ws_scope));

        // Scope-wide `Invalidated` carries no id and always passes.
        assert!(host.change_in_scope(&CapabilityChange::Invalidated, &ws_scope));
    }

    #[test]
    fn change_in_scope_filters_wrong_kind_and_wrong_namespace() {
        let (host, _reg, _tree) = host_parts(&["a"]);
        host.reconcile_registry_bindings();
        let change = CapabilityChange::Registered {
            id: tool_id("a"),
            revision: CapabilityRevision(1),
        };

        // A non-Tool kind scope never admits a Tool change.
        let skill_scope = Scope {
            kind: Some(crate::capability::descriptor::CapabilityKind::Skill),
            namespace: None,
            visibility: VisibilityScope::default(),
        };
        assert!(!host.change_in_scope(&change, &skill_scope));

        // A foreign namespace scope never admits a Tool change.
        let foreign_ns_scope = Scope {
            kind: None,
            namespace: Some("aleph/skills".to_string()),
            visibility: VisibilityScope::default(),
        };
        assert!(!host.change_in_scope(&change, &foreign_ns_scope));

        // Control: the unfiltered scope still admits it.
        assert!(host.change_in_scope(&change, &scope_all()));
    }

    #[test]
    fn subscribe_lagged_resyncs_to_fresh_snapshot() {
        let (host, reg, _tree) = host_parts(&["a"]);
        // ① Receiver BEFORE snapshot, exactly like `subscribe`. The receiver is
        // never drained, so the 256-slot ring overflows during the mutation
        // storm below and the next `try_recv` observes `Lagged`.
        let receiver = reg.subscribe();
        let snapshot = host.describe(scope_all());

        // ② >256 valid registry mutations: each `register` sends one broadcast
        // event (revision increments), overflowing the undrained receiver.
        for i in 0..300 {
            let name = format!("overflow_{i}");
            reg.register(
                ToolCapabilityDescriptor::from_definition(&fake_def(&name), 0),
                Arc::new(fake_handler(&name)),
            )
            .expect("register overflow tool");
        }

        // ③ Drain through the SAME production helper the subscribe path uses.
        let stream = host.drain_registry(&scope_all(), snapshot, receiver);

        // ④ Exactly one scope-wide invalidation, never a fake per-event replay.
        assert_eq!(stream.changes.len(), 1);
        assert!(matches!(&stream.changes[0], CapabilityChange::Invalidated));

        // ⑤ Fresh snapshot cursor and contents reflect the ACTUAL latest
        // registry: 1 original + 300 overflow tools = 301, cursor = revision.
        let latest_revision = reg.snapshot_state().revision();
        assert_eq!(stream.committed_cursor, Cursor(latest_revision));
        assert_eq!(host.describe(scope_all()).capabilities.len(), 301);
    }

    #[test]
    fn reconcile_preserves_incumbent_task_owner_and_claim() {
        let (host, _reg, tree) = host_parts(&["a"]);
        let id = tool_id("a");
        let vis = VisibilityScope::default();

        // An incumbent Task owner holds the binding with a live claim before
        // any facade reconcile runs.
        let incumbent = OwnerRef::Task(TaskId("worker".into()));
        tree.register(
            id.clone(),
            incumbent.clone(),
            LifetimeScope::Task,
            vis.clone(),
        )
        .expect("register incumbent");
        let claim = tree
            .claim(&id, &vis, incumbent.clone(), "r1".to_string())
            .expect("claim incumbent");

        // Reconcile is insert-if-absent: it must not clobber the Task owner,
        // its generation, or its live claim — even when called repeatedly.
        host.reconcile_registry_bindings();
        host.reconcile_registry_bindings();

        assert_eq!(tree.generation(&id, &vis), Some(OwnerGeneration(0)));
        assert_eq!(tree.claim_state(&claim, &id, &vis), ClaimState::Active);
        // The incumbent can still claim; the Runtime reconcile owner cannot.
        assert!(tree
            .claim(&id, &vis, incumbent, "r2".to_string())
            .is_some());
        assert!(tree
            .claim(&id, &vis, OwnerRef::Runtime, "r3".to_string())
            .is_none());
    }

    #[test]
    fn snapshot_entry_handler_and_descriptor_share_revision() {
        let (host, reg, _tree) = host_parts(&["a", "b"]);
        let snap = host.snapshot_entries_in_scope(&scope_all());
        // Two registered tools → registry revision 2.
        assert_eq!(snap.registry_cursor, Cursor(2));
        assert_eq!(snap.entries.len(), 2);
        assert_eq!(snap.owner_generations.len(), 2);
        for (name, entry) in snap.entries.iter() {
            // The frozen descriptor is never ahead of the snapshot cursor.
            assert!(entry.descriptor.revision <= snap.registry_cursor.0);
            // The handler definition matches the frozen descriptor it shares
            // a registry generation with.
            assert_eq!(entry.handler.definition().name, entry.descriptor.name);
            assert_eq!(
                entry.handler.definition().input_schema,
                entry.descriptor.input_schema
            );
            // Entries and owner map share identical id membership.
            assert!(snap.owner_generations.contains_key(&tool_id(name)));
        }

        // Replace "a" with a changed schema: a freshly obtained snapshot
        // changes atomically (cursor advances, handler + descriptor move on).
        let mut def = fake_def("a");
        def.input_schema = json!({"type": "object", "properties": {"x": {"type": "string"}}});
        reg.replace(
            ToolCapabilityDescriptor::from_definition(&def, 0),
            Arc::new(FakeHandler { definition: def.clone() }),
        )
        .expect("replace a");
        let snap2 = host.snapshot_entries_in_scope(&scope_all());
        assert_eq!(snap2.registry_cursor, Cursor(3));
        let a = snap2.entries.get("a").expect("a present");
        assert_eq!(a.handler.definition().input_schema, def.input_schema);
        assert_eq!(a.descriptor.input_schema, def.input_schema);

        // Unregister "b": a freshly obtained snapshot drops it atomically.
        assert!(reg.unregister("b").is_some());
        let snap3 = host.snapshot_entries_in_scope(&scope_all());
        assert_eq!(snap3.registry_cursor, Cursor(4));
        assert!(snap3.entries.contains_key("a"));
        assert!(!snap3.entries.contains_key("b"));
        assert_eq!(snap3.owner_generations.len(), 1);
    }

    #[test]
    fn snapshot_entries_in_scope_filters_revoked_without_second_predicate() {
        let (host, _reg, tree) = host_parts(&["a", "b"]);
        host.reconcile_registry_bindings();
        let id_a = tool_id("a");
        let id_b = tool_id("b");
        // Revoke only "a"; the reconcile must not resurrect the tombstone.
        assert!(tree.revoke(&id_a));

        let snap = host.snapshot_entries_in_scope(&scope_all());
        assert!(!snap.entries.contains_key("a"), "revoked id is excluded");
        assert!(snap.entries.contains_key("b"), "live id is preserved");
        assert!(!snap.owner_generations.contains_key(&id_a));
        assert!(snap.owner_generations.contains_key(&id_b));
    }

    #[test]
    fn snapshot_entries_in_scope_keeps_registry_and_owner_domains_separate() {
        let (host, _reg, tree) = host_parts(&["a", "b"]);
        let before = host.snapshot_entries_in_scope(&scope_all());
        assert_eq!(before.registry_cursor, Cursor(2));
        assert_eq!(
            before.owner_generations.get(&tool_id("a")),
            Some(&OwnerGeneration(0))
        );
        assert_eq!(
            before.owner_generations.get(&tool_id("b")),
            Some(&OwnerGeneration(0))
        );

        // Bump ownership WITHOUT any registry mutation.
        tree.bump(LifetimeScope::Runtime);
        let after = host.snapshot_entries_in_scope(&scope_all());

        // Registry cursor unchanged: no registry mutation happened.
        assert_eq!(after.registry_cursor, Cursor(2));
        // Owner generations were recomputed and advanced.
        assert_eq!(
            after.owner_generations.get(&tool_id("a")),
            Some(&OwnerGeneration(1))
        );
        assert_eq!(
            after.owner_generations.get(&tool_id("b")),
            Some(&OwnerGeneration(1))
        );
        assert_eq!(after.entries.len(), 2);
    }

    #[test]
    fn snapshot_captures_registry_closed() {
        let (host, reg, _tree) = host_parts(&["a"]);
        assert!(!host.snapshot_entries_in_scope(&scope_all()).registry_closed);
        reg.close();
        assert!(host.snapshot_entries_in_scope(&scope_all()).registry_closed);
    }

    #[test]
    fn snapshot_entries_in_scope_honors_kind_namespace_visibility() {
        use crate::capability::descriptor::CapabilityKind;
        let (host, _reg, _tree) = host_parts(&["a", "b"]);

        // Non-Tool kind → no entries.
        let skill_scope = Scope {
            kind: Some(CapabilityKind::Skill),
            namespace: None,
            visibility: VisibilityScope::default(),
        };
        assert!(host
            .snapshot_entries_in_scope(&skill_scope)
            .entries
            .is_empty());

        // Foreign namespace → no entries.
        let foreign = Scope {
            kind: None,
            namespace: Some("aleph/skills".to_string()),
            visibility: VisibilityScope::default(),
        };
        assert!(host
            .snapshot_entries_in_scope(&foreign)
            .entries
            .is_empty());

        // Custom visibility with no bindings → owner map empty, so entries
        // empty (identical membership).
        let ws = VisibilityScope {
            workspace: Some("w".to_string()),
            ..VisibilityScope::default()
        };
        let ws_scope = Scope {
            kind: None,
            namespace: None,
            visibility: ws,
        };
        assert!(host
            .snapshot_entries_in_scope(&ws_scope)
            .entries
            .is_empty());

        // Default visibility → both present.
        assert_eq!(
            host.snapshot_entries_in_scope(&scope_all()).entries.len(),
            2
        );
    }

    #[test]
    fn held_snapshot_is_immutable_across_replace_and_unregister() {
        let (host, reg, _tree) = host_parts(&["a", "b"]);
        let old = host.snapshot_entries_in_scope(&scope_all());
        let old_cursor = old.registry_cursor;
        let old_schema = old.entries["a"].descriptor.input_schema.clone();
        let old_gen = old.owner_generations.get(&tool_id("a")).copied();

        // Replace "a" with a changed schema and unregister "b": the OLD
        // snapshot must stay frozen (its `Arc`s point at the old generation).
        let mut def = fake_def("a");
        def.input_schema =
            json!({"type": "object", "properties": {"y": {"type": "number"}}});
        reg.replace(
            ToolCapabilityDescriptor::from_definition(&def, 0),
            Arc::new(FakeHandler { definition: def.clone() }),
        )
        .expect("replace a");
        assert!(reg.unregister("b").is_some());

        assert_eq!(old.registry_cursor, old_cursor);
        assert_eq!(old.entries["a"].descriptor.input_schema, old_schema);
        assert_eq!(old.owner_generations.get(&tool_id("a")).copied(), old_gen);
        assert!(old.entries.contains_key("b"), "old snapshot still has b");

        // The new snapshot reflects the replacement + unregister.
        let new = host.snapshot_entries_in_scope(&scope_all());
        assert_eq!(new.entries["a"].descriptor.input_schema, def.input_schema);
        assert!(!new.entries.contains_key("b"));
    }

    #[test]
    fn snapshot_entries_field_is_shared_arc_and_clone_shares() {
        let (host, _reg, _tree) = host_parts(&["a", "b"]);
        let snap = host.snapshot_entries_in_scope(&scope_all());

        // Compile-time contract: `entries` is a SHARED map
        // (`Arc<HashMap<..>>`), not an owned `HashMap`. If the field ever
        // reverts to an owned map, this binding fails to type-check (semantic
        // API RED) before any runtime assertion runs — the crate contract is
        // the `Arc` type itself.
        let entries_ref: &Arc<HashMap<String, RegistryEntry>> = &snap.entries;
        assert_eq!(entries_ref.len(), 2);

        // Runtime contract: `Clone` shares the SAME allocation
        // (pointer-equal `Arc`), not a deep copy of the entries map.
        let cloned = snap.clone();
        assert!(Arc::ptr_eq(&snap.entries, &cloned.entries));
        assert_eq!(snap.registry_cursor, cloned.registry_cursor);
        assert_eq!(snap.registry_closed, cloned.registry_closed);
        assert_eq!(snap.owner_generations, cloned.owner_generations);
    }
}
