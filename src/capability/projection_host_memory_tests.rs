use super::ProjectionHost;
use crate::capability::descriptor::CapabilityKind;
use crate::capability::facade::{Scope, Zahir};
use crate::capability::ownership::OwnershipTree;
use crate::memory::runtime::{MemoryRuntime, MemoryRuntimeConfig};
use crate::sync_primitives::Arc;
use crate::tools::registry::ToolHandlerRegistry;

#[tokio::test]
async fn resource_is_discoverable_without_fake_tool_entry() {
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
    let cursor = registry.revision();
    let host = ProjectionHost::mount_with_memory(registry.clone(), tree, runtime.clone(), false);
    assert!(host.wait_until_ready().await);
    let delivered = host.current_snapshot().unwrap();
    assert!(delivered.entries.is_empty());
    assert!(delivered.owner_generations.is_empty());
    assert!(registry.entries_snapshot().is_empty());
    assert_eq!(
        delivered.resources.len(),
        1,
        "the real default applier must deliver the Resource projection"
    );
    assert_eq!(delivered.resources[0].kind, CapabilityKind::Resource);
    assert_eq!(delivered.resources[0].id.name, "memory.runtime");
    assert_eq!(delivered.registry_cursor.0, cursor);
    assert!(!root.path().join("metadata.db").exists());
    runtime.start().await.unwrap();
    runtime.stop().await.unwrap();
    assert_eq!(
        registry.revision(),
        cursor,
        "Resource transitions must not manufacture Tool registry revisions"
    );
    let described = host.facade().describe(Scope {
        kind: Some(CapabilityKind::Resource),
        ..Default::default()
    });
    assert_eq!(described.capabilities.len(), 1);
    assert_eq!(described.committed_cursor.0, cursor);
    let _ = host.close_and_await().await;
}
