//! Capability backend — the storage/query half of the capability contract.
//!
//! `CapabilityBackend` is the trait every concrete registry implements.
//! `ToolBackendAdapter` wraps the existing
//! [`crate::tools::registry::ToolHandlerRegistry`], while
//! [`LegacyCapabilityHandlerRegistry`] owns the lightweight special-capability
//! handler entries without duplicating their descriptor source.

use crate::capability::descriptor::{
    CapabilityDescriptor, CapabilityFacet, CapabilityFacets, CapabilityId, CapabilityKind,
    CapabilityRevision, ProjectionMetadata, SchemaRef,
};
use crate::capability::events::{CapabilityEvent, CapabilityEventEmitter, HandlerResult};
use crate::capability::facade::{CapabilitySnapshot, Cursor, Scope};
use crate::capability::ownership::{LifetimeScope, OwnerRef, VisibilityScope};
use crate::capability::provider::{
    ProviderHandle, ProviderId, ProviderLifecycle, ProviderTransport,
};
use crate::capability::special as special_backend;
use crate::sync_primitives::Arc;
use crate::tools::descriptor::{
    ReplayPolicy, ToolCapabilityDescriptor, ToolSource, canonicalize_json, canonicalize_services,
};
use async_trait::async_trait;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

/// Legacy synchronous special registry retained for existing ambient/terminal/UI
/// tests and event instrumentation. New captured async records live in `special`.
pub use self::{
    LegacyCapabilityDispatchError as CapabilityDispatchError,
    LegacyCapabilityHandler as CapabilityHandler,
    LegacyCapabilityHandlerRegistry as CapabilityHandlerRegistry,
    LegacyCapabilityRegistryError as CapabilityRegistryError,
};

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
pub enum LegacyCapabilityDispatchError {
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
pub trait LegacyCapabilityHandler: Send + Sync {
    fn invoke(&self, input: Value) -> Result<Value, LegacyCapabilityDispatchError>;
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LegacyCapabilityRegistryError {
    #[error("capability is already registered: {0:?}")]
    Duplicate(CapabilityId),
    #[error("capability id must have a non-empty namespace and name")]
    InvalidId,
    #[error("capability revision is exhausted")]
    RevisionExhausted,
}

/// Compatibility adapter for the pre-async terminal/UI handler contract.
/// Storage and dispatch still belong to the async special registry.
struct LegacyHandlerAdapter {
    handler: Arc<dyn LegacyCapabilityHandler>,
}

#[async_trait]
impl special_backend::CapabilityHandler for LegacyHandlerAdapter {
    async fn invoke(
        &self,
        input: Value,
        _call: special_backend::CapabilityHandlerCall,
        _cancel: CancellationToken,
    ) -> Result<Value, special_backend::CapabilityDispatchError> {
        self.handler.invoke(input).map_err(|error| match error {
            LegacyCapabilityDispatchError::InvalidInput(message) => {
                special_backend::CapabilityDispatchError::InvalidInput(message)
            }
            LegacyCapabilityDispatchError::Execution(message) => {
                special_backend::CapabilityDispatchError::Execution(message)
            }
            LegacyCapabilityDispatchError::NotFound(id) => {
                special_backend::CapabilityDispatchError::NotFound(id)
            }
        })
    }
}

/// Compatibility registry that delegates all record storage and handler
/// resolution to the one async special registry. Its old sync methods remain
/// only for terminal/UI callers that have not migrated yet.
#[derive(Clone)]
pub struct LegacyCapabilityHandlerRegistry {
    special: special_backend::CapabilityHandlerRegistry,
    events: Option<CapabilityEventEmitter>,
}

impl Default for LegacyCapabilityHandlerRegistry {
    fn default() -> Self {
        Self {
            special: special_backend::CapabilityHandlerRegistry::new(),
            events: None,
        }
    }
}

impl LegacyCapabilityHandlerRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Observe this registry's invocations; internal dispatch uses Runtime as actor.
    #[must_use]
    pub fn with_events(self, events: CapabilityEventEmitter) -> Self {
        Self {
            events: Some(events),
            ..self
        }
    }

