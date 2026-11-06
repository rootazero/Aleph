//! Capability backend — the storage/query half of the capability contract.
//!
//! `CapabilityBackend` is the trait every concrete registry implements.
//! `ToolBackendAdapter` wraps the existing
//! [`crate::tools::registry::ToolHandlerRegistry`], while
//! [`CapabilityHandlerRegistry`] owns the lightweight special-capability
//! handler entries without duplicating their descriptor source.

use crate::capability::descriptor::{
    CapabilityDescriptor, CapabilityFacet, CapabilityFacets, CapabilityId, CapabilityKind,
    CapabilityRevision, ProjectionMetadata, SchemaRef,
};
use crate::capability::facade::{CapabilitySnapshot, Cursor, Scope};
use crate::capability::ownership::{LifetimeScope, OwnerRef, VisibilityScope};
use crate::sync_primitives::Arc;
use crate::tools::descriptor::{
    canonicalize_json, canonicalize_services, ReplayPolicy, ToolCapabilityDescriptor, ToolSource,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::RwLock;

/// Namespace under which the tool backend registers its capabilities.
pub const TOOL_NAMESPACE: &str = "aleph/tools";

/// The storage/query contract every capability registry implements.
///
/// `lookup` resolves one capability by id; `enumerate` lists a scope. The Tool
/// backend and the lightweight special-capability handler backend are live;
/// other descriptor kinds remain descriptive until their owning backend is
/// mounted.
pub trait CapabilityBackend: Send + Sync {
    fn lookup(&self, id: &CapabilityId) -> Option<CapabilityDescriptor>;
    fn enumerate(&self, scope: &Scope) -> Vec<CapabilityDescriptor>;
    fn snapshot(&self, scope: &Scope) -> CapabilitySnapshot;
}

/// Errors produced by a capability handler during invocation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CapabilityDispatchError {
    #[error("capability handler received invalid input: {0}")]
    InvalidInput(String),
    #[error("capability handler execution failed: {0}")]
    Execution(String),
    #[error("capability is not registered: {0:?}")]
    NotFound(CapabilityId),
}

/// The callable boundary for non-Tool capabilities.
///
/// Handlers receive owned JSON so the registry never exposes mutable internal
/// state. A handler may perform an effect at this boundary, but its descriptor
/// remains an immutable value owned by the registry entry.
pub trait CapabilityHandler: Send + Sync {
    fn invoke(&self, input: Value) -> Result<Value, CapabilityDispatchError>;
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CapabilityRegistryError {
    #[error("capability is already registered: {0:?}")]
    Duplicate(CapabilityId),
    #[error("capability id must have a non-empty namespace and name")]
    InvalidId,
    #[error("capability revision is exhausted")]
    RevisionExhausted,
}

#[derive(Clone)]
struct HandlerEntry {
    descriptor: CapabilityDescriptor,
    handler: Arc<dyn CapabilityHandler>,
}

#[derive(Default)]
struct HandlerRegistryState {
    entries: HashMap<CapabilityId, HandlerEntry>,
    revision: u64,
}

/// A descriptor-backed registry for callable special capabilities.
///
/// The map stores descriptor and handler together as one entry, so discovery
/// and dispatch cannot pair values from independent registries. This backend is
/// intentionally separate from [`ToolBackendAdapter`]: Tool dispatch keeps its
/// existing approval and provider gates, while special handlers use this small
/// typed boundary until their concrete platform bridges are mounted.
#[derive(Clone, Default)]
pub struct CapabilityHandlerRegistry {
    state: Arc<RwLock<HandlerRegistryState>>,
}

impl CapabilityHandlerRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &self,
        mut descriptor: CapabilityDescriptor,
        handler: Arc<dyn CapabilityHandler>,
    ) -> Result<CapabilityId, CapabilityRegistryError> {
        if descriptor.id.namespace.trim().is_empty() || descriptor.id.name.trim().is_empty() {
            return Err(CapabilityRegistryError::InvalidId);
        }
        let mut state = self
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.entries.contains_key(&descriptor.id) {
            return Err(CapabilityRegistryError::Duplicate(descriptor.id));
        }
        let revision = state
            .revision
            .checked_add(1)
            .ok_or(CapabilityRegistryError::RevisionExhausted)?;
        descriptor.revision = CapabilityRevision(revision);
        let id = descriptor.id.clone();
        state.entries.insert(
            id.clone(),
            HandlerEntry {
                descriptor,
                handler,
            },
        );
        state.revision = revision;
        Ok(id)
    }

    /// Invoke a registered handler without holding the registry lock.
    ///
    /// This is an internal, pre-gated seam. Callers must validate the Phase 6
    /// lease, policy, visibility, and Task 1 activation contract before entry;
    /// the registry only performs lookup and handler execution here. Keeping it
    /// crate-private prevents external callers from treating an ID plus JSON as
    /// an authorization decision.
    pub(crate) fn dispatch(
        &self,
        id: &CapabilityId,
        input: Value,
    ) -> Result<Value, CapabilityDispatchError> {
        let handler = {
            let state = self
                .state
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state
                .entries
                .get(id)
                .map(|entry| Arc::clone(&entry.handler))
        }
        .ok_or_else(|| CapabilityDispatchError::NotFound(id.clone()))?;
        handler.invoke(input)
    }

    #[must_use]
    pub fn revision(&self) -> u64 {
        self.state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .revision
    }

    fn descriptors(&self, scope: &Scope) -> Vec<CapabilityDescriptor> {
        let state = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Self::descriptors_from_state(&state, scope)
    }

    fn descriptors_from_state(
        state: &HandlerRegistryState,
        scope: &Scope,
    ) -> Vec<CapabilityDescriptor> {
        state
            .entries
            .values()
            .map(|entry| {
                let mut descriptor = entry.descriptor.clone();
                descriptor.visibility = scope.visibility.clone();
                descriptor
            })
            .filter(|descriptor| {
                scope.kind.is_none_or(|kind| descriptor.kind == kind)
                    && scope
                        .namespace
                        .as_ref()
                        .is_none_or(|namespace| descriptor.id.namespace == *namespace)
                    && scope.facet.is_none_or(|facet| descriptor.supports(facet))
            })
            .collect()
    }
}

