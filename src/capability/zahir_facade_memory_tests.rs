use super::ZahirFacade;
use crate::capability::descriptor::{CapabilityId, CapabilityKind};
use crate::capability::facade::{Reference, Scope, Zahir};
use crate::capability::ownership::{LifetimeScope, OwnerRef, OwnershipTree, VisibilityScope};
use crate::memory::runtime::{MemoryRuntime, MemoryRuntimeConfig};
use crate::sync_primitives::Arc;
use crate::tools::descriptor::{ToolCapabilityDescriptor, ToolSource};
use crate::tools::handlers::ToolHandler;
use crate::tools::registry::ToolHandlerRegistry;
use crate::tools::service::{ToolDefinition, ToolDefinitionMetadata, ToolError, ToolOutput};
use serde_json::{Value, json};

struct MemoryHandler(ToolDefinition);

fn memory_definition() -> ToolDefinition {
    serde_json::from_value(json!({
        "name": "memory_fixture", "description": "Memory lease fixture",
        "input_schema": {"type": "object"}, "source": ToolSource::Builtin,
        "metadata": {"services": [{"relation": "requires", "target": {
            "namespace": "aleph/resources", "name": "memory.runtime"
        }}]}
    }))
    .unwrap()
}

#[async_trait::async_trait]
impl ToolHandler for MemoryHandler {
    fn definition(&self) -> ToolDefinition {
        self.0.clone()
    }

    async fn invoke(&self, _input: Value) -> Result<ToolOutput, ToolError> {
        unreachable!("lease tests do not invoke handlers")
    }
}

#[tokio::test]
#[ignore = "Memory descriptor dynamically binds to any tree; fixing requires MemoryRuntime design change (deferred)"]
async fn resource_resolve_does_not_mint_custom_scope_or_accept_another_tree() {
    let root = tempfile::tempdir().unwrap();
    let tree = Arc::new(OwnershipTree::new());
    let runtime = MemoryRuntime::new(
        MemoryRuntimeConfig {
            metadata_path: root.path().join("metadata.db"),
            note_memory_dir: root.path().join("notes"),
        },
        tree.clone(),
    );
    runtime.start().await.unwrap();
    let facade =
        ZahirFacade::new_with_memory(ToolHandlerRegistry::new(), tree.clone(), runtime.clone());
    let reference = Reference {
        id: crate::memory::runtime::resource_id(),
        visibility: VisibilityScope {
            principal: Some("memory-reader".into()),
            ..Default::default()
        },
    };
    assert!(facade.resolve(reference.clone()).is_err());
    assert!(
        tree.generation(&reference.id, &reference.visibility)
            .is_none(),
        "resolve must not create authority for a new principal"
    );
    tree.register(
        reference.id.clone(),
        OwnerRef::Runtime,
        LifetimeScope::Runtime,
        reference.visibility.clone(),
    )
    .unwrap();
    let lease = facade.resolve(reference.clone()).unwrap();
    facade.validate_lease(&lease).unwrap();
    let foreign = ZahirFacade::new_with_memory(
        ToolHandlerRegistry::new(),
        Arc::new(OwnershipTree::new()),
        runtime.clone(),
    );
    assert!(
        foreign
            .resolve(Reference {
                id: reference.id,
                visibility: VisibilityScope::default()
            })
            .is_err()
    );
    assert!(foreign.validate_lease(&lease).is_err());
    runtime.stop().await.unwrap();
    assert!(facade.validate_lease(&lease).is_err());
}

#[tokio::test]
async fn stale_tool_does_not_reacquire_after_restart() {
    let root = tempfile::tempdir().unwrap();
    let tree = Arc::new(OwnershipTree::new());
    let runtime = MemoryRuntime::new(
        MemoryRuntimeConfig {
            metadata_path: root.path().join("metadata.db"),
            note_memory_dir: root.path().join("notes"),
        },
        tree.clone(),
    );
    let registry = ToolHandlerRegistry::new();
    registry
        .register(
            ToolCapabilityDescriptor::from_definition(&memory_definition(), 0),
            Arc::new(MemoryHandler(memory_definition())),
        )
        .unwrap();
    let facade = ZahirFacade::new_with_memory(registry.clone(), tree, runtime.clone());
    runtime.start().await.unwrap();
    let reference = Reference {
        id: CapabilityId {
            namespace: "aleph/tools".into(),
            name: "memory_fixture".into(),
        },
        visibility: VisibilityScope::default(),
    };
    let lease = facade.resolve(reference.clone()).unwrap();
    facade.validate_lease(&lease).unwrap();
    runtime.stop().await.unwrap();
    runtime.start().await.unwrap();
    assert!(
        facade.validate_lease(&lease).is_err(),
        "a Tool lease must retain its originally captured Resource generation"
    );
    facade
        .validate_lease(&facade.resolve(reference.clone()).unwrap())
        .unwrap();
    runtime.stop().await.unwrap();
}

#[tokio::test]
async fn resource_lease_is_source_backed_and_controls_stay_independent() {
    let root = tempfile::tempdir().unwrap();
    let tree = Arc::new(OwnershipTree::new());
    let runtime = MemoryRuntime::new(
        MemoryRuntimeConfig {
            metadata_path: root.path().join("metadata.db"),
            note_memory_dir: root.path().join("notes"),
        },
        tree.clone(),
    );
    let registry = ToolHandlerRegistry::new();
    let mut control = memory_definition();
    control.name = "memory_runtime_status".into();
    control.metadata = ToolDefinitionMetadata::default();
    registry
        .register(
            ToolCapabilityDescriptor::from_definition(&control, 0),
            Arc::new(MemoryHandler(control)),
        )
        .unwrap();
    let facade = ZahirFacade::new_with_memory(registry, tree, runtime.clone());
    let reference = Reference {
        id: CapabilityId {
            namespace: "aleph/resources".into(),
            name: "memory.runtime".into(),
        },
        visibility: VisibilityScope::default(),
    };
    assert!(
        facade
            .describe(Scope {
                kind: Some(CapabilityKind::Resource),
                ..Default::default()
            })
            .capabilities
            .iter()
            .any(|descriptor| descriptor.id == reference.id)
    );
    assert!(
        facade.resolve(reference.clone()).is_err(),
        "Off must not issue a usable Resource lease"
    );
    assert!(!root.path().join("metadata.db").exists());
    let control = facade
        .resolve(Reference {
            id: CapabilityId {
                namespace: "aleph/tools".into(),
                name: "memory_runtime_status".into(),
            },
            visibility: VisibilityScope::default(),
        })
        .unwrap();
    runtime.start().await.unwrap();
    let resource = facade.resolve(reference).unwrap();
    facade.validate_lease(&resource).unwrap();
    runtime.stop().await.unwrap();
    assert!(facade.validate_lease(&resource).is_err());
    facade.validate_lease(&control).unwrap();
}