    pub fn register(
        &self,
        mut descriptor: CapabilityDescriptor,
        handler: Arc<dyn LegacyCapabilityHandler>,
    ) -> Result<CapabilityId, LegacyCapabilityRegistryError> {
        if descriptor.id.namespace.trim().is_empty() || descriptor.id.name.trim().is_empty() {
            return Err(LegacyCapabilityRegistryError::InvalidId);
        }
        descriptor.revision = CapabilityRevision(self.special.revision().saturating_add(1));
        let provider = ProviderHandle::new(ProviderLifecycle::ready(
            ProviderId::new("legacy-compat")
                .map_err(|_| LegacyCapabilityRegistryError::InvalidId)?,
            ProviderTransport::InProcess,
        ));
        self.special
            .register_legacy(
                descriptor,
                Arc::new(LegacyHandlerAdapter { handler }),
                provider,
            )
            .map_err(|error| match error {
                special_backend::CapabilityRegistryError::Duplicate(id) => {
                    LegacyCapabilityRegistryError::Duplicate(id)
                }
                special_backend::CapabilityRegistryError::RevisionExhausted => {
                    LegacyCapabilityRegistryError::RevisionExhausted
                }
                _ => LegacyCapabilityRegistryError::InvalidId,
            })
    }

    pub(crate) fn dispatch(
        &self,
        id: &CapabilityId,
        input: Value,
    ) -> Result<Value, LegacyCapabilityDispatchError> {
        let operation = input
            .get("operation")
            .and_then(Value::as_str)
            .unwrap_or("invoke")
            .to_owned();
        let result = self
            .special
            .dispatch_legacy(id, input)
            .map_err(|error| match error {
                special_backend::CapabilityDispatchError::InvalidInput(message) => {
                    LegacyCapabilityDispatchError::InvalidInput(message)
                }
                special_backend::CapabilityDispatchError::Execution(message) => {
                    LegacyCapabilityDispatchError::Execution(message)
                }
                special_backend::CapabilityDispatchError::NotFound(id) => {
                    LegacyCapabilityDispatchError::NotFound(id)
                }
                special_backend::CapabilityDispatchError::Unavailable => {
                    LegacyCapabilityDispatchError::Execution("capability unavailable".into())
                }
                special_backend::CapabilityDispatchError::Cancelled => {
                    LegacyCapabilityDispatchError::Execution("capability cancelled".into())
                }
            });
        if let Some(events) = &self.events {
            let handler_result = match &result {
                Ok(_) => HandlerResult::Success,
                Err(LegacyCapabilityDispatchError::InvalidInput(_)) => HandlerResult::InvalidInput,
                Err(LegacyCapabilityDispatchError::Execution(_)) => HandlerResult::ExecutionFailed,
                Err(LegacyCapabilityDispatchError::NotFound(_)) => HandlerResult::NotFound,
            };
            events.emit(
                OwnerRef::Runtime,
                CapabilityEvent::CapabilityInvoked {
                    id: id.clone(),
                    operation,
                    handler_result,
                },
            );
        }
        result
    }

    #[must_use]
    pub fn revision(&self) -> u64 {
        self.special.revision()
    }
}

impl CapabilityBackend for LegacyCapabilityHandlerRegistry {
    fn lookup(&self, id: &CapabilityId) -> Option<CapabilityDescriptor> {
        self.special
            .capture(id)
            .map(|capture| capture.descriptor().clone())
    }

    fn enumerate(&self, scope: &Scope) -> Vec<CapabilityDescriptor> {
        self.special
            .describe(scope)
            .into_iter()
            .map(|capture| {
                let mut descriptor = capture.descriptor().clone();
                descriptor.visibility = scope.visibility.clone();
                descriptor
            })
            .collect()
    }