impl CapabilityBackend for CapabilityHandlerRegistry {
    fn lookup(&self, id: &CapabilityId) -> Option<CapabilityDescriptor> {
        self.state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entries
            .get(id)
            .map(|entry| entry.descriptor.clone())
    }

    fn enumerate(&self, scope: &Scope) -> Vec<CapabilityDescriptor> {
        self.descriptors(scope)
    }

    fn snapshot(&self, scope: &Scope) -> CapabilitySnapshot {
        let state = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        CapabilitySnapshot {
            capabilities: Self::descriptors_from_state(&state, scope),
            committed_cursor: Cursor(state.revision),
        }
    }
}

/// The one live backend: adapts [`crate::tools::registry::ToolHandlerRegistry`]
/// to the capability contract without duplicating it.
pub struct ToolBackendAdapter {
    pub registry: crate::tools::registry::ToolHandlerRegistry,
}

impl ToolBackendAdapter {
    /// Whether `id` falls inside `scope`'s kind / namespace restrictions.
    ///
    /// `pub(super)` so the capability sibling module (`zahir_facade`) shares
    /// THIS predicate when filtering registry change events — one definition of
    /// "in scope" for both `enumerate`/`snapshot_capabilities` and the
    /// subscribe drain.
    pub(super) fn in_scope(id: &CapabilityId, scope: &Scope) -> bool {
        if let Some(kind) = scope.kind {
            if kind != CapabilityKind::Tool {
                return false;
            }
        }
        if let Some(ns) = &scope.namespace {
            if id.namespace != *ns {
                return false;
            }
        }
        true
    }

    /// Whether a descriptor falls inside `scope`, including its behavior facet.
    pub(crate) fn descriptor_in_scope(descriptor: &CapabilityDescriptor, scope: &Scope) -> bool {
        scope.kind.is_none_or(|kind| descriptor.kind == kind)
            && scope
                .namespace
                .as_ref()
                .is_none_or(|namespace| descriptor.id.namespace == *namespace)
            && scope.facet.is_none_or(|facet| descriptor.supports(facet))
    }

    /// `(descriptors, revision)` from ONE registry generation.
    ///
    /// Both halves come from a single `snapshot_state()` load, so a caller can
    /// never pair descriptors from generation N with a revision from N+1.
    /// `describe`/`subscribe` build their `CapabilitySnapshot` from THIS — never
    /// `enumerate` + a separate `revision()` read (two loads can tear).
    #[must_use]
    pub fn snapshot_capabilities(&self, scope: &Scope) -> (Vec<CapabilityDescriptor>, u64) {
        let snap = self.registry.canonical_publication();
        let capabilities = snap
            .entries()
            .values()
            .map(|entry| {
                let mut descriptor = to_descriptor(entry.descriptor.as_ref());
                // The registry descriptor is identity-level; the facade binds
                // the returned descriptor to the requested visibility scope.
                descriptor.visibility = scope.visibility.clone();
                descriptor
            })
            .filter(|d| Self::descriptor_in_scope(d, scope))
            .collect();
        (capabilities, snap.revision())
    }
}

