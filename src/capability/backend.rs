//! Capability backend — the storage/query half of the capability contract.
//!
//! `CapabilityBackend` is the trait every concrete registry (tools today,
//! skills/plugins/agents later) implements; `ToolBackendAdapter` is the one
//! live adapter, wrapping the existing [`crate::tools::registry::ToolHandlerRegistry`]
//! as a capability source of truth without duplicating it.

use crate::capability::descriptor::{
    CapabilityDescriptor, CapabilityId, CapabilityKind, CapabilityRevision, ProjectionMetadata,
    SchemaRef,
};
use crate::capability::facade::Scope;
use crate::capability::ownership::{LifetimeScope, OwnerGeneration, OwnerRef, VisibilityScope};
use crate::tools::descriptor::ToolCapabilityDescriptor;

/// Namespace under which the tool backend registers its capabilities.
pub const TOOL_NAMESPACE: &str = "aleph/tools";

/// The storage/query contract every capability registry implements.
///
/// `lookup` resolves one capability by id; `enumerate` lists a scope;
/// `generation` reports the current owner generation. Only the Tool backend is
/// implemented this phase ([`ToolBackendAdapter`]); the other eight kinds are
/// deferred and non-invocable — no backend implements them and none is mounted.
pub trait CapabilityBackend: Send + Sync {
    fn lookup(&self, id: &CapabilityId) -> Option<CapabilityDescriptor>;
    fn enumerate(&self, scope: &Scope) -> Vec<CapabilityDescriptor>;
    fn generation(&self) -> OwnerGeneration;
}

/// The one live backend: adapts [`crate::tools::registry::ToolHandlerRegistry`]
/// to the capability contract without duplicating it.
pub struct ToolBackendAdapter {
    pub registry: crate::tools::registry::ToolHandlerRegistry,
}

impl ToolBackendAdapter {
    /// Whether `id` falls inside `scope`'s kind / namespace restrictions.
    fn in_scope(id: &CapabilityId, scope: &Scope) -> bool {
        if let Some(kind) = scope.kind {
            if kind != CapabilityKind::Tool {
                return false;
            }
        }
        if let Some(ns) = &scope.namespace {
            if !id.namespace.starts_with(ns.as_str()) {
                return false;
            }
        }
        true
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
            .filter(|d| Self::in_scope(&d.id, scope))
            .collect()
    }

    fn generation(&self) -> OwnerGeneration {
        // `OwnerGeneration` is the owner-layer's monotonic counter, distinct
        // from the tool registry's own revision (which tracks tool-registration
        // mutations). They MUST NOT be aliased: a registry bump (a tool
        // re-registration) does not invalidate owner-held leases, only an
        // owner-side bump does. Until Task 2 introduces the real
        // `OwnershipTree` to drive this counter, return a stable `0` so it
        // matches the `owner_generation: OwnerGeneration(0)` field every
        // descriptor produced by `to_descriptor` carries — anything else
        // would make the current descriptor appear already-stale at
        // construction time, before the owner layer even exists.
        //
        // Task 2 (OwnershipTree) replaces this constant with a read from the
        // ownership engine's monotonic nonce.
        OwnerGeneration(0)
    }
}

/// Fold a tool descriptor into the kind-agnostic [`CapabilityDescriptor`].
fn to_descriptor(tool: &ToolCapabilityDescriptor) -> CapabilityDescriptor {
    CapabilityDescriptor {
        id: CapabilityId {
            namespace: TOOL_NAMESPACE.to_string(),
            name: tool.name.clone(),
        },
        kind: CapabilityKind::Tool,
        schema: SchemaRef {
            version: tool.schema_version,
            // Fingerprinting is computed by the describe/projection path;
            // Task 1 only establishes the type and leaves the digest uncomputed.
            fingerprint: [0u8; 32],
        },
        revision: CapabilityRevision(tool.revision),
        owner_generation: OwnerGeneration(0),
        lifetime: LifetimeScope::Runtime,
        visibility: VisibilityScope::default(),
        owner: OwnerRef::Runtime,
        services: Vec::new(),
        metadata: ProjectionMetadata::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::descriptor::{CapabilityId, CapabilityKind};
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
    fn generation_matches_descriptor_owner_generation() {
        // The backend's current owner generation must equal the
        // `owner_generation` stamped on every descriptor it produces.
        // Otherwise any consumer that holds a `BackendLease` from
        // `Zahir::resolve` and checks it against `backend.generation()` would
        // see a phantom invalidation the instant the lease is created — a
        // fail-closed-by-design contract broken at the source.
        //
        // This assertion is the reason `generation()` and `to_descriptor()`
        // both return `OwnerGeneration(0)`: they are decoupled from the
        // registry's own revision (a tool-mutation counter, not an
        // owner-issuance counter), and pinned to a stable `0` until Task 2
        // introduces the real `OwnershipTree` to drive both.
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
        assert_eq!(backend.generation(), d.owner_generation);
    }
}