    fn snapshot(&self, scope: &Scope) -> CapabilitySnapshot {
        CapabilitySnapshot {
            capabilities: self.enumerate(scope),
            committed_cursor: Cursor(self.special.revision()),
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

    struct JsonLegacyCapabilityHandler;

    impl LegacyCapabilityHandler for JsonLegacyCapabilityHandler {
        fn invoke(
            &self,
            input: serde_json::Value,
        ) -> Result<serde_json::Value, LegacyCapabilityDispatchError> {
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
        let registry = LegacyCapabilityHandlerRegistry::new();
        let descriptor = special_descriptor("test/ambient", "duplicate");
        let id = descriptor.id.clone();
        registry
            .register(descriptor.clone(), Arc::new(JsonLegacyCapabilityHandler))
            .unwrap();

        assert_eq!(
            registry.register(descriptor, Arc::new(JsonLegacyCapabilityHandler)),
            Err(LegacyCapabilityRegistryError::Duplicate(id))
        );
    }

    #[test]
    fn handler_registry_lookup_of_an_invalid_id_is_none() {
        let registry = LegacyCapabilityHandlerRegistry::new();
        let invalid = CapabilityId {
            namespace: String::new(),
            name: String::new(),
        };

        assert!(registry.lookup(&invalid).is_none());
    }

    #[test]
    fn handler_registry_rejects_registration_with_an_invalid_id() {
        let registry = LegacyCapabilityHandlerRegistry::new();
        let descriptor = special_descriptor("", "invalid");

        assert_eq!(
            registry.register(descriptor, Arc::new(JsonLegacyCapabilityHandler)),
            Err(LegacyCapabilityRegistryError::InvalidId)
        );
    }

    #[test]
    fn handler_registry_enumerates_only_the_requested_namespace() {
        let registry = LegacyCapabilityHandlerRegistry::new();
        registry
            .register(
                special_descriptor("test/one", "a"),
                Arc::new(JsonLegacyCapabilityHandler),
            )
            .unwrap();
        registry
            .register(
                special_descriptor("test/two", "b"),
                Arc::new(JsonLegacyCapabilityHandler),
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
        let registry = LegacyCapabilityHandlerRegistry::new();
        assert_eq!(
            registry.snapshot(&Scope::default()).committed_cursor,
            Cursor(0)
        );

        registry
            .register(
                special_descriptor("test/revision", "a"),
                Arc::new(JsonLegacyCapabilityHandler),
            )
            .unwrap();
        assert_eq!(
            registry.snapshot(&Scope::default()).committed_cursor,
            Cursor(1)
        );

        registry
            .register(
                special_descriptor("test/revision", "b"),
                Arc::new(JsonLegacyCapabilityHandler),
            )
            .unwrap();
        assert_eq!(
            registry.snapshot(&Scope::default()).committed_cursor,
            Cursor(2)
        );
    }

    #[test]
    fn handler_registry_rejects_revision_exhaustion() {
        let registry = LegacyCapabilityHandlerRegistry::new();
        registry.special.set_revision_for_test(u64::MAX);

        assert_eq!(
            registry.register(
                special_descriptor("test/exhausted", "a"),
                Arc::new(JsonLegacyCapabilityHandler)
            ),
            Err(LegacyCapabilityRegistryError::RevisionExhausted)
        );
    }

    #[test]
    fn handler_registry_supports_concurrent_registration_and_enumeration() {
        let registry = LegacyCapabilityHandlerRegistry::new();
        std::thread::scope(|scope| {
            for index in 0..8 {
                let registry = registry.clone();
                scope.spawn(move || {
                    registry
                        .register(
                            special_descriptor("test/concurrent", &format!("cap-{index}")),
                            Arc::new(JsonLegacyCapabilityHandler),
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
        assert!(
            CapabilityBackend::snapshot(&backend, &events)
                .capabilities
                .is_empty()
        );
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