impl CapabilityBackend for ToolBackendAdapter {
    fn lookup(&self, id: &CapabilityId) -> Option<CapabilityDescriptor> {
        if id.namespace != TOOL_NAMESPACE {
            return None;
        }
        self.registry
            .descriptor(&id.name)
            .map(|desc| to_descriptor(&desc))
    }

    fn enumerate(&self, scope: &Scope) -> Vec<CapabilityDescriptor> {
        let snapshot = self.registry.descriptor_snapshot();
        snapshot
            .values()
            .map(|desc| to_descriptor(desc))
            .filter(|d| Self::descriptor_in_scope(d, scope))
            .collect()
    }

    fn snapshot(&self, scope: &Scope) -> CapabilitySnapshot {
        let (capabilities, revision) = self.snapshot_capabilities(scope);
        CapabilitySnapshot {
            capabilities,
            committed_cursor: Cursor(revision),
        }
    }
}

/// Fold a tool descriptor into the kind-agnostic [`CapabilityDescriptor`].
pub(super) fn to_descriptor(tool: &ToolCapabilityDescriptor) -> CapabilityDescriptor {
    let mut schema_bytes = Vec::new();
    canonicalize_json(&tool.input_schema, &mut schema_bytes);
    canonicalize_services(&tool.services, &mut schema_bytes);
    let digest = Sha256::digest(&schema_bytes);
    let mut fingerprint = [0u8; 32];
    fingerprint.copy_from_slice(&digest);

    CapabilityDescriptor {
        id: CapabilityId {
            namespace: TOOL_NAMESPACE.to_string(),
            name: tool.name.clone(),
        },
        kind: CapabilityKind::Tool,
        schema: SchemaRef {
            version: tool.schema_version,
            fingerprint,
        },
        revision: CapabilityRevision(tool.revision),
        lifetime: LifetimeScope::Runtime,
        visibility: VisibilityScope::default(),
        owner: OwnerRef::Runtime,
        services: tool.services.clone(),
        metadata: ProjectionMetadata::default(),
        facets: tool_facets(tool),
    }
}

