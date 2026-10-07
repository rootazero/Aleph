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
use crate::capability::ownership::{LifetimeScope, OwnerRef, VisibilityScope};
use crate::tools::descriptor::{canonicalize_json, ToolCapabilityDescriptor};
use sha2::{Digest, Sha256};

/// Namespace under which the tool backend registers its capabilities.
pub const TOOL_NAMESPACE: &str = "aleph/tools";

/// The storage/query contract every capability registry implements.
///
/// `lookup` resolves one capability by id; `enumerate` lists a scope. Only the
/// Tool backend is implemented this phase ([`ToolBackendAdapter`]); the other
/// eight kinds are deferred and non-invocable — no backend implements them and
/// none is mounted.
pub trait CapabilityBackend: Send + Sync {
    fn lookup(&self, id: &CapabilityId) -> Option<CapabilityDescriptor>;
    fn enumerate(&self, scope: &Scope) -> Vec<CapabilityDescriptor>;
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
            if !id.namespace.starts_with(ns.as_str()) {
                return false;
            }
        }
        true
    }

    /// `(descriptors, revision)` from ONE registry generation.
    ///
    /// Both halves come from a single `snapshot_state()` load, so a caller can
    /// never pair descriptors from generation N with a revision from N+1.
    /// `describe`/`subscribe` build their `CapabilitySnapshot` from THIS — never
    /// `enumerate` + a separate `revision()` read (two loads can tear).
    #[must_use]
    pub fn snapshot_capabilities(&self, scope: &Scope) -> (Vec<CapabilityDescriptor>, u64) {
        let snap = self.registry.snapshot_state();
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
            .filter(|d| Self::in_scope(&d.id, scope))
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
            .filter(|d| Self::in_scope(&d.id, scope))
            .collect()
    }

}

/// Fold a tool descriptor into the kind-agnostic [`CapabilityDescriptor`].
fn to_descriptor(tool: &ToolCapabilityDescriptor) -> CapabilityDescriptor {
    let mut schema_bytes = Vec::new();
    canonicalize_json(&tool.input_schema, &mut schema_bytes);
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