pub(super) fn tool_facets(tool: &ToolCapabilityDescriptor) -> CapabilityFacets {
    let facets = CapabilityFacets::new().with(CapabilityFacet::Callable);
    if tool.replay_policy == ReplayPolicy::Unsafe
        || matches!(
            tool.source,
            ToolSource::Mcp { .. } | ToolSource::Extension { .. }
        )
    {
        return facets.with(CapabilityFacet::ExternalEffect);
    }
    facets
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::descriptor::{CapabilityFacet, CapabilityId, CapabilityKind};
    use crate::capability::facade::Cursor;
    use crate::session::events::ToolOutput;
    use crate::sync_primitives::Arc;
    use crate::tools::descriptor::ToolCapabilityDescriptor;
    use crate::tools::handlers::ToolHandler;
    use crate::tools::registry::ToolHandlerRegistry;
    use crate::tools::service::{ToolDefinition, ToolDefinitionMetadata, ToolError, ToolSource};
    use async_trait::async_trait;
    use serde_json::json;

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
        name: String,
    }

    #[async_trait]
    impl ToolHandler for FakeHandler {
        async fn invoke(&self, _input: serde_json::Value) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput {
                value: json!({"tool": self.name}),
                metadata: Default::default(),
            })
        }
        fn definition(&self) -> ToolDefinition {
            fake_def(&self.name)
        }
    }

    fn fake_handler(name: &str) -> FakeHandler {
        FakeHandler {
            name: name.to_string(),
        }
    }

    struct JsonCapabilityHandler;

    impl CapabilityHandler for JsonCapabilityHandler {
        fn invoke(
            &self,
            input: serde_json::Value,
        ) -> Result<serde_json::Value, CapabilityDispatchError> {
            Ok(input)
        }
    }

    fn special_descriptor(namespace: &str, name: &str) -> CapabilityDescriptor {
        CapabilityDescriptor {
            id: CapabilityId {
                namespace: namespace.into(),
                name: name.into(),
            },
            kind: CapabilityKind::Ambient,
            schema: SchemaRef {
                version: 1,
                fingerprint: [0; 32],
            },
            revision: CapabilityRevision(0),
            lifetime: LifetimeScope::Runtime,
            visibility: VisibilityScope::default(),
            owner: OwnerRef::Runtime,
            services: Vec::new(),
            metadata: ProjectionMetadata::default(),
            facets: CapabilityFacets::new().with(CapabilityFacet::Callable),
        }
    }

    #[test]
    fn handler_registry_rejects_duplicate_ids() {
        let registry = CapabilityHandlerRegistry::new();
        let descriptor = special_descriptor("test/ambient", "duplicate");
        let id = descriptor.id.clone();
        registry
            .register(descriptor.clone(), Arc::new(JsonCapabilityHandler))
            .unwrap();

        assert_eq!(
            registry.register(descriptor, Arc::new(JsonCapabilityHandler)),
            Err(CapabilityRegistryError::Duplicate(id))
        );
    }

    #[test]
    fn handler_registry_lookup_of_an_invalid_id_is_none() {
        let registry = CapabilityHandlerRegistry::new();
        let invalid = CapabilityId {
            namespace: String::new(),
            name: String::new(),
        };

        assert!(registry.lookup(&invalid).is_none());
    }

    #[test]
    fn handler_registry_rejects_registration_with_an_invalid_id() {
        let registry = CapabilityHandlerRegistry::new();
        let descriptor = special_descriptor("", "invalid");

        assert_eq!(
            registry.register(descriptor, Arc::new(JsonCapabilityHandler)),
            Err(CapabilityRegistryError::InvalidId)
        );
    }

    #[test]
    fn handler_registry_enumerates_only_the_requested_namespace() {
        let registry = CapabilityHandlerRegistry::new();
        registry
            .register(
                special_descriptor("test/one", "a"),
                Arc::new(JsonCapabilityHandler),
            )
            .unwrap();
        registry
            .register(
                special_descriptor("test/two", "b"),
                Arc::new(JsonCapabilityHandler),
            )
            .unwrap();
        let scope = Scope {
            namespace: Some("test/one".into()),
            ..Scope::default()
        };

        let descriptors = registry.enumerate(&scope);
        assert_eq!(descriptors.len(), 1);
        assert_eq!(descriptors[0].id.namespace, "test/one");
    }

    #[test]
    fn handler_registry_snapshot_cursor_tracks_registration_revision() {
        let registry = CapabilityHandlerRegistry::new();
        assert_eq!(
            registry.snapshot(&Scope::default()).committed_cursor,
            Cursor(0)
        );

        registry
            .register(
                special_descriptor("test/revision", "a"),
                Arc::new(JsonCapabilityHandler),
            )
            .unwrap();
        assert_eq!(
            registry.snapshot(&Scope::default()).committed_cursor,
            Cursor(1)
        );

        registry
            .register(
                special_descriptor("test/revision", "b"),
                Arc::new(JsonCapabilityHandler),
            )
            .unwrap();
        assert_eq!(
            registry.snapshot(&Scope::default()).committed_cursor,
            Cursor(2)
        );
    }

    #[test]
    fn handler_registry_rejects_revision_exhaustion() {
        let registry = CapabilityHandlerRegistry::new();
        registry
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .revision = u64::MAX;

        assert_eq!(
            registry.register(
                special_descriptor("test/exhausted", "a"),
                Arc::new(JsonCapabilityHandler)
            ),
            Err(CapabilityRegistryError::RevisionExhausted)
        );
    }

    #[test]
    fn handler_registry_supports_concurrent_registration_and_enumeration() {
        let registry = CapabilityHandlerRegistry::new();
        std::thread::scope(|scope| {
            for index in 0..8 {
                let registry = registry.clone();
                scope.spawn(move || {
                    registry
                        .register(
                            special_descriptor("test/concurrent", &format!("cap-{index}")),
                            Arc::new(JsonCapabilityHandler),
                        )
                        .unwrap();
                });
            }
            for _ in 0..8 {
                let registry = registry.clone();
                scope.spawn(move || {
                    for _ in 0..32 {
                        let scope = Scope {
                            namespace: Some("test/concurrent".into()),
                            ..Scope::default()
                        };
                        assert!(registry.enumerate(&scope).len() <= 8);
                    }
                });
            }
        });

        let scope = Scope {
            namespace: Some("test/concurrent".into()),
            ..Scope::default()
        };
        assert_eq!(registry.enumerate(&scope).len(), 8);
    }

    #[test]
    fn required_resource_roundtrips_canonical_metadata() {
        let dependency = json!([{
            "relation": "requires", "target": {"namespace": "aleph/resources", "name": "memory.runtime"}
        }]);
        let mut wire = serde_json::to_value(fake_def("memory_search")).unwrap();
        wire["metadata"]["services"] = dependency.clone();
        let descriptor =
            ToolCapabilityDescriptor::from_definition(&serde_json::from_value(wire).unwrap(), 1);
        let services = to_descriptor(&descriptor).services;
        assert_eq!(services.len(), 1);
        assert_eq!(services[0].target.namespace, "aleph/resources");
        assert_eq!(services[0].target.name, "memory.runtime");
        assert_eq!(
            services[0].relation,
            crate::capability::descriptor::ServiceRelation::Requires
        );
    }

    #[test]
    fn tool_name_round_trips_to_descriptor() {
        let reg = ToolHandlerRegistry::new();
        let desc = ToolCapabilityDescriptor::from_definition(&fake_def("hello"), 0);
        reg.register(desc, Arc::new(fake_handler("hello"))).unwrap();
        let backend = ToolBackendAdapter { registry: reg };
        let d = backend
            .lookup(&CapabilityId {
                namespace: "aleph/tools".into(),
                name: "hello".into(),
            })
            .unwrap();
        assert_eq!(d.kind, CapabilityKind::Tool);
        assert_eq!(d.revision.0, 1);
    }

    #[test]
    fn tool_descriptor_exposes_orthogonal_behavior_facets() {
        let descriptor = to_descriptor(&ToolCapabilityDescriptor::from_definition(
            &fake_def("facets"),
            0,
        ));

        assert!(descriptor.supports(CapabilityFacet::Callable));
        assert!(!descriptor.supports(CapabilityFacet::ProviderLifecycle));
        assert!(!descriptor.supports(CapabilityFacet::EventSource));
        assert!(descriptor.supports(CapabilityFacet::ExternalEffect));
    }

    #[test]
    fn snapshot_returns_descriptors_and_revision_from_one_publication() {
        let reg = ToolHandlerRegistry::new();
        reg.register(
            ToolCapabilityDescriptor::from_definition(&fake_def("snapshot"), 0),
            Arc::new(fake_handler("snapshot")),
        )
        .unwrap();
        let backend = ToolBackendAdapter { registry: reg };
        let callable = Scope {
            facet: Some(CapabilityFacet::Callable),
            ..Scope::default()
        };
        let events = Scope {
            facet: Some(CapabilityFacet::EventSource),
            ..Scope::default()
        };

        let snapshot = CapabilityBackend::snapshot(&backend, &callable);
        assert_eq!(snapshot.capabilities.len(), 1);
        assert_eq!(snapshot.capabilities[0].kind, CapabilityKind::Tool);
        assert_eq!(snapshot.committed_cursor, Cursor(1));
        assert!(CapabilityBackend::snapshot(&backend, &events)
            .capabilities
            .is_empty());
    }

    #[test]
    fn snapshot_filters_by_behavior_facet_without_changing_kind() {
        let reg = ToolHandlerRegistry::new();
        reg.register(
            ToolCapabilityDescriptor::from_definition(&fake_def("callable"), 0),
            Arc::new(fake_handler("callable")),
        )
        .unwrap();
        let backend = ToolBackendAdapter { registry: reg };
        let callable = Scope {
            facet: Some(CapabilityFacet::Callable),
            ..Scope::default()
        };
        let events = Scope {
            facet: Some(CapabilityFacet::EventSource),
            ..Scope::default()
        };

        assert_eq!(backend.snapshot_capabilities(&callable).0.len(), 1);
        assert!(backend.snapshot_capabilities(&events).0.is_empty());
        assert_eq!(
            backend.snapshot_capabilities(&callable).0[0].kind,
            CapabilityKind::Tool
        );
    }
    #[test]
    fn to_descriptor_fingerprint_is_canonical_and_stable() {
        let a = ToolCapabilityDescriptor::from_definition(
            &ToolDefinition {
                name: "fp".to_string(),
                description: "fp".to_string(),
                input_schema: serde_json::from_str(
                    r#"{"type":"object","properties":{"b":{"type":"string"},"a":{"type":"number"}}}"#,
                )
                .unwrap(),
                source: ToolSource::Builtin,
                metadata: ToolDefinitionMetadata::default(),
            },
            0,
        );
        let b = ToolCapabilityDescriptor::from_definition(
            &ToolDefinition {
                name: "fp".to_string(),
                description: "fp".to_string(),
                input_schema: serde_json::from_str(
                    r#"{"type":"object","properties":{"a":{"type":"number"},"b":{"type":"string"}}}"#,
                )
                .unwrap(),
                source: ToolSource::Builtin,
                metadata: ToolDefinitionMetadata::default(),
            },
            0,
        );
        let da = to_descriptor(&a);
        let db = to_descriptor(&b);
        // Same schema, different key order → same fingerprint.
        assert_eq!(da.schema.fingerprint, db.schema.fingerprint);
        assert_ne!(da.schema.fingerprint, [0u8; 32]);
    }
}
